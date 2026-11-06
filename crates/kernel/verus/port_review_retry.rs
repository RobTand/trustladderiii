//! #806: exact shared retry updates, checked against the production handlers.
use super::*;

verus! {
/// State-table credit rows: the dependency-aware one-credit decision.
pub(super) open spec fn funded(n: Node) -> bool {
    !n.credit.withheld && n.credit.start as int + n.credit.earned as int
        + n.credit.added as int >= n.credit.spent as int + 1
}

/// State-table paid retry rows: every other credit field is unchanged.
pub(super) open spec fn paid(n: Node) -> Node {
    Node { credit: Credit { spent: (n.credit.spent + 1) as u64, ..n.credit }, ..n }
}

/// State-table AwaitingRob rows: the real ask_rob node update.
pub(super) open spec fn asked(n: Node, id: u64, kind: QKind, cont: Cont) -> Node {
    retirement::phased(Node { holds: false,
        question: Some(Question { id, kind, cont, cost: 1, clarified: false }), ..n },
        Phase::AwaitingRob)
}

/// Q54: stopping for review records the reason before the real stopping update.
pub(super) open spec fn review_stopped(n: Node, step: ReviewStop) -> Node {
    retirement::stopped(Node { review_stop: Some(step), ..n }, Outcome::Stopped)
}

/// State-table failed-round ladder: exact attempt, re-plan, credit and question fields.
pub(super) open spec fn rung(n: Node, fail_fast: bool, id: u64, step: ReviewStop) -> (Node, u64) {
    if fail_fast { (review_stopped(n, step), id) }
    else if n.attempt < 3 {
        let m = Node { attempt: (n.attempt + 1) as u8, review_restarted: false, round: 0, ..n };
        if funded(m) { (retirement::phased(paid(m), Phase::Making), id) }
        else { (asked(m, id, QKind::OutOfCredit, Cont::Ready(Resume::Making)), (id + 1) as u64) }
    } else if !n.replanned {
        let m = Node { replanned: true, holds: false, ..n };
        if funded(m) {
            (retirement::phased(Node { plan_retries: 0, ..paid(m) }, Phase::Planning { must_split: false }), id)
        } else { (asked(m, id, QKind::OutOfCredit, Cont::Planning { must_split: false }), (id + 1) as u64) }
    } else { (asked(n, id, QKind::CantMeet, Cont::Ready(Resume::Making)), (id + 1) as u64) }
}

/// State-table failed round: the first fix is free, even under fail_fast.
pub(super) open spec fn round(n: Node, fail_fast: bool, id: u64, step: ReviewStop) -> (Node, u64) {
    if n.round == 0 { (retirement::phased(Node { round: 1, ..n }, Phase::Making), id) }
    else { rung(n, fail_fast, id, step) }
}

/// State-table exhaustion rows and Q54: split, request credit, or stop for review.
pub(super) open spec fn room(n: Node, fail_fast: bool, id: u64, step: ReviewStop) -> (Node, u64) {
    if fail_fast { (review_stopped(n, step), id) }
    else {
        let m = Node { holds: false, ..n };
        if funded(m) {
            (retirement::phased(Node { plan_retries: 0, ..paid(m) }, Phase::Planning { must_split: true }), id)
        } else { (asked(m, id, QKind::OutOfCredit, Cont::Planning { must_split: true }), (id + 1) as u64) }
    }
}

/// State-table recheck allowance: saturating u8 count, then one credit or a question.
pub(super) open spec fn rechecked(n: Node, target: Phase, id: u64) -> (Node, u64) {
    let count = if n.rechecks < u8::MAX { (n.rechecks + 1) as u8 } else { u8::MAX };
    let m = Node { rechecks: count, ..n };
    if count <= FREE_RECHECKS || funded(m) {
        let credit = if count <= FREE_RECHECKS { m } else { paid(m) };
        (retirement::phased(Node { holds: holding(target) && credit.holds, ..credit }, target), id)
    } else {
        (asked(m, id, QKind::OutOfCredit, Cont::Ready(if target == Phase::Reviewing {
            Resume::Reviewing } else { Resume::Checking })), (id + 1) as u64)
    }
}

/// Q30: abandoning an operation cancels only its start effect.
pub(super) open spec fn abandoned(n: Node) -> Node {
    Node { run: None, job: None, pool: false, effect: match n.effect {
        Some(e) => Some(InFlight { cancelling: e.cancelling || matches!(e.kind,
            EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase), ..e }),
        None => None,
    }, ..n }
}

/// State-table failed run: restart once, then raise the saturating incident.
pub(super) open spec fn failed_run(n: Node) -> Node {
    let m = Node { pos: if n.phase == Phase::Making { 1u8 } else { 0u8 },
        generation: (n.generation + 1) as u64,
        run_failures: (n.run_failures + 1) as u8, ..abandoned(n) };
    if m.run_failures >= 2 {
        let count = if n.run_incidents < 2 { (n.run_incidents + 1) as u8 } else { 2u8 };
        retirement::incident(Node { run_failures: 0, run_incidents: count, ..m }, count >= 2)
    } else { m }
}
}
