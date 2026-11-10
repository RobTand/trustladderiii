//! K4b #910: node rows every command keeps, as one opaque post proved per
//! arm shape. Handlers ensure only `command_step`; these small lemmas carry
//! the case analysis so no handler reveals the state-table updates.
use super::*;

verus! {

/// check_nodes identity rows and the Q54 check_fail_fast node rows that hold
/// for every command and answer.
#[verifier::opaque]
pub(super) open spec fn command_step(b: Node, n: Node) -> bool {
    n.proposed == b.proposed && n.adopted == b.adopted
        && (b.phase is Closed ==> n.phase == b.phase)
        && n.review_stop == b.review_stop
        && n.attempt <= (if b.attempt > 1 { b.attempt } else { 1u8 })
        && (b.replanned || !n.replanned)
        && (n.phase is Planning ==> b.phase is Planning || b.phase == Phase::AwaitingRob)
        && !transition_safety::asks_cant_meet(b, n)
        && n.credit.merge_credited == b.credit.merge_credited
        && n.credit.withheld == b.credit.withheld
}

/// Stop, ChangeRequest and stopping continuations: the state-table stop update.
pub(super) proof fn stop_step(b: Node, o: Outcome)
    ensures command_step(b, retirement::stopped(b, o)),
{
    reveal(command_step); reveal(retirement::stopped);
}

/// Pause, Resume, Retry and MarkHandled: no tracked row changes and any
/// question is kept or cleared.
pub(super) proof fn field_step(b: Node, n: Node)
    requires n.proposed == b.proposed, n.adopted == b.adopted, n.phase == b.phase,
        n.review_stop == b.review_stop, n.attempt == b.attempt, n.replanned == b.replanned,
        n.question.is_none() || n.question == b.question,
        n.credit.merge_credited == b.credit.merge_credited, n.credit.withheld == b.credit.withheld,
    ensures command_step(b, n),
{
    reveal(command_step);
}

/// Rob's unclear answer re-asks his open question once, under a new id.
pub(super) proof fn clarify_step(b: Node, q: Question, id: u64)
    requires b.phase == Phase::AwaitingRob, b.question == Some(q), !q.clarified,
    ensures command_step(b, Node { question: Some(Question { id, clarified: true, ..q }), ..b }),
{
    reveal(command_step);
}

/// Continue, TryAgain and AddCredit: a continuation from AwaitingRob closes the
/// question and may restart the first attempt, but never re-plans.
pub(super) proof fn continue_step(b: Node, n: Node)
    requires b.phase == Phase::AwaitingRob,
        n.proposed == b.proposed, n.adopted == b.adopted, n.review_stop == b.review_stop,
        n.attempt <= (if b.attempt > 1 { b.attempt } else { 1u8 }),
        b.replanned || !n.replanned, n.question.is_none(),
        n.credit.merge_credited == b.credit.merge_credited, n.credit.withheld == b.credit.withheld,
    ensures command_step(b, n),
{
    reveal(command_step);
}

/// A command or answer that changes nothing keeps every row.
pub(super) proof fn same_step(b: Node)
    ensures command_step(b, b),
{
    reveal(command_step);
}

/// The three check_nodes identity rows of command_step, for callers that
/// state them directly.
pub(super) proof fn step_identity(b: Node, n: Node)
    requires command_step(b, n),
    ensures n.proposed == b.proposed, n.adopted == b.adopted,
        b.phase is Closed ==> n.phase == b.phase,
{
    reveal(command_step);
}

/// on_command records the command id before dispatching; the rows ignore it.
pub(super) proof fn last_cmd_step(b: Node, m: Node, n: Node)
    requires m == (Node { last_cmd: m.last_cmd, ..b }), command_step(m, n),
    ensures command_step(b, n),
{
    reveal(command_step);
}


/// Q54 rows of one retry-ladder step (fail_fast_rows (1)-(3) at the node):
/// a step that is not a retry keeps the review stop; with the flag a retry
/// stops for TL3 review, keeping credit and emitting nothing; without it the
/// work is retried.
#[verifier::opaque]
pub(super) open spec fn ladder(fast: bool, retry: bool, b: Node, n: Node, ob: Outbox, oa: Outbox) -> bool {
    if !retry {
        n.review_stop == b.review_stop
    } else if fast {
        n.review_stop.is_some() && n.credit == b.credit && oa.len == ob.len
            && n.phase == Phase::Stopping(Outcome::Stopped) && n.question.is_none()
    } else {
        transition_safety::retried(b, n) && n.review_stop == b.review_stop
    }
}

/// The fail_fast branch: stop_for_review's exact result.
pub(super) proof fn stop_ladder(b: Node, step: ReviewStop, ob: Outbox)
    requires !(b.phase is Closed || b.phase is Stopping),
    ensures ladder(true, true, b,
        retirement::stopped(Node { review_stop: Some(step), ..b }, Outcome::Stopped), ob, ob),
{
    reveal(ladder); reveal(retirement::stopped);
}

/// A flag-off retry.
pub(super) proof fn retry_ladder(b: Node, n: Node, ob: Outbox, oa: Outbox)
    requires transition_safety::retried(b, n), n.review_stop == b.review_stop,
    ensures ladder(false, true, b, n, ob, oa),
{
    reveal(ladder);
}

/// A step that is not a retry.
pub(super) proof fn same_ladder(fast: bool, b: Node, n: Node, ob: Outbox, oa: Outbox)
    requires n.review_stop == b.review_stop,
    ensures ladder(fast, false, b, n, ob, oa),
{
    reveal(ladder);
}

/// A ladder row from an intermediate node that differs from b only in fields
/// the rows do not read.
pub(super) proof fn ladder_rebase(fast: bool, retry: bool, b: Node, m: Node, n: Node,
    ob: Outbox, oa: Outbox)
    requires ladder(fast, retry, m, n, ob, oa), m.credit == b.credit,
        m.review_stop == b.review_stop, m.attempt == b.attempt,
    ensures ladder(fast, retry, b, n, ob, oa),
{
    reveal(ladder);
}


/// The seven fail-fast reply kinds of check_fail_fast_diverts.
pub(super) open spec fn fail_fast_reply(reply: Reply) -> bool {
    matches!(reply, Reply::PlanCantMeet | Reply::MakerCantMeet | Reply::MakerOutOfRoom
        | Reply::MakerNeedsFiles { .. } | Reply::ChecksDone { .. } | Reply::ChangesRequested
        | Reply::ReviewerOutOfRoom)
}

/// reply_work's retry rows, for an admitted reply to node b.
pub(super) open spec fn work_retry(b: Node, reply: Reply) -> bool {
    match reply {
        Reply::MakerCantMeet | Reply::MakerOutOfRoom => true,
        Reply::ChecksDone { pass, in_footprint, .. } => !(pass && in_footprint) && b.round != 0,
        Reply::ChangesRequested => b.round != 0,
        Reply::ReviewerOutOfRoom => b.review_restarted,
        _ => false,
    }
}

/// check_fail_fast_funding row (4): an unfunded fitting extension keeps its
/// credit and asks Rob; with the flag it continues the maker, without it it
/// asks for a split.
#[verifier::opaque]
pub(super) open spec fn funding(fast: bool, b: Node, n: Node) -> bool {
    n.credit == b.credit && n.review_stop == b.review_stop
        && (fast ==> n.phase == Phase::AwaitingRob && match n.question {
            Some(q) => q.kind == QKind::OutOfCredit && q.cont == Cont::Ready(Resume::Making),
            None => false })
        && (!fast ==> match n.question {
            Some(q) => q.kind == QKind::OutOfCredit && q.cont == Cont::Planning { must_split: true },
            None => false })
}

/// reply_files' Q54 rows: a non-extension goes out of room, a funded extension
/// keeps the review stop, an unfunded one takes the funding split.
#[verifier::opaque]
pub(super) open spec fn files_post(fast: bool, files: Footprint, fits: bool, b: Node, n: Node,
    ob: Outbox, oa: Outbox) -> bool {
    let ext = fits && !footprint::footprint_empty(files);
    (!ext ==> ladder(fast, true, b, n, ob, oa))
        && (ext && transition_safety::can_spend_one(b) ==> n.review_stop == b.review_stop)
        && (ext && !transition_safety::can_spend_one(b) ==> funding(fast, b, n))
}

/// The flag-on funding branch.
pub(super) proof fn fund_fast(b: Node, n: Node)
    requires n.credit == b.credit, n.review_stop == b.review_stop,
        n.phase == Phase::AwaitingRob,
        match n.question {
            Some(q) => q.kind == QKind::OutOfCredit && q.cont == Cont::Ready(Resume::Making),
            None => false },
    ensures funding(true, b, n),
{
    reveal(funding);
}

/// The flag-off funding branch (out of room without credit).
pub(super) proof fn fund_off(b: Node, n: Node)
    requires n.credit == b.credit, n.review_stop == b.review_stop,
        match n.question {
            Some(q) => q.kind == QKind::OutOfCredit && q.cont == Cont::Planning { must_split: true },
            None => false },
    ensures funding(false, b, n),
{
    reveal(funding);
}

/// reply_files' three branches.
pub(super) proof fn files_rows(fast: bool, files: Footprint, fits: bool, b: Node, n: Node,
    ob: Outbox, oa: Outbox)
    requires
        !(fits && !footprint::footprint_empty(files)) ==> ladder(fast, true, b, n, ob, oa),
        fits && !footprint::footprint_empty(files) && transition_safety::can_spend_one(b)
            ==> n.review_stop == b.review_stop,
        fits && !footprint::footprint_empty(files) && !transition_safety::can_spend_one(b)
            ==> funding(fast, b, n),
    ensures files_post(fast, files, fits, b, n, ob, oa),
{
    reveal(files_post);
}


/// The phase that admits each fail-fast reply kind.
pub(super) open spec fn fail_fast_admitted(b: Node, reply: Reply) -> bool {
    match reply {
        Reply::PlanCantMeet => b.phase is Planning,
        Reply::MakerCantMeet | Reply::MakerOutOfRoom | Reply::MakerNeedsFiles { .. } =>
            b.phase == Phase::Making,
        Reply::ChecksDone { .. } => b.phase == Phase::Checking,
        Reply::ChangesRequested | Reply::ReviewerOutOfRoom => b.phase == Phase::Reviewing,
        _ => false,
    }
}

/// The retry row of an admitted fail-fast reply.
pub(super) open spec fn admitted_retry(b: Node, reply: Reply) -> bool {
    match reply {
        Reply::PlanCantMeet => b.phase is Planning,
        Reply::MakerNeedsFiles { files, fits } => !(fits && !footprint::footprint_empty(files)),
        _ => work_retry(b, reply),
    }
}

/// The funding row of an admitted fail-fast reply.
pub(super) open spec fn admitted_fund(b: Node, reply: Reply) -> bool {
    match reply {
        Reply::MakerNeedsFiles { files, fits } => fits && !footprint::footprint_empty(files)
            && !transition_safety::can_spend_one(b),
        _ => false,
    }
}

/// One reply's Q54 rows at its node: the ladder row plus the funding split.
#[verifier::opaque]
pub(super) open spec fn reply_rows(fast: bool, retry: bool, fund: bool, b: Node, n: Node,
    ob: Outbox, oa: Outbox) -> bool {
    ladder(fast, retry, b, n, ob, oa) && (fund ==> funding(fast, b, n))
}

/// end_operation changes no field the rows read.
pub(super) open spec fn rows_frame(b: Node, m: Node) -> bool {
    m.credit == b.credit && m.review_stop == b.review_stop && m.attempt == b.attempt
        && m.round == b.round && m.review_restarted == b.review_restarted && m.phase == b.phase
}

/// on_admitted_reply, MakerNeedsFiles: reply_files' rows after end_operation.
pub(super) proof fn files_admitted(fast: bool, files: Footprint, fits: bool, b: Node, m: Node,
    n: Node, ob: Outbox, oa: Outbox)
    requires rows_frame(b, m), files_post(fast, files, fits, m, n, ob, oa),
    ensures reply_rows(fast, admitted_retry(b, Reply::MakerNeedsFiles { files, fits }),
        admitted_fund(b, Reply::MakerNeedsFiles { files, fits }), b, n, ob, oa),
{
    reveal(reply_rows); reveal(files_post); reveal(ladder); reveal(funding);
}

/// on_admitted_reply, other fail-fast replies: reply_transition's row after end_operation.
pub(super) proof fn transition_admitted(fast: bool, reply: Reply, b: Node, m: Node, n: Node,
    ob: Outbox, oa: Outbox)
    requires rows_frame(b, m), !(reply is MakerNeedsFiles), fail_fast_admitted(b, reply),
        ladder(fast, admitted_retry(m, reply), m, n, ob, oa),
    ensures reply_rows(fast, admitted_retry(b, reply), admitted_fund(b, reply), b, n, ob, oa),
{
    reveal(reply_rows); reveal(ladder);
}

/// on_reply's stopping, stale and rejected paths: no row is due and the review
/// stop is kept.
pub(super) proof fn quiet_due(w: World, n: Node, i: usize, g: u64, reply: Reply, ob: Outbox, oa: Outbox)
    requires i < w.nodes@.len(), n.review_stop == w.nodes@[i as int].review_stop,
        w.nodes@[i as int].phase is Stopping || !transition_safety::reply_owner(w.nodes@[i as int], g)
            || !port_t860_contracts::admissible(w, w.nodes@[i as int], reply),
    ensures reply_rows(w.fail_fast,
        transition_safety::retry_due(w, Event::Reply { node: NodeId(i as u64), generation: g, reply }),
        transition_safety::funding_due(w, Event::Reply { node: NodeId(i as u64), generation: g, reply }),
        w.nodes@[i as int], n, ob, oa),
{
    reveal(reply_rows); reveal(ladder); reveal(port_t860_contracts::admissible);
}

/// on_reply's admitted path: the node-level rows are the retry_due/funding_due rows.
pub(super) proof fn admitted_due(w: World, n: Node, i: usize, g: u64, reply: Reply, ob: Outbox, oa: Outbox)
    requires i < w.nodes@.len(), !(w.nodes@[i as int].phase is Stopping),
        transition_safety::reply_owner(w.nodes@[i as int], g),
        port_t860_contracts::admissible(w, w.nodes@[i as int], reply), fail_fast_reply(reply),
        fail_fast_admitted(w.nodes@[i as int], reply) ==> reply_rows(w.fail_fast,
            admitted_retry(w.nodes@[i as int], reply), admitted_fund(w.nodes@[i as int], reply),
            w.nodes@[i as int], n, ob, oa),
    ensures reply_rows(w.fail_fast,
        transition_safety::retry_due(w, Event::Reply { node: NodeId(i as u64), generation: g, reply }),
        transition_safety::funding_due(w, Event::Reply { node: NodeId(i as u64), generation: g, reply }),
        w.nodes@[i as int], n, ob, oa),
{
    reveal(port_t860_contracts::admissible);
}


/// Compose's fail_fast_rows for a fail-fast reply, from on_reply's node rows
/// and Kani's consistent-input premise (the checker's node rule for b).
pub(super) proof fn fail_fast_reply_rows(before: World, after: World, node: NodeId, g: u64, reply: Reply,
    ob: Outbox, oa: Outbox)
    requires node.0 < before.nodes@.len(), node.0 < after.nodes@.len(), ob.len <= OUTBOX,
        retirement::node_rules(before, before.nodes@[node.0 as int]),
        reply_rows(before.fail_fast,
            transition_safety::retry_due(before, Event::Reply { node, generation: g, reply }),
            transition_safety::funding_due(before, Event::Reply { node, generation: g, reply }),
            before.nodes@[node.0 as int], after.nodes@[node.0 as int], ob, oa),
    ensures transition_safety::fail_fast_rows(before, after,
        Event::Reply { node, generation: g, reply }, node.0 as int, transition_safety::emitted(ob, oa)),
{
    reveal(reply_rows); reveal(ladder); reveal(funding); reveal(retirement::node_rules);
    assert(ob.items@.len() == OUTBOX && oa.items@.len() == OUTBOX);
    if oa.len == ob.len {
        assert(transition_safety::emitted(ob, oa).len() == 0);
    }
}


/// The failed-run update keeps the review stop (port_t860 failed_post).
pub(super) proof fn failed_review_stop(b: Node, a: Node, bo: Outbox, ao: Outbox)
    requires port_t860_contracts::failed_post(b, a, bo, ao),
    ensures a.review_stop == b.review_stop,
{
    reveal(port_t860_contracts::failed_post); reveal(port_t860_contracts::failed_node);
    reveal(retirement::incident);
}


/// next_rung's plain rows (design (3)): the caller forms the ladder row.
pub(super) proof fn rung_ladder(fast: bool, b: Node, n: Node, ob: Outbox, oa: Outbox)
    requires
        fast ==> n.review_stop.is_some() && n.credit == b.credit && oa.len == ob.len
            && n.phase == Phase::Stopping(Outcome::Stopped) && n.question.is_none(),
        !fast ==> transition_safety::retried(b, n) && n.review_stop == b.review_stop,
    ensures ladder(fast, true, b, n, ob, oa),
{
    reveal(ladder);
}


/// recheck keeps the review stop; opaque so that recheck's other callers do
/// not reason about it.
#[verifier::opaque]
pub(super) open spec fn keeps_review_stop(b: Node, n: Node) -> bool {
    n.review_stop == b.review_stop
}

/// Establish keeps_review_stop.
pub(super) proof fn keep_review_stop(b: Node, n: Node)
    requires n.review_stop == b.review_stop,
    ensures keeps_review_stop(b, n),
{
    reveal(keeps_review_stop);
}

/// reply_work's non-retry recheck branch: the review stop is kept from the
/// node before review_restarted was set.
pub(super) proof fn kept_ladder(fast: bool, b: Node, m: Node, n: Node, ob: Outbox, oa: Outbox)
    requires keeps_review_stop(m, n), m.review_stop == b.review_stop,
    ensures ladder(fast, false, b, n, ob, oa),
{
    reveal(keeps_review_stop); reveal(ladder);
}


/// K5 gap 2a / G8: node_checked's node conjuncts for the value-form command
/// handlers. Copied verbatim from lib.rs node_checked, except: the repo and
/// footprint conjuncts (world tables) are dropped, `w.next_question` is the
/// parameter `nq`, and the `t < w.next_ticket` atom is dropped. Implied by
/// node_checked (checked_node_ok).
pub(super) open spec fn node_ok(n: Node, nq: u64) -> bool {
    arithmetic_consistent(n) && settle_shape(n) && stopping_effect_fits(n)
        && (n.phase is Closed ==> n.pos == 0 && n.effect.is_none() && n.run.is_none()
            && n.job.is_none() && !n.pool && !n.holds && n.ticket.is_none()
            && n.question.is_none() && !n.waiting_incident && !n.pr_open && !n.worktree)
        && (!(n.phase is Closed) ==> n.pos <= items(n.phase).len()
            && (n.pos == items(n.phase).len() ==> n.phase is Stopping))
        && (holding(n.phase) ==> n.holds)
        && (n.holds ==> holding(n.phase) || (n.phase is Stopping && n.pos <= RELEASE_STEP))
        && (n.phase == Phase::AwaitingDelivery ==> n.pr_open && n.effect.is_none())
        && (n.pr_open ==> n.had_pr)
        && (n.merged.is_some() ==> n.phase == Phase::Stopping(Outcome::Merged)
            || n.phase == Phase::Closed(Outcome::Merged))
        && (n.pool ==> n.run.is_some() && n.phase != Phase::Rebasing)
        && (match n.run {
            Some(g) => (current_item(n) == Some(Item::Await) && g == n.generation
                && matches!(n.phase, Phase::Ingesting | Phase::Auditing | Phase::Planning { .. }
                    | Phase::Making | Phase::Reviewing | Phase::Rebasing))
                || (n.phase is Stopping && n.pos == 0),
            None => true,
        })
        && (match n.job {
            Some(g) => (n.phase == Phase::Checking && current_item(n) == Some(Item::Await) && g == n.generation)
                || (n.phase is Stopping && n.pos <= 1),
            None => true,
        })
        && (current_item(n) == Some(Item::Await) ==> if n.phase == Phase::Checking {
            n.job == Some(n.generation)
        } else { n.run == Some(n.generation) })
        && (match n.effect {
            None => true,
            Some(e) => e.seq < n.next_seq && (e.cancelling || if
                matches!(e.kind, EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase) {
                current_item(n) == Some(Item::Await)
            } else { current_item(n) == Some(Item::Do(e.kind)) || current_item(n) == Some(Item::Teardown(e.kind)) }),
        })
        && (match n.ticket { None => true, Some(t) => !n.waiting_incident && n.effect.is_none() && !n.pool
            && matches!(current_item(n), Some(Item::Slot) | Some(Item::Start(_))) })
        && (n.waiting_incident ==> !(n.phase is Closed) && !holding(n.phase)
            && n.ticket.is_none() && (match n.effect { None => true, Some(e) => e.cancelling })
            && (!(n.phase is Stopping) ==> n.run.is_none() && n.job.is_none() && !n.pool))
        && (match n.question {
            None => n.phase != Phase::AwaitingRob,
            Some(q) => q.id < nq && (match n.effect { None => true, Some(e) => e.cancelling })
                && match n.phase {
                    Phase::AwaitingRob => question_fits(q) && n.run.is_none() && n.job.is_none() && !n.pool,
                    Phase::Stopping(_) => q.kind == QKind::CleanupFailed && matches!(current_item(n), Some(Item::Teardown(_))),
                    _ => false,
                },
        })
        && (match n.phase { Phase::Ready(r) => ready_checked(n, r), _ => true })
        && (match n.phase {
            Phase::Making => ready_checked(n, Resume::Making),
            Phase::Checking => ready_checked(n, Resume::Checking),
            Phase::Reviewing => ready_checked(n, Resume::Reviewing),
            Phase::Publishing | Phase::AwaitingDelivery => ready_checked(n, Resume::Publishing),
            Phase::Rebasing => ready_checked(n, Resume::Rebasing),
            _ => true,
        })
}

/// node_checked gives the value handlers' node premise.
pub(super) proof fn checked_node_ok(w: World, n: Node)
    requires node_checked(w, n),
    ensures node_ok(n, w.next_question),
{
}


/// K5 design 3 rung: the node distance does not read last_cmd, proved once
/// here so resume_command never unfolds distance.
pub(super) proof fn distance_last_cmd(n: Node, id: u64)
    ensures distance(Node { last_cmd: id, ..n }) == distance(n),
{
    reveal(distance);
}


/// G9 Command arm (frozen F_node_safety signature): on_command's existing
/// ensures give the frozen node contract at the command's node. R2: the
/// premise is exactly node_checked of the target node (the G8 Answer row needs
/// q.id < next_question); it is discharged by the accepted-world route (G1/G19).
pub(super) proof fn command_node_safety(before: World, after: World, event: Event, i: int,
    ob: Outbox, oa: Outbox)
    requires transition_safety::event_target(event) == Some(i), 0 <= i < before.nodes@.len(),
        event is Command,
        node_checked(before, before.nodes@[i]),
        command_step(before.nodes@[i], after.nodes@[i]),
        after.nodes@[i].next_seq == before.nodes@[i].next_seq,
        after.nodes@[i].credit.earned == before.nodes@[i].credit.earned,
        transition_safety::notice_only(ob, oa, NodeId(i as u64)),
        match event {
            Event::Command { command: Command::Answer { question, .. }, .. } =>
                node_checked(before, before.nodes@[i])
                    && after.nodes@[i] != (Node { last_cmd: after.nodes@[i].last_cmd, ..before.nodes@[i] })
                    ==> (match before.nodes@[i].question {
                        Some(q) => q.id == question,
                        None => false,
                    }) && (match after.nodes@[i].question {
                        Some(q) => q.id != question,
                        None => true,
                    }),
            Event::Command { command: Command::Stop | Command::Pause | Command::Resume
                | Command::ChangeRequest, .. }
                | Event::NewRequest { .. } | Event::NewAdoption { .. } | Event::Reply { .. }
                | Event::Ack { .. } | Event::Fact { .. } => true,
        },
    ensures transition_safety::node_safety(before.nodes@[i], after.nodes@[i], before.fail_fast,
        event, i, ob, oa),
{
    reveal(transition_safety::node_safety);
    reveal(command_step);
    let b = before.nodes@[i];
    let n = after.nodes@[i];
    assert forall|k: int| ob.len <= k < oa.len implies match #[trigger] oa.items@[k] {
        Some(Out::Effect { node, seq, kind, head }) => node.0 == i
            && transition_safety::effect_node_ok(b, n, seq, kind, head,
                transition_safety::apply_exempt(event) == Some(i)),
        Some(Out::Notice { .. }) | None => true,
    } by {
        assert(k == ob.len);
    }
}

}
