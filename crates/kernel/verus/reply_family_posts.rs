//! Derived reply leaf properties, proved independently of executable node updates.
use super::*;

verus! {
pub(super) proof fn room_arms(n: Node, fast: bool, step: ReviewStop)
    ensures reply_family_values::room(n, fast, step) == (
        if fast { reply_family_values::room_fast(n, step) }
        else if transition_safety::can_spend_one(n) {
            reply_family_values::room_paid(Node { holds: false, ..n })
        } else { reply_family_values::room_ask(Node { holds: false, ..n }) }),
        (fast ==> reply_family_values::room(n, fast, step)
            == reply_family_values::room_fast(n, step)),
        (!fast && transition_safety::can_spend_one(n) ==> reply_family_values::room(n, fast, step)
            == reply_family_values::room_paid(Node { holds: false, ..n })),
        (!fast && !transition_safety::can_spend_one(n) ==> reply_family_values::room(n, fast, step)
            == reply_family_values::room_ask(Node { holds: false, ..n })),
        (node_headroom(n) ==> reply_family_values::callable(reply_family_values::room(n, fast, step))),
{
    reveal(reply_family_values::room);
}

pub(super) proof fn failed_arms(n: Node)
    requires n.generation as int + 2 <= u64::MAX as int,
        n.run_failures <= 1, n.run_incidents <= 2,
    ensures reply_family_values::finish(0, reply_family_values::failed_change(n))
            == retries_facts::failed_run(n),
        reply_family_values::callable(reply_family_values::failed_change(n)),
        match reply_family_values::failed_change(n) {
            ReplyChange::Done { note, .. } => note.is_none(),
            ReplyChange::Ask { .. } => false, ReplyChange::Incident { .. } => true },
{
    let node = reply_family_values::failed_prefix(n);
    if node.run_failures >= 2 {
        let ready = reply_family_values::prepared(reply_family_values::failed_incident(node));
        retries_facts::incident(n, ready, ready.run_incidents >= 2);
    } else {
        retries_facts::restarted(n, node);
    }
}

pub(super) proof fn room_posts(n: Node, fast: bool, nq: u64, step: ReviewStop, i: int, ob: Outbox, oa: Outbox)
    requires node_headroom(n), arithmetic_consistent(n), nq < u64::MAX,
        reply_family_values::resolved(reply_family_values::room(n, fast, step), i, ob, oa),
    ensures ({ let r = reply_family_values::room(n, fast, step); let z = reply_family_values::finish(nq, r);
        reply_family_values::retry_bound(n, nq, r) && z.repo == n.repo && z.last_cmd == n.last_cmd
        && reply_family_values::next_question(nq, r) as int <= nq as int + 1 && oa.len <= ob.len + 1
        && (!(n.phase is Closed || n.phase is Stopping) ==> handler_safety::ladder(fast, true, n, z, ob, oa))
        && (!fast && !transition_safety::can_spend_one(n) ==> z.credit == n.credit
            && z.review_stop == n.review_stop
            && match z.question { Some(q) => q.kind == QKind::OutOfCredit
                && q.cont == Cont::Planning { must_split: true }, None => false }) }),
{
    reveal(reply_family_values::room);
    if fast {
        reply_family_facts::stopped_review(n, step);
        if !(n.phase is Closed || n.phase is Stopping) {
            handler_safety::stop_ladder(n, step, ob);
        }
    } else {
        let z = reply_family_values::finish(nq, reply_family_values::room(n, fast, step));
        if transition_safety::can_spend_one(n) {
            handler_safety::retry_ladder(n, z, ob, oa);
        } else {
            handler_safety::retry_ladder(n, z, ob, oa);
        }
    }
}

pub(super) proof fn files_arms(n: Node, fast: bool, files: Footprint, fits: bool,
    union: Footprint)
    requires fits, !footprint::footprint_empty(files),
    ensures
        (transition_safety::can_spend_one(n) ==> reply_family_values::files(n, fast, files, fits, union)
            == (ReplyChange::Done { node: reply_family_values::files_paid(n, union), note: None })),
        (!transition_safety::can_spend_one(n) && fast ==> reply_family_values::files(n, fast, files, fits, union)
            == (ReplyChange::Ask { node: reply_family_values::files_ask(n, union),
                kind: QKind::OutOfCredit, cont: Cont::Ready(Resume::Making), cost: 1 })),
{
    reveal(reply_family_values::files);
}

pub(super) proof fn files_posts(n: Node, fast: bool, nq: u64, files: Footprint, fits: bool,
    union: Footprint, i: int, ob: Outbox, oa: Outbox)
    requires node_headroom(n), arithmetic_consistent(n), nq < u64::MAX,
        fits, !footprint::footprint_empty(files), transition_safety::can_spend_one(n) || fast,
        reply_family_values::resolved(reply_family_values::files(n, fast, files, fits, union), i, ob, oa),
    ensures ({ let r = reply_family_values::files(n, fast, files, fits, union);
        let z = reply_family_values::finish(nq, r);
        reply_family_values::reply_bound(n, nq, r) && z.credit.start == n.credit.start
        && z.repo == n.repo && z.footprint == union
        && z.last_cmd == n.last_cmd && oa.len <= ob.len + 1
        && reply_family_values::next_question(nq, r) as int <= nq as int + 1
        && (transition_safety::can_spend_one(n) ==> z.review_stop == n.review_stop)
        && (!transition_safety::can_spend_one(n) ==> z.credit == n.credit
            && z.review_stop == n.review_stop && z.phase == Phase::AwaitingRob
            && match z.question { Some(q) => q.kind == QKind::OutOfCredit
                && q.cont == Cont::Ready(Resume::Making), None => false }) }),
{
    reveal(reply_family_values::files);
    if transition_safety::can_spend_one(n) {} else {}
}

pub(super) proof fn failed_run_props(n: Node)
    requires n.generation as int + 2 <= u64::MAX as int, n.run_failures <= 1, n.run_incidents <= 2,
    ensures ({ let z = retries_facts::failed_run(n);
        z.repo == n.repo && z.footprint == n.footprint && (settle_shape(n) ==> settle_shape(z))
        && z.generation as int <= n.generation as int + 2 && z.next_seq == n.next_seq
        && z.credit == n.credit && z.question == n.question && z.run_failures <= 1
        && z.run_incidents <= 2 && z.last_cmd == n.last_cmd && reply_family_values::frames(n, z)
        && (!(n.phase is Closed) ==> transition_safety::node_step(n, z)) }),
{
    reveal(retries_facts::failed_run);
    if n.run_failures == 1 {
        let ready = Node { pos: if n.phase == Phase::Making { 1u8 } else { 0u8 },
            generation: (n.generation + 1) as u64, run_failures: 0,
            run_incidents: if n.run_incidents < 2 { (n.run_incidents + 1) as u8 } else { 2u8 },
            ..retries_facts::abandoned(n) };
        reply_family_facts::incident_facts(ready, ready.run_incidents >= 2);
    }
}

pub(super) proof fn retry_lift(before: World, after: World, i: int, r: ReplyChange, ob: Outbox, oa: Outbox)
    requires 0 <= i < before.nodes@.len(), after.nodes@.len() == before.nodes@.len(),
        forall|j: int| 0 <= j < before.nodes@.len() && j != i
            ==> #[trigger] after.nodes@[j] == before.nodes@[j],
        after.nodes@[i] == reply_family_values::finish(before.next_question, r),
        after.next_question == reply_family_values::next_question(before.next_question, r),
        after.next_ticket == before.next_ticket, after.gates@ == before.gates@,
        after.pool_capacity == before.pool_capacity, after.fail_fast == before.fail_fast,
        after.file_sets@ == before.file_sets@, after.dependencies@ == before.dependencies@,
        reply_family_values::retry_bound(before.nodes@[i], before.next_question, r), oa.len <= ob.len + 1,
    ensures retry_bounds(before, after, i, ob.len, oa.len),
{
    reveal(retry_bounds);
}

pub(super) proof fn rung_posts(n: Node, fast: bool, nq: u64, step: ReviewStop,
    i: int, ob: Outbox, oa: Outbox)
    requires node_headroom(n), arithmetic_consistent(n), nq < u64::MAX,
        reply_family_values::resolved(reply_family_values::rung(n, fast, step), i, ob, oa),
    ensures ({ let r = reply_family_values::rung(n, fast, step);
        let z = reply_family_values::finish(nq, r);
        reply_family_values::retry_bound(n, nq, r)
        && z.repo == n.repo && z.last_cmd == n.last_cmd && oa.len <= ob.len + 1
        && reply_family_values::next_question(nq, r) as int <= nq as int + 1
        && (!(n.phase is Closed || n.phase is Stopping) && fast ==>
            z.review_stop.is_some() && z.credit == n.credit && oa.len == ob.len
                && z.phase == Phase::Stopping(Outcome::Stopped) && z.question.is_none())
        && (!(n.phase is Closed || n.phase is Stopping) && !fast ==>
            transition_safety::retried(n, z) && z.review_stop == n.review_stop)
        && (!(n.phase is Closed || n.phase is Stopping) ==>
            handler_safety::ladder(fast, true, n, z, ob, oa)) }),
{
    reveal(reply_family_values::rung);
    let z = reply_family_values::finish(nq, reply_family_values::rung(n, fast, step));
    if fast {
        reply_family_facts::stopped_review(n, step);
        if !(n.phase is Closed || n.phase is Stopping) {
            handler_safety::stop_ladder(n, step, ob);
        }
    } else {
        if n.attempt < 3 {} else if !n.replanned {} else {}
        handler_safety::retry_ladder(n, z, ob, oa);
    }
}

pub(super) proof fn failed_round_posts(n: Node, fast: bool, nq: u64, step: ReviewStop,
    i: int, ob: Outbox, oa: Outbox)
    requires node_headroom(n), arithmetic_consistent(n), nq < u64::MAX,
        reply_family_values::resolved(reply_family_values::failed_round(n, fast, step), i, ob, oa),
    ensures ({ let r = reply_family_values::failed_round(n, fast, step);
        let z = reply_family_values::finish(nq, r);
        reply_family_values::retry_bound(n, nq, r)
        && z.repo == n.repo && z.last_cmd == n.last_cmd && oa.len <= ob.len + 1
        && reply_family_values::next_question(nq, r) as int <= nq as int + 1
        && (!(n.phase is Closed || n.phase is Stopping) ==>
            handler_safety::ladder(fast, n.round != 0, n, z, ob, oa)) }),
{
    reveal(reply_family_values::failed_round);
    if n.round == 0 {
        let z = reply_family_values::finish(nq, reply_family_values::failed_round(n, fast, step));
        handler_safety::same_ladder(fast, n, z, ob, oa);
    } else {
        rung_posts(n, fast, nq, step, i, ob, oa);
    }
}

pub(super) proof fn plan_retry_posts(n: Node, fast: bool, nq: u64, cant_meet: bool,
    i: int, ob: Outbox, oa: Outbox)
    requires node_headroom(n), arithmetic_consistent(n), nq < u64::MAX,
        reply_family_values::resolved(reply_family_values::plan_retry(n, fast, cant_meet), i, ob, oa),
    ensures ({ let r = reply_family_values::plan_retry(n, fast, cant_meet);
        let z = reply_family_values::finish(nq, r);
        reply_family_values::retry_bound(n, nq, r)
        && z.repo == n.repo && z.last_cmd == n.last_cmd && oa.len <= ob.len + 1
        && reply_family_values::next_question(nq, r) as int <= nq as int + 1
        && (n.phase is Planning ==> handler_safety::ladder(fast, cant_meet, n, z, ob, oa))
        && (fast && cant_meet && n.phase is Planning ==>
            z.phase == Phase::Stopping(Outcome::Stopped)
                && z.review_stop == Some(ReviewStop::PlanCantMeet)) }),
{
    reveal(reply_family_values::plan_retry);
    let m = Node { plan_retries: if n.plan_retries < 255 { (n.plan_retries + 1) as u8 }
        else { 255u8 }, ..n };
    let z = reply_family_values::finish(nq, reply_family_values::plan_retry(n, fast, cant_meet));
    if fast && cant_meet {
        reply_family_facts::stopped_review(m, ReviewStop::PlanCantMeet);
        if n.phase is Planning {
            handler_safety::stop_ladder(m, ReviewStop::PlanCantMeet, ob);
            handler_safety::ladder_rebase(fast, true, n, m, z, ob, oa);
        }
    } else if n.phase is Planning {
        if cant_meet { handler_safety::retry_ladder(n, z, ob, oa); }
        else { handler_safety::same_ladder(fast, n, z, ob, oa); }
    }
}

pub(super) proof fn recheck_posts(n: Node, nq: u64, target: Phase,
    i: int, ob: Outbox, oa: Outbox)
    requires !(target is Closed), node_headroom(n), arithmetic_consistent(n), nq < u64::MAX,
        reply_family_values::resolved(reply_family_values::recheck(n, target), i, ob, oa),
    ensures ({ let r = reply_family_values::recheck(n, target);
        let z = reply_family_values::finish(nq, r);
        reply_family_values::retry_bound(n, nq, r)
        && z.repo == n.repo && z.last_cmd == n.last_cmd && oa.len <= ob.len + 1
        && reply_family_values::next_question(nq, r) as int <= nq as int + 1
        && handler_safety::keeps_review_stop(n, z)
        && (ordinary_await(n) && n.phase == Phase::Rebasing && target == Phase::Checking
            ==> distance(z) < distance(n)) }),
{
    reveal(reply_family_values::recheck);
    let z = reply_family_values::finish(nq, reply_family_values::recheck(n, target));
    handler_safety::keep_review_stop(n, z);
    reveal(distance);
    reveal_with_fuel(phase_rank, 3);
    reveal(continuation_rank);
}

pub(super) proof fn rung_arms(n: Node, fast: bool, step: ReviewStop)
    ensures
        fast ==> reply_family_values::rung(n, fast, step) == reply_family_values::room_fast(n, step),
        !fast && n.attempt < 3 && transition_safety::can_spend_one(n) ==>
            reply_family_values::rung(n, fast, step) == reply_family_values::rung_paid(reply_family_values::bumped(n)),
        !fast && n.attempt < 3 && !transition_safety::can_spend_one(n) ==>
            reply_family_values::rung(n, fast, step) == reply_family_values::rung_ask(reply_family_values::bumped(n)),
        !fast && !(n.attempt < 3) && !n.replanned && transition_safety::can_spend_one(n) ==>
            reply_family_values::rung(n, fast, step) == reply_family_values::rung_replan_paid(reply_family_values::replanning(n)),
        !fast && !(n.attempt < 3) && !n.replanned && !transition_safety::can_spend_one(n) ==>
            reply_family_values::rung(n, fast, step) == reply_family_values::rung_replan_ask(reply_family_values::replanning(n)),
        !fast && !(n.attempt < 3) && n.replanned ==>
            reply_family_values::rung(n, fast, step) == reply_family_values::rung_cant_meet(n),
{
    reveal(reply_family_values::rung); reveal(reply_family_values::bumped);
    reveal(reply_family_values::replanning); reveal(reply_family_values::rung_paid);
    reveal(reply_family_values::rung_ask); reveal(reply_family_values::rung_replan_paid);
    reveal(reply_family_values::rung_replan_ask); reveal(reply_family_values::rung_cant_meet);
}

pub(super) proof fn failed_round_arms(n: Node, fast: bool, step: ReviewStop)
    ensures
        n.round == 0 ==> reply_family_values::failed_round(n, fast, step) == reply_family_values::round_free(n),
        n.round != 0 ==> reply_family_values::failed_round(n, fast, step) == reply_family_values::rung(n, fast, step),
{
    reveal(reply_family_values::failed_round); reveal(reply_family_values::round_free);
}

pub(super) proof fn plan_retry_arms(n: Node, fast: bool, cant_meet: bool)
    ensures ({ let m = reply_family_values::plan_counted(n);
        (fast && cant_meet ==> reply_family_values::plan_retry(n, fast, cant_meet)
            == reply_family_values::room_fast(m, ReviewStop::PlanCantMeet))
        && (!(fast && cant_meet) && m.plan_retries < 2 ==>
            reply_family_values::plan_retry(n, fast, cant_meet) == reply_family_values::plan_again(m))
        && (!(fast && cant_meet) && !(m.plan_retries < 2) ==>
            reply_family_values::plan_retry(n, fast, cant_meet) == reply_family_values::plan_ask(m)) }),
{
    reveal(reply_family_values::plan_retry); reveal(reply_family_values::plan_counted);
    reveal(reply_family_values::plan_again); reveal(reply_family_values::plan_ask);
}

pub(super) proof fn held_eq(n: Node, target: Phase)
    ensures retirement::phased(reply_family_values::held(n, target), target)
        == retirement::phased(Node { holds: holding(target) && n.holds, ..n }, target),
{
    reveal(reply_family_values::held);
    if holding(target) {} else {}
}

pub(super) proof fn recheck_free_arm(n: Node, target: Phase)
    ensures ({ let m = reply_family_values::rechecked(n);
        m.rechecks <= FREE_RECHECKS ==> reply_family_values::recheck(n, target)
            == reply_family_values::recheck_free(m, target) }),
{
    let m = reply_family_values::rechecked(n);
    if m.rechecks <= FREE_RECHECKS {
        reveal(reply_family_values::recheck); reveal(reply_family_values::rechecked);
        reveal(reply_family_values::recheck_free);
        held_eq(m, target);
    }
}

pub(super) proof fn recheck_paid_arm(n: Node, target: Phase)
    ensures ({ let m = reply_family_values::rechecked(n);
        !(m.rechecks <= FREE_RECHECKS) && transition_safety::can_spend_one(m) ==>
            reply_family_values::recheck(n, target) == reply_family_values::recheck_paid(m, target) }),
{
    let m = reply_family_values::rechecked(n);
    if !(m.rechecks <= FREE_RECHECKS) && transition_safety::can_spend_one(m) {
        reveal(reply_family_values::recheck); reveal(reply_family_values::rechecked);
        reveal(reply_family_values::recheck_paid);
        held_eq(reply_family_values::spent(m), target);
    }
}

pub(super) proof fn recheck_ask_arm(n: Node, target: Phase)
    ensures ({ let m = reply_family_values::rechecked(n);
        !(m.rechecks <= FREE_RECHECKS) && !transition_safety::can_spend_one(m) ==>
            reply_family_values::recheck(n, target) == reply_family_values::recheck_ask(m, target) }),
{
    let m = reply_family_values::rechecked(n);
    if !(m.rechecks <= FREE_RECHECKS) && !transition_safety::can_spend_one(m) {
        reveal(reply_family_values::recheck); reveal(reply_family_values::rechecked);
        reveal(reply_family_values::recheck_ask);
    }
}

pub(super) proof fn recheck_arms(n: Node, target: Phase)
    ensures ({ let m = reply_family_values::rechecked(n);
        (m.rechecks <= FREE_RECHECKS ==> reply_family_values::recheck(n, target)
            == reply_family_values::recheck_free(m, target))
        && (!(m.rechecks <= FREE_RECHECKS) && transition_safety::can_spend_one(m) ==>
            reply_family_values::recheck(n, target) == reply_family_values::recheck_paid(m, target))
        && (!(m.rechecks <= FREE_RECHECKS) && !transition_safety::can_spend_one(m) ==>
            reply_family_values::recheck(n, target) == reply_family_values::recheck_ask(m, target)) }),
{
    recheck_free_arm(n, target);
    recheck_paid_arm(n, target);
    recheck_ask_arm(n, target);
}

pub(super) proof fn room_bridge(n: Node, fast: bool, step: ReviewStop)
    ensures
        fast ==> reply_family_values::room(n, fast, step) == reply_family_values::room_fast(n, step),
        !fast && transition_safety::can_spend_one(n) ==> reply_family_values::room(n, fast, step)
            == reply_family_values::room_paid(reply_family_values::unheld(n)),
        !fast && !transition_safety::can_spend_one(n) ==> reply_family_values::room(n, fast, step)
            == reply_family_values::room_ask(reply_family_values::unheld(n)),
{
    reveal(reply_family_values::room); reveal(reply_family_values::unheld);
}

pub(super) proof fn rung_value_posts(n: Node, fast: bool, nq: u64, step: ReviewStop)
    requires node_headroom(n), arithmetic_consistent(n), nq < u64::MAX,
    ensures
        reply_family_values::retry_bound(n, nq, reply_family_values::rung(n, fast, step)),
        !(n.phase is Closed) ==> transition_safety::node_step(n, reply_family_values::finish(nq, reply_family_values::rung(n, fast, step))),
{
    reveal(reply_family_values::rung);
    if fast { reply_family_facts::stopped_review(n, step); }
    else if n.attempt < 3 {} else if !n.replanned {} else {}
}

pub(super) proof fn failed_round_value_posts(n: Node, fast: bool, nq: u64, step: ReviewStop)
    requires node_headroom(n), arithmetic_consistent(n), nq < u64::MAX,
    ensures
        !reply_family_values::retry_bound(n, nq, reply_family_values::failed_round(n, fast, step)),
        !(n.phase is Closed) ==> transition_safety::node_step(n, reply_family_values::finish(nq, reply_family_values::failed_round(n, fast, step))),
{
    reveal(reply_family_values::failed_round);
    if n.round != 0 { rung_value_posts(n, fast, nq, step); }
}

pub(super) proof fn plan_retry_value_posts(n: Node, fast: bool, nq: u64, cant_meet: bool)
    requires node_headroom(n), arithmetic_consistent(n), nq < u64::MAX,
    ensures
        reply_family_values::retry_bound(n, nq, reply_family_values::plan_retry(n, fast, cant_meet)),
        fast && cant_meet && n.phase is Planning ==>
            reply_family_values::finish(nq, reply_family_values::plan_retry(n, fast, cant_meet)).phase == Phase::Stopping(Outcome::Stopped)
                && reply_family_values::finish(nq, reply_family_values::plan_retry(n, fast, cant_meet)).review_stop == Some(ReviewStop::PlanCantMeet),
        !(n.phase is Closed) ==> transition_safety::node_step(n, reply_family_values::finish(nq, reply_family_values::plan_retry(n, fast, cant_meet))),
{
    reveal(reply_family_values::plan_retry);
    if fast && cant_meet {
        let m = Node { plan_retries: if n.plan_retries < 255 { (n.plan_retries + 1) as u8 } else { 255u8 }, ..n };
        reply_family_facts::stopped_review(m, ReviewStop::PlanCantMeet);
    }
}

pub(super) proof fn recheck_value_posts(n: Node, nq: u64, target: Phase)
    requires !(target is Closed), node_headroom(n), arithmetic_consistent(n), nq < u64::MAX,
    ensures
        reply_family_values::retry_bound(n, nq, reply_family_values::recheck(n, target)),
        (ordinary_await(n) && n.phase == Phase::Rebasing && target == Phase::Checking
            ==> distance(reply_family_values::finish(nq, reply_family_values::recheck(n, target))) < distance(n)),
        !(n.phase is Closed) ==> transition_safety::node_step(n, reply_family_values::finish(nq, reply_family_values::recheck(n, target))),
{
    reveal(reply_family_values::recheck);
    reveal(distance); reveal_with_fuel(phase_rank, 3); reveal(continuation_rank);
}

pub(super) proof fn room_bound(n: Node, fast: bool, nq: u64, step: ReviewStop)
    requires node_headroom(n), arithmetic_consistent(n), nq < u64::MAX,
    ensures reply_family_values::retry_bound(n, nq, reply_family_values::room(n, fast, step)),
{
    reveal(reply_family_values::room);
    if fast { reply_family_facts::stopped_review(n, step); }
}

pub(super) proof fn rebase_value_posts(n: Node, nq: u64, reply: Reply)
    requires node_headroom(n), arithmetic_consistent(n), nq < u64::MAX,
        matches!(reply, Reply::RebaseClean { .. } | Reply::RebaseNeedsChanges | Reply::RunFailed),
    ensures reply_family_values::reply_bound(n, nq, reply_family_values::rebase(n, reply)),
        (successful_reply_input(n, reply) ==>
            distance(reply_family_values::finish(nq, reply_family_values::rebase(n, reply))) < distance(n)),
        reply_family_values::finish(nq, reply_family_values::rebase(n, reply)).footprint == n.footprint,
{
    reveal(reply_family_values::rebase);
    reveal(distance); reveal_with_fuel(phase_rank, 3); reveal(continuation_rank);
    match reply {
        Reply::RebaseClean { head } => {
            let m = Node { head: Some(head), checked: None,
                review_restarted: n.head == Some(head) && n.review_restarted, ..n };
            recheck_value_posts(m, nq, Phase::Checking);
        },
        _ => {},
    }
}

}
