//! Executable command safety contracts, split to bound each required query.
use super::*;

kernel_fn! {
/// Question answer rows / #771: one clarification or one paid continuation.
/// K5: the world entry for answer_node; one node write-back, one next-question
/// write and at most one notice, so the world frame is proved once here.
pub(super) fn on_answer(w: &mut World, i: usize, id: u64, answer: Answer,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
    ensures (node_checked(*old(w), old(w).nodes@[i as int])
            && answer_progress_input(old(w).nodes@[i as int], id, answer)
            ==> distance(final(w).nodes@[i as int]) < distance(old(w).nodes@[i as int])),
        (old(w).nodes@[i as int].phase is Closed ==> final(w).nodes@[i as int].phase is Closed),
        final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo,
        final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint,
        (settle_shape(old(w).nodes@[i as int]) ==> settle_shape(final(w).nodes@[i as int])),
        final(w).nodes@.len() == old(w).nodes@.len(),
        forall|j: int| 0 <= j < old(w).nodes@.len() && j != i
            ==> final(w).nodes@[j] == old(w).nodes@[j],
        final(w).nodes@[i as int].generation as int <= old(w).nodes@[i as int].generation as int + 1,
        final(w).nodes@[i as int].next_seq == old(w).nodes@[i as int].next_seq,
        final(w).nodes@[i as int].credit.start == old(w).nodes@[i as int].credit.start,
        final(w).nodes@[i as int].credit.earned == old(w).nodes@[i as int].credit.earned,
        final(w).nodes@[i as int].credit.added as int <= old(w).nodes@[i as int].credit.added as int + 255,
        final(w).nodes@[i as int].credit.spent as int <= old(w).nodes@[i as int].credit.spent as int + 1,
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
        final(w).next_ticket == old(w).next_ticket,
        final(w).next_question as int <= old(w).next_question as int + 1,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len <= old(out).len + 1,
        final(w).nodes@[i as int].proposed == old(w).nodes@[i as int].proposed,
        final(w).nodes@[i as int].adopted == old(w).nodes@[i as int].adopted,
        (old(w).nodes@[i as int].phase is Closed ==> final(w).nodes@[i as int].phase == old(w).nodes@[i as int].phase),
        handler_safety::answer_pair(old(w).nodes@[i as int], final(w).nodes@[i as int], true, id, answer,
            final(w).dependencies@, NodeId(i as u64)),
        transition_safety::notice_only(*old(out), *final(out), NodeId(i as u64)),
        (node_checked(*old(w), old(w).nodes@[i as int])
            && final(w).nodes@[i as int] != old(w).nodes@[i as int]
            ==> (match old(w).nodes@[i as int].question {
                Some(q) => q.id == id,
                None => false,
            }) && (match final(w).nodes@[i as int].question {
                Some(q) => q.id != id,
                None => true,
            })),

        (final(w).nodes@[i as int].holds ==> old(w).nodes@[i as int].holds),
        (final(w).nodes@[i as int].pool ==> old(w).nodes@[i as int].pool),
        final(w).nodes@[i as int].ticket == old(w).nodes@[i as int].ticket || final(w).nodes@[i as int].ticket.is_none(),
        final(w).nodes@[i as int].credit.withheld == old(w).nodes@[i as int].credit.withheld,
        old(w).next_question <= final(w).next_question,
        (retirement::node_rules(*old(w), old(w).nodes@[i as int])
            ==> retirement::node_rules(*final(w), final(w).nodes@[i as int])),
] {
    kernel_snapshot! { before_answer_rules = *w }
    kernel_proof! {
        if retirement::node_rules(*w, w.nodes@[i as int]) {
            handler_safety::rules_to_v(*w, w.nodes@[i as int]);
        }
    }

    kernel_proof! {
        if node_checked(*w, w.nodes@[i as int]) { handler_safety::checked_node_ok(*w, w.nodes@[i as int]); }
    }
    kernel_snapshot! { before_answer = w.nodes@[i as int] }
    let withhold = match w.nodes[i].question {
        Some(q) => w.nodes[i].phase == Phase::AwaitingRob
            && q.id == id
            && matches!(answer, Answer::Continue | Answer::AddCredit(_))
            && allowed(q.kind, answer),
        None => false,
    };
    kernel_proof! {
        assert(withhold == handler_safety::answer_withholds(before_answer@, id, answer));
    }
    let (n1, nq1, note) = answer_node(w.nodes[i], w.next_question, id, answer);
    kernel_proof! { handler_safety::step_withheld(before_answer@, n1); }
    w.nodes[i] = n1;
    if withhold {
        kernel_snapshot! { continued = w.nodes@[i as int] }
        w.withhold_dependency_credit(i);
        kernel_proof! {
            let n = w.nodes@[i as int];
            assert(n == handler_safety::unwithheld(continued@, n.credit.withheld));
            handler_safety::distance_withheld(continued@, n.credit.withheld);
            if continued@ == before_answer@ {
                // An answer that left the node AwaitingRob withholds nothing.
                handler_safety::unwithheld_same(before_answer@);
            }
        }
    }
    kernel_proof! {
        handler_safety::answer_pair_intro(before_answer@, n1, w.nodes@[i as int], id, answer,
            w.dependencies@, NodeId(i as u64));
    }
    w.next_question = nq1;
    match note {
        Some(x) => notice(out, i, x),
        None => {}
    }

    kernel_proof! {
        if retirement::node_rules(before_answer_rules@, before_answer_rules@.nodes@[i as int]) {
            handler_safety::rules_world_update(before_answer_rules@, *w,
                before_answer_rules@.nodes@[i as int], w.nodes@[i as int]);
        }
    }
}
}

kernel_fn! {
/// K5 design 4: on_answer's unchanged statements with an exact, per-field world post,
/// so answer_command transfers one node equation and no field rows.
pub(super) fn on_answer_exact(w: &mut World, i: usize, id: u64, answer: Answer,
    out: &mut Outbox) -> (result: ()) [
    requires i < old(w).nodes@.len(), old(out).len < OUTBOX,
        node_headroom(old(w).nodes@[i as int]),
        arithmetic_consistent(old(w).nodes@[i as int]),
        old(w).next_question < u64::MAX,
    ensures final(w).nodes@ == old(w).nodes@.update(i as int, apply_command_safety::held_answer(
            old(w).nodes@[i as int], old(w).next_question, id, answer, old(w).dependencies@,
            NodeId(i as u64))),
        final(w).next_question == apply_answer_values::answered(
            old(w).nodes@[i as int], old(w).next_question, id, answer).1,
        final(w).next_ticket == old(w).next_ticket,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(w).dependencies@ == old(w).dependencies@,
        final(out).len <= old(out).len + 1,
        transition_safety::notice_only(*old(out), *final(out), NodeId(i as u64)),
] {
    kernel_snapshot! { before_exact = *w }
    kernel_snapshot! { before_answer = w.nodes@[i as int] }
    let withhold = match w.nodes[i].question {
        Some(q) => w.nodes[i].phase == Phase::AwaitingRob
            && q.id == id
            && matches!(answer, Answer::Continue | Answer::AddCredit(_))
            && allowed(q.kind, answer),
        None => false,
    };
    kernel_proof! {
        assert(withhold == handler_safety::answer_withholds(before_answer@, id, answer));
    }
    let (n1, nq1, note) = answer_value(w.nodes[i], w.next_question, id, answer);
    w.nodes[i] = n1;
    if withhold {
        w.withhold_dependency_credit(i);
        kernel_proof! {
            let n = w.nodes@[i as int];
            assert(n == handler_safety::unwithheld(n1, n.credit.withheld));
        }
    }
    kernel_proof! {
        apply_command_safety::held_answer_is(before_answer@, before_exact@.next_question, id, answer,
            before_exact@.dependencies@, NodeId(i as u64), w.nodes@[i as int]);
        assert(w.nodes@ =~= before_exact@.nodes@.update(i as int, w.nodes@[i as int]));
    }
    w.next_question = nq1;
    match note {
        Some(x) => notice(out, i, x),
        None => {}
    }
}
}

kernel_fn! {
/// Question answer rows / #771: one clarification or one paid continuation.
/// K5 value form of on_answer's work: the node and next question id come in by
/// value; the answered node, the next id and at most one notice are returned.
pub(super) fn answer_node(n: Node, nq: u64, id: u64, answer: Answer)
    -> (result: (Node, u64, Option<Notice>)) [
    requires node_headroom(n),
        arithmetic_consistent(n),
        nq < u64::MAX,
    ensures (handler_safety::node_ok(n, nq)
            && answer_progress_input(n, id, answer)
            ==> distance(result.0) < distance(n)),
        (n.phase is Closed ==> result.0.phase is Closed),
        result.0.repo == n.repo,
        result.0.footprint == n.footprint,
        (settle_shape(n) ==> settle_shape(result.0)),
        result.0.generation as int <= n.generation as int + 1,
        result.0.next_seq == n.next_seq,
        result.0.credit.start == n.credit.start,
        result.0.credit.earned == n.credit.earned,
        result.0.credit.added as int <= n.credit.added as int + 255,
        result.0.credit.spent as int <= n.credit.spent as int + 1,
        result.0.last_cmd == n.last_cmd,
        result.1 as int <= nq as int + 1,
        result.0.proposed == n.proposed,
        result.0.adopted == n.adopted,
        (n.phase is Closed ==> result.0.phase == n.phase),
        handler_safety::command_step(n, result.0),
        (handler_safety::node_ok(n, nq) && result.0 != n ==> (match n.question {
            Some(q) => q.id == id,
            None => false,
        }) && (match result.0.question {
            Some(q) => q.id != id,
            None => true,
        })),

        (result.0.holds ==> n.holds),
        (result.0.pool ==> n.pool),
        result.0.ticket == n.ticket || result.0.ticket.is_none(),
        result.0.credit.withheld == n.credit.withheld,
        nq <= result.1,
        (handler_safety::node_rules_v(n, nq) ==> handler_safety::node_rules_v(result.0, result.1)),
] {
    let result = answer_value(n, nq, id, answer);
    kernel_proof! {
        apply_answer_safety::answer_frame(n, nq, id, answer);
        apply_answer_safety::answer_resources(n, nq, id, answer);
        apply_answer_safety::answer_question_order(n, nq, id, answer);
        apply_answer_safety::answer_rules(n, nq, id, answer);
        apply_answer_safety::answer_step(n, nq, id, answer);
        apply_answer_safety::answer_progress(n, nq, id, answer);
        apply_answer_safety::answer_consumed(n, nq, id, answer);
    }
    result
}
}

#[cfg(verus_keep_ghost)]
verus! {
/// TryAgain / #771: the first attempt restarts before the continuation.
pub(super) open spec fn try_again_node(n: Node) -> Node {
    Node { attempt: 1, review_restarted: false, round: 0, replanned: false, ..n }
}
}

kernel_fn! {
/// Clarification rows / #771: one clarifying question under a fresh id, as a
/// value function (K5): the node, the next id and the Ask notice are returned.
fn clarify_node(n: Node, nq: u64, q: Question) -> (result: (Node, u64, Notice)) [
    requires nq < u64::MAX,
    ensures result.0 == (Node {
            question: Some(Question { id: nq, clarified: true, ..q }), ..n
        }),
        result.1 == nq + 1,
        result.2 == (Notice::Ask { question: nq }),
        n.phase == Phase::AwaitingRob && n.question == Some(q) && !q.clarified
            ==> handler_safety::command_step(n, result.0),

        (result.0.holds ==> n.holds),
        (result.0.pool ==> n.pool),
        result.0.ticket == n.ticket || result.0.ticket.is_none(),
        result.0.credit.withheld == n.credit.withheld,
        (handler_safety::node_rules_v(n, nq) && n.phase == Phase::AwaitingRob && n.question == Some(q)
            ==> handler_safety::node_rules_v(result.0, result.1)),
] {
    let mut m = n;
    let next = nq;
    m.question = Some(Question {
        id: next,
        clarified: true,
        ..q
    });
    kernel_proof! {
        if n.phase == Phase::AwaitingRob && n.question == Some(q) && !q.clarified {
            handler_safety::clarify_step(n, q, next);
        }
    }
    kernel_proof! {
        if handler_safety::node_rules_v(n, nq) && n.phase == Phase::AwaitingRob && n.question == Some(q) {
            handler_safety::clarified_rules_v(n, nq, q);
        }
    }

    (m, next + 1, Notice::Ask { question: next })
}
}

kernel_fn! {
/// TryAgain rows / #771: restart the first attempt, then continue with at least
/// one credit, as a value function (K5); the continuation is continue_node, then
/// go_on's replanned note (never due here: replanned was just cleared).
fn retry_answer_node(n: Node, q: Question) -> (result: (Node, Option<Notice>)) [
    requires q.cost <= 1,
        n.generation < u64::MAX,
        n.credit.start as int + n.credit.earned as int + n.credit.added as int
            >= n.credit.spent as int,
        n.credit.withheld ==> (if q.cost > 1 { q.cost } else { 1u8 }) == 0,
        n.credit.added as int + 255 <= u64::MAX as int,
        n.credit.spent < u64::MAX,
    ensures ({
            let b = try_again_node(n);
            let q1 = Question { cost: if q.cost > 1 { q.cost } else { 1u8 }, ..q };
            (b.phase == Phase::AwaitingRob && b.question == Some(q1) && !b.paused
                && !b.waiting_incident && b.effect.is_none()
                ==> distance(result.0) < distance(b))
            && result.0.repo == b.repo && result.0.footprint == b.footprint
            && (settle_shape(b) ==> settle_shape(result.0))
            && result.0.generation as int <= b.generation as int + 1
            && result.0.next_seq == b.next_seq
            && result.0.credit.start == b.credit.start && result.0.credit.earned == b.credit.earned
            && result.0.credit.added as int <= b.credit.added as int + 255
            && result.0.credit.spent as int <= b.credit.spent as int + 1
            && result.0.last_cmd == b.last_cmd
            && result.0.question.is_none()
            && result.0.proposed == b.proposed && result.0.adopted == b.adopted
            && result.0.review_stop == b.review_stop && result.0.attempt == 1
            && !result.0.replanned
        }),
        n.phase == Phase::AwaitingRob ==> handler_safety::command_step(n, result.0),
        result.1.is_none(),

        (result.0.holds ==> n.holds),
        (result.0.pool ==> n.pool),
        result.0.ticket == n.ticket || result.0.ticket.is_none(),
        result.0.credit.withheld == n.credit.withheld,
        (handler_safety::node_rules_v(n, u64::MAX) && n.phase == Phase::AwaitingRob && n.question == Some(q) && q.kind == QKind::CantMeet ==> handler_safety::node_rules_v(result.0, u64::MAX)),
] {
    let r = retry_answer_exact(n, q);
    kernel_proof! {
        apply_answer_safety::retry_frame(n, q);
        apply_answer_safety::retry_progress(n, q);
        apply_answer_safety::retry_resources(n, q);
        apply_answer_safety::retry_step(n, q);
        if handler_safety::node_rules_v(n, u64::MAX) && n.phase == Phase::AwaitingRob
            && n.question == Some(q) && q.kind == QKind::CantMeet {
            apply_answer_safety::retried_answer_rules(n, q, u64::MAX);
        }
    }
    r
}
}

kernel_fn! {
fn retry_answer_exact(n: Node, q: Question) -> (result: (Node, Option<Notice>)) [
    requires q.cost <= 1,
        n.generation < u64::MAX,
        n.credit.start as int + n.credit.earned as int + n.credit.added as int
            >= n.credit.spent as int,
        n.credit.withheld ==> (if q.cost > 1 { q.cost } else { 1u8 }) == 0,
        n.credit.added as int + 255 <= u64::MAX as int,
        n.credit.spent < u64::MAX,
    ensures result == apply_answer_values::retried_answer(n, q),
] {
    let mut m = n;
    m.attempt = 1;
    m.review_restarted = false;
    m.round = 0;
    m.replanned = false;

    m = continue_exact(
        m,
        Question {
            cost: retry_cost(q.cost),
            ..q
        },
        0,
    );
    let mut note: Option<Notice> = None;
    match q.cont {
        Cont::Planning { must_split } => {
            if !must_split && m.replanned && q.kind == QKind::OutOfCredit {
                note = Some(Notice::Note(NoteKind::Replanned));
            }
        }
        _ => {}
    }

    kernel_proof! { reveal(apply_answer_values::retried_answer); reveal(apply_answer_values::continued_answer); }
    (m, note)
}
}

kernel_fn! {
pub(super) fn answer_value(n: Node, nq: u64, id: u64, answer: Answer)
    -> (result: (Node, u64, Option<Notice>)) [
    requires node_headroom(n),
        arithmetic_consistent(n),
        nq < u64::MAX,
    ensures result == apply_answer_values::answered(n, nq, id, answer),
] {
    let m = n;
    let nq = nq;
    let note: Option<Notice> = None;
    let Some(q) = m.question else {
        kernel_proof! { reveal(apply_answer_values::answered); }
        return (m, nq, note);
    };
    if q.id != id {
        kernel_proof! { reveal(apply_answer_values::answered); }
        return (m, nq, note); // an answer to a question that is no longer open
    }
    match m.phase {
        Phase::Stopping(_) => cleanup_answer_v(m, nq, id, answer),
        Phase::AwaitingRob => awaiting_answer_v(m, nq, id, answer, q),
        _ => {
            kernel_proof! { reveal(apply_answer_values::answered); }
            (m, nq, note)
        }
    }
}
}

kernel_fn! {
/// The unchanged TryAgain cost floor `q.cost.max(1)`, as a value with an exact post.
fn retry_cost(c: u8) -> (result: u8) [
    ensures result == (if c > 1 { c } else { 1u8 }),
] {
    c.max(1)
}
}

kernel_fn! {
/// MarkHandled on a cleanup question: the unchanged two statements as one value
/// step, so the dispatcher sees one equation and not mark_handled's borrow.
fn cleanup_handled_v(n: Node) -> (result: Node) [
    ensures result == handler_safety::handled_node(Node { question: None, ..n }),
] {
    let mut m = n;
    m.question = None;
    mark_handled(&mut m);
    m
}
}

kernel_fn! {
/// Cleanup-question answers (answer_value's Stopping arm, unchanged statements).
fn cleanup_answer_v(n: Node, nq: u64, id: u64, answer: Answer)
    -> (result: (Node, u64, Option<Notice>)) [
    requires n.phase is Stopping,
        match n.question { Some(q) => q.id == id, None => false },
    ensures result == apply_answer_values::answered(n, nq, id, answer),
] {
    let mut m = n;
    let note: Option<Notice> = None;
    match answer {
        Answer::Retry => {
            m.question = None;
        }
        Answer::MarkHandled => {
            m = cleanup_handled_v(m);
        }
        _ => {}
    }
    kernel_proof! { reveal(apply_answer_values::answered); }
    (m, nq, note)
}
}

kernel_fn! {
/// Rob's answers to an open question (answer_value's AwaitingRob arm, unchanged
/// statements, with the arm's own early returns).
fn awaiting_answer_v(n: Node, nq: u64, id: u64, answer: Answer, q: Question)
    -> (result: (Node, u64, Option<Notice>)) [
    requires node_headroom(n),
        arithmetic_consistent(n),
        nq < u64::MAX,
        n.phase == Phase::AwaitingRob, n.question == Some(q), q.id == id,
    ensures result == apply_answer_values::answered(n, nq, id, answer),
] {
    kernel_proof! { apply_answer_safety::awaiting_arms_bridge(n, nq, id, answer, q); }
    if answer == Answer::Unclear {
        return awaiting_unclear_v(n, nq, id, q);
    }
    if !allowed(q.kind, answer) {
        kernel_proof! { assert(!apply_answer_values::permits(q.kind, answer)); }
        return awaiting_denied_v(n, nq, id, q);
    }
    kernel_proof! { assert(apply_answer_values::permits(q.kind, answer)); }
    match answer {
        Answer::Continue => awaiting_continue_v(n, nq, id, q),
        Answer::TryAgain => awaiting_retry_v(n, nq, id, q),
        Answer::AddCredit(k) => awaiting_credit_v(n, nq, id, q, k),
        Answer::ChangeRequest => awaiting_replace_v(n, nq, id, q),
        Answer::Stop => awaiting_stop_v(n, nq, id, q),
        _ => awaiting_other_v(n, nq, id, q),
    }
}
}

kernel_fn! {
fn awaiting_unclear_v(n: Node, nq: u64, id: u64, q: Question)
    -> (result: (Node, u64, Option<Notice>)) [
    requires node_headroom(n), arithmetic_consistent(n), nq < u64::MAX,
        n.phase == Phase::AwaitingRob, n.question == Some(q), q.id == id,
    ensures result == (apply_answer_values::unclear_answer(n, nq, q)),
] {
    let mut m = n;
    let mut nq = nq;
    let mut note: Option<Notice> = None;
    if !q.clarified {
        let (m1, nq1, x) = clarify_node(m, nq, q);
        m = m1;
        nq = nq1;
        note = Some(x);
    }
    (m, nq, note)
}
}

kernel_fn! {
fn awaiting_denied_v(n: Node, nq: u64, id: u64, q: Question)
    -> (result: (Node, u64, Option<Notice>)) [
    requires node_headroom(n), arithmetic_consistent(n), nq < u64::MAX,
        n.phase == Phase::AwaitingRob, n.question == Some(q), q.id == id,
    ensures result == ((n, nq, None)),
] {
    let mut m = n;
    let mut nq = nq;
    let mut note: Option<Notice> = None;

    (m, nq, note)
}
}

kernel_fn! {
fn awaiting_continue_v(n: Node, nq: u64, id: u64, q: Question)
    -> (result: (Node, u64, Option<Notice>)) [
    requires node_headroom(n), arithmetic_consistent(n), nq < u64::MAX,
        n.phase == Phase::AwaitingRob, n.question == Some(q), q.id == id,
        apply_answer_values::permits(q.kind, Answer::Continue),
    ensures result == ({ let r = apply_answer_values::continued_answer(n, q, 0); (r.0, nq, r.1) }),
] {
    let mut m = n;
    let mut nq = nq;
    let mut note: Option<Notice> = None;
    let (m1, x) = go_on_exact(m, q, 0);
    m = m1;
    note = x;
    (m, nq, note)
}
}

kernel_fn! {
fn awaiting_retry_v(n: Node, nq: u64, id: u64, q: Question)
    -> (result: (Node, u64, Option<Notice>)) [
    requires node_headroom(n), arithmetic_consistent(n), nq < u64::MAX,
        n.phase == Phase::AwaitingRob, n.question == Some(q), q.id == id,
        apply_answer_values::permits(q.kind, Answer::TryAgain),
    ensures result == ({ let r = apply_answer_values::retried_answer(n, q); (r.0, nq, r.1) }),
] {
    let mut m = n;
    let mut nq = nq;
    let mut note: Option<Notice> = None;
    let (m1, x) = retry_answer_exact(m, q);
    m = m1;
    note = x;
    (m, nq, note)
}
}

kernel_fn! {
fn awaiting_credit_v(n: Node, nq: u64, id: u64, q: Question, k: u8)
    -> (result: (Node, u64, Option<Notice>)) [
    requires node_headroom(n), arithmetic_consistent(n), nq < u64::MAX,
        n.phase == Phase::AwaitingRob, n.question == Some(q), q.id == id,
        apply_answer_values::permits(q.kind, Answer::AddCredit(k)),
    ensures result == ({ let r = apply_answer_values::continued_answer(n, q, k); (r.0, nq, r.1) }),
] {
    let mut m = n;
    let mut nq = nq;
    let mut note: Option<Notice> = None;
    let (m1, x) = go_on_exact(m, q, k);
    m = m1;
    note = x;
    (m, nq, note)
}
}

kernel_fn! {
fn awaiting_replace_v(n: Node, nq: u64, id: u64, q: Question)
    -> (result: (Node, u64, Option<Notice>)) [
    requires node_headroom(n), arithmetic_consistent(n), nq < u64::MAX,
        n.phase == Phase::AwaitingRob, n.question == Some(q), q.id == id,
    ensures result == ((retirement::stopped(n, Outcome::Replaced), nq, None)),
] {
    let mut m = n;
    let mut nq = nq;
    let mut note: Option<Notice> = None;
    m = stopped_v(m, Outcome::Replaced);
    (m, nq, note)
}
}

kernel_fn! {
fn awaiting_stop_v(n: Node, nq: u64, id: u64, q: Question)
    -> (result: (Node, u64, Option<Notice>)) [
    requires node_headroom(n), arithmetic_consistent(n), nq < u64::MAX,
        n.phase == Phase::AwaitingRob, n.question == Some(q), q.id == id,
    ensures result == ((retirement::stopped(n, Outcome::Stopped), nq, None)),
] {
    let mut m = n;
    let mut nq = nq;
    let mut note: Option<Notice> = None;
    m = stopped_v(m, Outcome::Stopped);
    (m, nq, note)
}
}

kernel_fn! {
fn awaiting_other_v(n: Node, nq: u64, id: u64, q: Question)
    -> (result: (Node, u64, Option<Notice>)) [
    requires node_headroom(n), arithmetic_consistent(n), nq < u64::MAX,
        n.phase == Phase::AwaitingRob, n.question == Some(q), q.id == id,
    ensures result == ((n, nq, None)),
] {
    let mut m = n;
    let mut nq = nq;
    let mut note: Option<Notice> = None;

    (m, nq, note)
}
}
