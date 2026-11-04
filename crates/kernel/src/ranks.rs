//! Mathematical ranks for the executable state-table code, #771.
//! These definitions contain no transition relation. Executable rank functions
//! prove their contracts against these mathematical values.
use super::*;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

#[cfg(verus_keep_ghost)]
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

pub(super) open spec fn base_distance(n: Node) -> nat {
    let rank = match n.phase {
        Phase::AwaitingRob => match n.question {
            Some(q) => continuation_rank(q.cont) + 1,
            None => 3,
        },
        p => phase_rank(p),
    };
    let len = items(n.phase).len();
    let pos = if n.pos < len { n.pos as nat } else { len };
    (rank * 64 + 2 * (len - pos)) as nat
}

pub(super) open spec fn effect_distance(n: Node) -> int {
    match n.effect {
        None => 0int,
        Some(e) => if e.cancelling { 1int } else {
            match current_item(n) {
                Some(Item::Do(k)) => if k == e.kind { -1int } else { 1int },
                Some(Item::Teardown(k)) => if k == e.kind { -1int } else { 1int },
                _ => 1int,
            }
        },
    }
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
        (base_distance(n) + effect_distance(n)
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

impl Node {
    kernel_fn! {
        /// Bookkeeping settle still owes the node without moving it closer to
        /// done: a move to Ready because it's paused (3), taking a place in line
        /// (2), or giving one back (1).
        fn pending(&self) -> (result: u64) [
            ensures result as int == pending_rank(*self), result <= 3,
        ] {
            if self.phase.is_closed() {
                return 0;
            }
            if self.waiting_incident {
                return if self.ticket.is_some() { 1 } else { 0 };
            }
            if self.effect.is_some() {
                return 0;
            }
            if self.pause_move_due() {
                return 3;
            }
            match self.current() {
                Some(Item::Start(_) | Item::Slot) => match (self.paused, self.ticket) {
                    (false, None) => 2,
                    (true, Some(_)) => 1,
                    _ => 0,
                },
                _ => 0,
            }
        }
    }

    kernel_fn! {
        /// Falls with every internal transition settle takes. Kani proves it, so
        /// settle ends for any number of requests.
        pub fn settle_rank(&self) -> (result: u64) [
            ensures result as int == 4 * super::ranks::distance(*self) + pending_rank(*self),
                result <= 16387,
        ] {
            4 * self.distance() + self.pending()
        }
    }

    kernel_fn! {
        /// Distance to Closed. Each witnessed event lowers the world's total, and
        /// no internal transition raises it.
        // The pinned Verus frontend requires nested if-let instead of let chains.
        #[allow(clippy::collapsible_if)]
        pub fn distance(&self) -> (result: u64) [
            ensures result as int == super::ranks::distance(*self), result <= 4096,
        ] proof [reveal(effect_distance); reveal(current_item);] {
            if self.phase.is_closed() {
                return 0;
            }
            if self.pause_move_due() {
                if let Some(r) = self.phase.resume() {
                    // Counted as the paused Ready it is about to become.
                    return rank(Phase::Ready(r)) * 64 + 2 + 8;
                }
            }
            kernel_proof! { assert(!pause_due(*self)); }
            let rank = match self.phase {
                Phase::AwaitingRob => match self.question {
                    Some(q) => cont_rank(q.cont) + 1,
                    None => 3,
                },
                phase => rank(phase),
            };
            let len = agenda(self.phase).len() as u64;
            let pos = (self.pos as u64).min(len);
            let mut d = rank * 64 + 2 * (len - pos);
            kernel_proof! { assert(d as int == base_distance(*self)); }
            if let Some(e) = self.effect {
                d += 1;
                if !e.cancelling {
                    match self.current() {
                        Some(Item::Do(k)) => {
                            if k == e.kind {
                                kernel_proof! { assert(effect_distance(*self) == -1); }
                                d -= 2;
                            } else {
                                kernel_proof! { assert(effect_distance(*self) == 1); }
                            }
                        }
                        Some(Item::Teardown(k)) => {
                            if k == e.kind {
                                kernel_proof! { assert(effect_distance(*self) == -1); }
                                d -= 2;
                            } else {
                                kernel_proof! { assert(effect_distance(*self) == 1); }
                            }
                        }
                        _ => { kernel_proof! { assert(effect_distance(*self) == 1); } }
                    }
                } else {
                    kernel_proof! { assert(effect_distance(*self) == 1); }
                }
                kernel_proof! {
                    assert(d as int == base_distance(*self) + effect_distance(*self));
                }
            } else {
                kernel_proof! { assert(effect_distance(*self) == 0); }
            }
            kernel_proof! {
                assert(d as int == base_distance(*self) + effect_distance(*self));
            }
            if self.paused && !self.phase.is_stopping() {
                d += 8;
            }
            if self.waiting_incident {
                d += 8;
            }
            if self.question.is_some() && self.phase.is_stopping() {
                d += 4;
            }
            d
        }
    }
}
