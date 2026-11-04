//! Mathematical ranks for the executable state-table code, #771.
//! These definitions contain no transition relation. Executable functions in
//! lib.rs must prove their contracts against these ranks.
use super::*;
use vstd::prelude::*;

verus! {
pub open spec fn resumed_phase(r: Resume) -> Phase {
    match r {
        Resume::Making => Phase::Making,
        Resume::Checking => Phase::Checking,
        Resume::Reviewing => Phase::Reviewing,
        Resume::Publishing => Phase::Publishing,
        Resume::Rebasing => Phase::Rebasing,
    }
}

pub open spec fn phase_rank(p: Phase) -> nat
    decreases if p is Ready { 1nat } else { 0nat },
{
    match p {
        Phase::Closed(_) => 0,
        Phase::Stopping(_) => 2,
        Phase::Queued => 4,
        Phase::Publishing => 6,
        Phase::Reviewing => 10,
        Phase::Checking => 14,
        Phase::Rebasing => 18,
        Phase::Making => 22,
        Phase::Ready(r) => phase_rank(resumed_phase(r)) + 2,
        Phase::Planning { .. } => 26,
        Phase::Unplanned => 28,
        Phase::Auditing => 30,
        Phase::Ingesting => 32,
        Phase::AwaitingRob => 3,
    }
}

pub open spec fn continuation_rank(c: Cont) -> nat {
    match c {
        Cont::Ingesting => 32,
        Cont::Auditing => 30,
        Cont::Unplanned => 28,
        Cont::Planning { .. } => 26,
        Cont::Ready(r) => phase_rank(Phase::Ready(r)),
        Cont::Stop(_) | Cont::Nowhere => 2,
    }
}

pub(super) open spec fn items(p: Phase) -> Seq<Item> {
    use EffectKind as K;
    use Item as I;
    match p {
        Phase::Ingesting => seq![I::Start(Role::Ingest), I::Await],
        Phase::Auditing => seq![I::Start(Role::Audit), I::Await],
        Phase::Unplanned => seq![I::Admit],
        Phase::Planning { .. } => seq![I::Start(Role::Plan), I::Await],
        Phase::Ready(_) => seq![I::Slot],
        Phase::Making => seq![I::Do(K::PrepareWorktree), I::Start(Role::Make), I::Await],
        Phase::Checking => seq![I::StartChecks, I::Await],
        Phase::Reviewing => seq![I::Start(Role::Review), I::Await],
        Phase::Publishing => seq![I::Do(K::Push), I::Do(K::PostApproval),
            I::Do(K::OpenPr), I::Do(K::Enqueue)],
        Phase::Queued => seq![I::Merge],
        Phase::Rebasing => seq![I::StartRebase, I::Await],
        Phase::AwaitingRob => seq![I::Answer],
        Phase::Stopping(_) => seq![I::Teardown(K::StopRun), I::Teardown(K::WithdrawJob),
            I::Teardown(K::ClosePr), I::Release, I::Teardown(K::RemoveWorktree)],
        Phase::Closed(_) => seq![],
    }
}

pub(super) open spec fn current_item(n: Node) -> Option<Item> {
    if n.pos < items(n.phase).len() { Some(items(n.phase)[n.pos as int]) } else { None }
}

pub open spec fn holding(p: Phase) -> bool {
    matches!(p, Phase::Making | Phase::Checking | Phase::Reviewing
        | Phase::Publishing | Phase::Queued | Phase::Rebasing)
}

pub(super) open spec fn pause_due(n: Node) -> bool {
    n.paused && holding(n.phase) && !n.waiting_incident && n.effect.is_none()
        && matches!(current_item(n), Some(Item::Do(_) | Item::Start(_)
            | Item::StartChecks | Item::StartRebase))
}

pub closed spec fn pending_rank(n: Node) -> nat {
    if n.phase is Closed { 0 }
    else if n.waiting_incident { if n.ticket.is_some() { 1 } else { 0 } }
    else if n.effect.is_some() { 0 }
    else if pause_due(n) { 3 }
    else { match current_item(n) {
        Some(Item::Start(_) | Item::Slot) => match (n.paused, n.ticket) {
            (false, None) => 2,
            (true, Some(_)) => 1,
            _ => 0,
        },
        _ => 0,
    } }
}

pub closed spec fn distance(n: Node) -> nat {
    if n.phase is Closed { 0 }
    else if pause_due(n) {
        phase_rank(Phase::Ready(match n.phase {
            Phase::Making => Resume::Making,
            Phase::Checking => Resume::Checking,
            Phase::Reviewing => Resume::Reviewing,
            Phase::Publishing | Phase::Queued => Resume::Publishing,
            _ => Resume::Rebasing,
        })) * 64 + 10
    } else {
        let rank = match n.phase {
            Phase::AwaitingRob => match n.question {
                Some(q) => continuation_rank(q.cont) + 1,
                None => 3,
            },
            p => phase_rank(p),
        };
        let len = items(n.phase).len();
        let pos = if n.pos < len { n.pos as nat } else { len };
        let effect = match n.effect {
            None => 0int,
            Some(e) => 1 - if !e.cancelling && match current_item(n) {
                Some(Item::Do(k) | Item::Teardown(k)) => k == e.kind,
                _ => false,
            } { 2int } else { 0int },
        };
        (rank * 64 + 2 * (len - pos) + effect
            + if n.paused && !(n.phase is Stopping) { 8int } else { 0int }
            + if n.waiting_incident { 8int } else { 0int }
            + if n.question.is_some() && n.phase is Stopping { 4int } else { 0int }) as nat
    }
}

/// State-table progress measure with a mathematical sum, for any sequence size.
pub(super) open spec fn node_total(nodes: Seq<Node>) -> nat
    decreases nodes.len(),
{
    if nodes.len() == 0 { 0 }
    else { node_total(nodes.drop_last()) + distance(nodes.last()) }
}

/// #771 / #764 Target A: a changed node is one summand at every finite size.
pub(super) proof fn sum_update(nodes: Seq<Node>, index: int, replacement: Node)
    requires 0 <= index < nodes.len(),
    ensures node_total(nodes.update(index, replacement)) + distance(nodes[index])
        == node_total(nodes) + distance(replacement),
    decreases nodes.len(),
{
    if index == nodes.len() - 1 {
        assert(nodes.update(index, replacement).drop_last() == nodes.drop_last());
    } else {
        sum_update(nodes.drop_last(), index, replacement);
        assert(nodes.update(index, replacement).drop_last()
            == nodes.drop_last().update(index, replacement));
        assert(nodes.drop_last()[index] == nodes[index]);
    }
}
}
