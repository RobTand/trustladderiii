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

#[verifier::opaque]
pub(super) open spec fn distance(n: Node) -> nat {
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

/// #771: public view of the executable node distance.
pub closed spec fn node_distance(n: Node) -> nat { distance(n) }

/// #771: local moves left before this node blocks, independent of scheduling.
/// Stopping has five agenda entries and closure, hence at most six moves.
pub(super) open spec fn internal_height(n: Node) -> nat {
    if n.phase is Closed { 0 }
    else if n.waiting_incident { if n.ticket.is_some() { 1 } else { 0 } }
    else if n.effect.is_some() { 0 }
    else { match current_item(n) {
        None => match n.phase {
            Phase::Publishing => 1,
            Phase::Stopping(_) => if n.question.is_none() { 1 } else { 0 },
            _ => 0,
        },
        Some(Item::Admit) => if n.paused { 0 } else { 3 },
        Some(Item::Slot) => if n.paused {
            if n.ticket.is_some() { 1 } else { 0 }
        } else { if n.ticket.is_some() { 3 } else { 4 } },
        Some(Item::Start(_)) => if n.paused {
            if holding(n.phase) || n.ticket.is_some() { 1 } else { 0 }
        } else { if n.ticket.is_some() { 1 } else { 2 } },
        Some(Item::Do(_) | Item::StartChecks | Item::StartRebase) => 1,
        Some(Item::Release) => (6 - n.pos) as nat,
        Some(Item::Teardown(_)) => if n.question.is_some() { 0 } else { (6 - n.pos) as nat },
        _ => 0,
    } }
}

/// #771: at most one phase change remains within a node's settle chain.
pub(super) open spec fn generation_capacity(n: Node) -> nat {
    if n.phase is Closed || n.waiting_incident || n.effect.is_some() { 0 }
    else if pause_due(n) { 1 }
    else { match current_item(n) {
        Some(Item::Admit | Item::Slot) => if n.paused { 0 } else { 1 },
        None => if n.phase == Phase::Publishing { 1 } else { 0 },
        _ => 0,
    } }
}

/// #771: emitting an effect blocks the node for the rest of settle.
pub(super) open spec fn effect_capacity(n: Node) -> nat {
    if n.phase is Closed || n.waiting_incident || n.effect.is_some() { 0 } else { 1 }
}

/// Q39 / #771: Ready can take a slot ticket followed by a pool ticket.
pub(super) open spec fn ticket_capacity(n: Node) -> nat {
    if n.phase is Closed || n.waiting_incident || n.effect.is_some() || n.paused { 0 }
    else { match current_item(n) {
        Some(Item::Slot) => if n.ticket.is_some() { 1 } else { 2 },
        Some(Item::Admit) => 1,
        Some(Item::Start(_)) => if n.ticket.is_some() { 0 } else { 1 },
        _ => 0,
    } }
}

pub(super) open spec fn merge_capacity(n: Node) -> nat {
    if n.credit.merge_credited || n.phase is Closed { 0 } else { 1 }
}

/// #771: these are mathematical potentials, not additional machine counters.
pub(super) proof fn local_capacities(n: Node)
    ensures internal_height(n) <= 6, generation_capacity(n) <= 1,
        effect_capacity(n) <= 1, ticket_capacity(n) <= 2, merge_capacity(n) <= 1,
{
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

/// #771: sum a local allocation potential over arbitrary finite storage.
pub(super) open spec fn potential_prefix(nodes: Seq<Node>, end: nat,
    potential: spec_fn(Node) -> nat) -> nat
    recommends end <= nodes.len(),
    decreases end,
{
    if end == 0 { 0 } else {
        potential_prefix(nodes, (end - 1) as nat, potential)
            + potential(nodes[end as int - 1])
    }
}

pub(super) open spec fn pool_users(nodes: Seq<Node>, end: nat) -> nat {
    potential_prefix(nodes, end, |n: Node| if n.pool { 1nat } else { 0nat })
}

pub(super) open spec fn slot_waiters(nodes: Seq<Node>, end: nat, repo: RepoId) -> nat {
    potential_prefix(nodes, end, |n: Node|
        if n.repo == repo && n.phase is Ready && n.ticket.is_some() { 1nat } else { 0nat })
}

pub(super) proof fn potential_prefix_equal(left: Seq<Node>, right: Seq<Node>, end: nat,
    potential: spec_fn(Node) -> nat)
    requires end <= left.len(), end <= right.len(),
        forall|i: int| 0 <= i < end ==> left[i] == right[i],
    ensures potential_prefix(left, end, potential) == potential_prefix(right, end, potential),
    decreases end,
{
    if end > 0 {
        potential_prefix_equal(left, right, (end - 1) as nat, potential);
    }
}

/// #771: one actual node update changes exactly its allocation potential.
pub(super) proof fn potential_update(nodes: Seq<Node>, index: int,
    replacement: Node, end: nat, potential: spec_fn(Node) -> nat)
    requires 0 <= index < end <= nodes.len(),
    ensures potential_prefix(nodes.update(index, replacement), end, potential)
            + potential(nodes[index])
        == potential_prefix(nodes, end, potential) + potential(replacement),
    decreases end,
{
    if index < end - 1 {
        potential_update(nodes, index, replacement, (end - 1) as nat, potential);
    } else {
        potential_prefix_equal(nodes.update(index, replacement), nodes,
            (end - 1) as nat, potential);
    }
}

/// #771: one node's allocation reserve is contained in the finite sum.
pub(super) proof fn potential_member(nodes: Seq<Node>, index: int, end: nat,
    potential: spec_fn(Node) -> nat)
    requires 0 <= index < end <= nodes.len(),
    ensures potential(nodes[index]) <= potential_prefix(nodes, end, potential),
    decreases end,
{
    if index < end - 1 { potential_member(nodes, index, (end - 1) as nat, potential); }
}

/// Q30/Q39 / #771: every nonempty finite line has an oldest eligible member.
/// The caller supplies the actual ticket or priority/ticket key.
pub(super) proof fn finite_minimum(nodes: Seq<Node>, end: nat, eligible: spec_fn(Node) -> bool,
    key: spec_fn(Node) -> nat)
    requires end <= nodes.len(), exists|i: int| 0 <= i < end && eligible(nodes[i]),
    ensures exists|i: int| 0 <= i < end && eligible(nodes[i])
        && forall|j: int| 0 <= j < end && eligible(nodes[j]) ==> key(nodes[i]) <= key(nodes[j]),
    decreases end,
{
    if exists|i: int| 0 <= i < end - 1 && eligible(nodes[i]) {
        finite_minimum(nodes, (end - 1) as nat, eligible, key);
        let best = choose|i: int| 0 <= i < (end - 1) && eligible(nodes[i])
            && forall|j: int| 0 <= j < (end - 1) && eligible(nodes[j])
                ==> key(nodes[i]) <= key(nodes[j]);
        let last = end - 1;
        if eligible(nodes[last]) && key(nodes[last]) < key(nodes[best]) {
            assert forall|j: int| 0 <= j < end && eligible(nodes[j])
                implies key(nodes[last]) <= key(nodes[j]) by {
                if j < last { assert(key(nodes[best]) <= key(nodes[j])); }
            }
            assert(0 <= last < end && eligible(nodes[last])
                && forall|j: int| 0 <= j < end && eligible(nodes[j]) ==> key(nodes[last]) <= key(nodes[j]));
        } else {
            assert forall|j: int| 0 <= j < end && eligible(nodes[j])
                implies key(nodes[best]) <= key(nodes[j]) by {
                if j < last { assert(key(nodes[best]) <= key(nodes[j])); }
            }
            assert(0 <= best < end && eligible(nodes[best])
                && forall|j: int| 0 <= j < end && eligible(nodes[j]) ==> key(nodes[best]) <= key(nodes[j]));
        }
    } else {
        let last = end - 1;
        assert(eligible(nodes[last]));
        assert forall|j: int| 0 <= j < end && eligible(nodes[j])
            implies key(nodes[last]) <= key(nodes[j]) by { assert(j == last); }
        assert(0 <= last < end && eligible(nodes[last])
            && forall|j: int| 0 <= j < end && eligible(nodes[j]) ==> key(nodes[last]) <= key(nodes[j]));
    }
}

/// #771: the open-node scratch count depends only on the scanned prefix.
pub(super) proof fn open_prefix_equal(left: Seq<Node>, right: Seq<Node>, end: nat)
    requires end <= left.len(), end <= right.len(),
        forall|j: int| 0 <= j < end ==> left[j] == right[j],
    ensures open_prefix(left, end) == open_prefix(right, end),
    decreases end,
{
    reveal(open_prefix);
    if end > 0 { open_prefix_equal(left, right, (end - 1) as nat); }
}

/// #771: a node update cannot reopen any closed history entry during settle.
pub(super) proof fn open_count_update(nodes: Seq<Node>, index: int, replacement: Node, end: nat)
    requires 0 <= index < end <= nodes.len(),
        nodes[index].phase is Closed ==> replacement.phase is Closed,
    ensures open_prefix(nodes.update(index, replacement), end) <= open_prefix(nodes, end),
    decreases end,
{
    reveal(open_prefix);
    if index < end - 1 {
        open_count_update(nodes, index, replacement, (end - 1) as nat);
    } else {
        open_prefix_equal(nodes.update(index, replacement), nodes, (end - 1) as nat);
    }
}

/// #771: compose the real local postcondition with arbitrary finite sums.
pub(super) proof fn advance_sum_bounds(before: Seq<Node>, after: Seq<Node>, index: int,
    ticket_before: u64, ticket_after: u64, changed: bool)
    requires 0 <= index < before.len(), after == before.update(index, after[index]),
        advance_post(before[index], after[index], ticket_before, ticket_after, changed),
    ensures node_total(after) <= node_total(before),
        potential_prefix(after, after.len(), |n: Node| internal_height(n))
            <= potential_prefix(before, before.len(), |n: Node| internal_height(n)),
        changed ==> potential_prefix(after, after.len(), |n: Node| internal_height(n))
            < potential_prefix(before, before.len(), |n: Node| internal_height(n)),
        ticket_after as int + potential_prefix(after, after.len(), |n: Node| ticket_capacity(n))
            <= ticket_before as int + potential_prefix(before, before.len(), |n: Node| ticket_capacity(n)),
        open_prefix(after, after.len()) <= open_prefix(before, before.len()),
{
    reveal(advance_post);
    sum_update(before, index, after[index]);
    potential_update(before, index, after[index], before.len(), |n: Node| internal_height(n));
    potential_update(before, index, after[index], before.len(), |n: Node| ticket_capacity(n));
    if before[index].phase is Closed {
        assert(internal_height(before[index]) == 0);
        assert(!changed);
    }
    open_count_update(before, index, after[index], before.len());
}

/// #771: closed history contributes no scratch or allocation reserve.
/// The bound depends on open nodes, not the total length of executable storage.
pub(super) proof fn potential_bound(nodes: Seq<Node>, end: nat,
    potential: spec_fn(Node) -> nat, per_open: nat)
    requires end <= nodes.len(),
        forall|i: int| 0 <= i < end ==> potential(nodes[i]) <= per_open
            && (nodes[i].phase is Closed ==> potential(nodes[i]) == 0),
    ensures potential_prefix(nodes, end, potential)
        <= per_open * open_prefix(nodes, end),
    decreases end,
{
    reveal(open_prefix);
    if end > 0 {
        potential_bound(nodes, (end - 1) as nat, potential, per_open);
        let prefix = potential_prefix(nodes, (end - 1) as nat, potential);
        let open = open_prefix(nodes, (end - 1) as nat);
        let value = potential(nodes[end as int - 1]);
        assert(prefix <= per_open * open);
        assert(value <= per_open);
        if nodes[end as int - 1].phase is Closed {
            assert(value == 0);
        } else {
            assert(prefix <= per_open * open && value <= per_open
                ==> prefix + value <= per_open * (open + 1)) by(nonlinear_arith);
        }
    }
}

/// #771: the real local settle contracts consume these finite-world reserves.
pub(super) proof fn settle_reserve_totals(nodes: Seq<Node>)
    ensures
        potential_prefix(nodes, nodes.len(), |n: Node| internal_height(n)) <= 6 * open_prefix(nodes, nodes.len()),
        potential_prefix(nodes, nodes.len(), |n: Node| generation_capacity(n)) <= open_prefix(nodes, nodes.len()),
        potential_prefix(nodes, nodes.len(), |n: Node| effect_capacity(n)) <= open_prefix(nodes, nodes.len()),
        potential_prefix(nodes, nodes.len(), |n: Node| ticket_capacity(n)) <= 2 * open_prefix(nodes, nodes.len()),
        potential_prefix(nodes, nodes.len(), |n: Node| merge_capacity(n)) <= open_prefix(nodes, nodes.len()),
{
    assert forall|i: int| 0 <= i < nodes.len() implies
        internal_height(nodes[i]) <= 6
        && generation_capacity(nodes[i]) <= 1
        && effect_capacity(nodes[i]) <= 1
        && ticket_capacity(nodes[i]) <= 2
        && merge_capacity(nodes[i]) <= 1
        && (nodes[i].phase is Closed ==> internal_height(nodes[i]) == 0
            && generation_capacity(nodes[i]) == 0 && effect_capacity(nodes[i]) == 0
            && ticket_capacity(nodes[i]) == 0 && merge_capacity(nodes[i]) == 0)
    by { local_capacities(nodes[i]); }
    potential_bound(nodes, nodes.len(), |n: Node| internal_height(n), 6);
    potential_bound(nodes, nodes.len(), |n: Node| generation_capacity(n), 1);
    potential_bound(nodes, nodes.len(), |n: Node| effect_capacity(n), 1);
    potential_bound(nodes, nodes.len(), |n: Node| ticket_capacity(n), 2);
    potential_bound(nodes, nodes.len(), |n: Node| merge_capacity(n), 1);
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
            ensures result as int == 4 * node_distance(*self) + pending_rank(*self),
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
            ensures result as int == node_distance(*self), result <= 4096,
        ] proof [reveal(distance); reveal(effect_distance); reveal(current_item);] {
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
                    if let Some(Item::Do(k) | Item::Teardown(k)) = self.current() {
                        if k == e.kind {
                            d -= 2;
                        }
                    }
                }
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

#[cfg(verus_keep_ghost)]
verus! {
/// #771: the subset of consistency needed to bound settle's scratch scans.
/// Cleanup releases the pool at its first step and its slot at Release.
pub(super) open spec fn settle_shape(n: Node) -> bool {
    (n.phase is Closed ==> !n.pool && !n.holds)
        && (n.phase is Stopping && n.pos >= 1 ==> !n.pool)
        && (n.phase is Stopping && n.pos >= 4 ==> !n.holds)
}

/// Q30/Q39 / #806: expose the exact mathematical ranks to the independently
/// gated real slot transition, without trusting an opaque rank identity.
pub(super) proof fn rank_definitions(n: Node)
    ensures node_distance(n) == distance(n), pending_rank(n) == {

    if n.phase is Closed { 0nat }
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
    },
{
}

/// #771: exact enabling condition of the executable local dispatcher.
/// This observes the agenda and scheduler decisions; it defines no replacement
/// transition relation and is checked against every production case body.
pub(super) open spec fn advance_enabled(n: Node, context: AdvanceContext) -> bool {
    if n.phase is Closed { false }
    else if n.waiting_incident { n.ticket.is_some() }
    else if n.effect.is_some() { false }
    else { match current_item(n) {
        None => n.phase == Phase::Publishing || (n.phase is Stopping && n.question.is_none()),
        Some(Item::Do(_) | Item::StartChecks | Item::StartRebase) =>
            !n.paused || holding(n.phase) || n.ticket.is_some(),
        Some(Item::Start(_)) => if n.paused { holding(n.phase) || n.ticket.is_some() }
            else { n.ticket.is_none() || context.pool },
        Some(Item::Admit) => !n.paused && context.admit,
        Some(Item::Slot) => n.phase is Ready && if n.paused { n.ticket.is_some() }
            else { n.ticket.is_none() || context.slot },
        Some(Item::Release) => true,
        Some(Item::Teardown(_)) => n.question.is_none(),
        Some(Item::Await | Item::Merge | Item::Answer) => false,
    } }
}

pub(super) open spec fn advance_pre(n: Node, ticket: u64, out_len: usize) -> bool {
    out_len as int + 2 <= OUTBOX as int
        && n.generation as int + generation_capacity(n) <= u64::MAX as int
        && n.next_seq as int + effect_capacity(n) <= u64::MAX as int
        && ticket as int + ticket_capacity(n) <= u64::MAX as int
        && n.credit.earned as int + merge_capacity(n) <= u64::MAX as int
}

/// #771: bounds proved for the actual local transition. This is a postcondition,
/// not an alternative transition relation.
#[verifier::opaque]
pub(super) open spec fn advance_post(before: Node, after: Node,
    ticket_before: u64, ticket_after: u64, changed: bool) -> bool {
    (changed ==> internal_height(after) < internal_height(before))
    && after.last_cmd == before.last_cmd
    && after.repo == before.repo && after.footprint == before.footprint
    && (settle_shape(before) ==> settle_shape(after))
    && (before.phase is Closed ==> after.phase is Closed)
    && distance(after) <= distance(before)
    && (changed ==> 4 * distance(after) + pending_rank(after)
        < 4 * distance(before) + pending_rank(before))
    && after.generation as int + generation_capacity(after)
        <= before.generation as int + generation_capacity(before)
    && after.next_seq as int + effect_capacity(after)
        <= before.next_seq as int + effect_capacity(before)
    && ticket_after as int + ticket_capacity(after)
        <= ticket_before as int + ticket_capacity(before)
    && after.credit.earned as int + merge_capacity(after)
        <= before.credit.earned as int + merge_capacity(before)
    && after.credit.start == before.credit.start
    && after.credit.added == before.credit.added
    && after.credit.spent == before.credit.spent
    && (!changed ==> after == before && ticket_after == ticket_before)
}
}

kernel_fn! {
    /// #771: same executable transition, partitioned for bounded solver queries.
    fn advance_blocked(node: &mut Node, _next_ticket: &mut u64,
        _context: AdvanceContext, _out: &mut Outbox) -> (result: bool) [
        requires advance_pre(*old(node), *old(_next_ticket), old(_out).len),
            old(node).phase is Closed || old(node).waiting_incident || old(node).effect.is_some(),
        ensures (!result ==> final(_out).len == old(_out).len),
            advance_post(*old(node), *final(node),
            *old(_next_ticket), *final(_next_ticket), result),
            result == advance_enabled(*old(node), _context),
    ] proof [reveal(advance_post); reveal(distance);] {
        let n = *node;
        if n.phase.is_closed() {
            return false;
        }
        if n.waiting_incident {
            if n.ticket.is_some() {
                node.ticket = None;
                return true;
            }
            return false;
        }
        if n.effect.is_some() {
            return false; // one effect in flight at a time
        }
        false
    }
}

kernel_fn! {
    /// #771: same executable transition, partitioned for bounded solver queries.
    fn advance_finished(node: &mut Node, _next_ticket: &mut u64,
        context: AdvanceContext, out: &mut Outbox) -> (result: bool) [
        requires advance_pre(*old(node), *old(_next_ticket), old(out).len),
            !(old(node).phase is Closed) && !old(node).waiting_incident && old(node).effect.is_none() && current_item(*old(node)).is_none(),
        ensures (!result ==> final(out).len == old(out).len),
            advance_post(*old(node), *final(node),
            *old(_next_ticket), *final(_next_ticket), result),
            result == advance_enabled(*old(node), context),
            progress_gate_leaf_steps::advance_details(*old(node), *final(node),
            *old(_next_ticket), *final(_next_ticket)),
    ] proof [reveal(advance_post); reveal(distance); reveal(progress_gate_leaf_steps::advance_details);] {
        let n = *node;
        match n.phase {
            Phase::Publishing => {
                set_phase(node, Phase::Queued);
                true
            }
            Phase::Stopping(outcome) if n.question.is_none() => {
                close_node(node, context.node, outcome, out);
                true
            }
            _ => false,
        }
    }
}

kernel_fn! {
    /// #771: same executable transition, partitioned for bounded solver queries.
    fn advance_do(node: &mut Node, _next_ticket: &mut u64,
        context: AdvanceContext, out: &mut Outbox, kind: EffectKind) -> (result: bool) [
        requires advance_pre(*old(node), *old(_next_ticket), old(out).len),
            !(old(node).phase is Closed) && !old(node).waiting_incident && old(node).effect.is_none() && current_item(*old(node)) == Some(Item::Do(kind)),
        ensures (!result ==> final(out).len == old(out).len),
            advance_post(*old(node), *final(node),
            *old(_next_ticket), *final(_next_ticket), result),
            result == advance_enabled(*old(node), context),
            progress_gate_leaf_steps::advance_details(*old(node), *final(node),
            *old(_next_ticket), *final(_next_ticket)),
    ] proof [reveal(advance_post); reveal(distance); reveal(progress_gate_leaf_steps::advance_details);] {
        let n = *node;
        if n.paused {
            return pause_node(node);
        }
        emit_node(node, context.node, kind, out);
        true
    }
}

kernel_fn! {
    /// #771: same executable transition, partitioned for bounded solver queries.
    fn advance_start(node: &mut Node, next_ticket: &mut u64,
        context: AdvanceContext, out: &mut Outbox, role: Role) -> (result: bool) [
        requires advance_pre(*old(node), *old(next_ticket), old(out).len),
            !(old(node).phase is Closed) && !old(node).waiting_incident && old(node).effect.is_none() && current_item(*old(node)) == Some(Item::Start(role)),
        ensures (!result ==> final(out).len == old(out).len),
            advance_post(*old(node), *final(node),
            *old(next_ticket), *final(next_ticket), result),
            result == advance_enabled(*old(node), context),
    ] proof [reveal(advance_post); reveal(distance);] {
        let n = *node;
        if n.paused {
            return pause_node(node);
        }
        if n.ticket.is_none() {
            take_ticket_node(node, next_ticket);
            return true;
        }
        if context.pool {
            let m = &mut *node;
            m.ticket = None;
            m.pool = true;
            m.run = Some(m.generation);
            m.pos += 1;
            emit_node(node, context.node, EffectKind::StartRun(role), out);
            return true;
        }
        false
    }
}

kernel_fn! {
    /// #771: same executable transition, partitioned for bounded solver queries.
    fn advance_checks(node: &mut Node, _next_ticket: &mut u64,
        context: AdvanceContext, out: &mut Outbox) -> (result: bool) [
        requires advance_pre(*old(node), *old(_next_ticket), old(out).len),
            !(old(node).phase is Closed) && !old(node).waiting_incident && old(node).effect.is_none() && current_item(*old(node)) == Some(Item::StartChecks),
        ensures (!result ==> final(out).len == old(out).len),
            advance_post(*old(node), *final(node),
            *old(_next_ticket), *final(_next_ticket), result),
            result == advance_enabled(*old(node), context),
    ] proof [reveal(advance_post); reveal(distance);] {
        let n = *node;
        if n.paused {
            return pause_node(node);
        }
        let m = &mut *node;
        m.job = Some(m.generation);
        m.pos += 1;
        emit_node(node, context.node, EffectKind::SubmitChecks, out);
        true
    }
}

kernel_fn! {
    /// #771: same executable transition, partitioned for bounded solver queries.
    fn advance_rebase(node: &mut Node, _next_ticket: &mut u64,
        context: AdvanceContext, out: &mut Outbox) -> (result: bool) [
        requires advance_pre(*old(node), *old(_next_ticket), old(out).len),
            !(old(node).phase is Closed) && !old(node).waiting_incident && old(node).effect.is_none() && current_item(*old(node)) == Some(Item::StartRebase),
        ensures (!result ==> final(out).len == old(out).len),
            advance_post(*old(node), *final(node),
            *old(_next_ticket), *final(_next_ticket), result),
            result == advance_enabled(*old(node), context),
    ] proof [reveal(advance_post); reveal(distance);] {
        let n = *node;
        if n.paused {
            return pause_node(node);
        }
        let m = &mut *node;
        m.run = Some(m.generation);
        m.pos += 1;
        emit_node(node, context.node, EffectKind::StartRebase, out);
        true
    }
}

kernel_fn! {
    /// #771: same executable transition, partitioned for bounded solver queries.
    fn advance_admit(node: &mut Node, _next_ticket: &mut u64,
        context: AdvanceContext, _out: &mut Outbox) -> (result: bool) [
        requires advance_pre(*old(node), *old(_next_ticket), old(_out).len),
            !(old(node).phase is Closed) && !old(node).waiting_incident && old(node).effect.is_none() && current_item(*old(node)) == Some(Item::Admit),
        ensures (!result ==> final(_out).len == old(_out).len),
            advance_post(*old(node), *final(node),
            *old(_next_ticket), *final(_next_ticket), result),
            result == advance_enabled(*old(node), context),
    ] proof [reveal(advance_post); reveal(distance);] {
        let n = *node;
        if n.paused || !context.admit {
            return false;
        }
        let m = &mut *node;
        m.credit.withheld = false;
        m.planning_started = true;
        set_phase(m, Phase::Planning { must_split: false });
        true
    }
}

kernel_fn! {
    /// #771: same executable transition, partitioned for bounded solver queries.
    fn advance_release(node: &mut Node, _next_ticket: &mut u64,
        _context: AdvanceContext, _out: &mut Outbox) -> (result: bool) [
        requires advance_pre(*old(node), *old(_next_ticket), old(_out).len),
            !(old(node).phase is Closed) && !old(node).waiting_incident && old(node).effect.is_none() && current_item(*old(node)) == Some(Item::Release),
        ensures (!result ==> final(_out).len == old(_out).len),
            advance_post(*old(node), *final(node),
            *old(_next_ticket), *final(_next_ticket), result),
            result == advance_enabled(*old(node), _context),
            progress_gate_leaf_steps::advance_details(*old(node), *final(node),
            *old(_next_ticket), *final(_next_ticket)),
    ] proof [reveal(advance_post); reveal(distance); reveal(progress_gate_leaf_steps::advance_details);] {
        let m = &mut *node;
        m.holds = false;
        m.pos += 1;
        true
    }
}

kernel_fn! {
    /// #771: same executable transition, partitioned for bounded solver queries.
    #[cfg_attr(verus_keep_ghost, verifier::rlimit(90))]
    fn advance_teardown(node: &mut Node, _next_ticket: &mut u64,
        context: AdvanceContext, out: &mut Outbox, kind: EffectKind) -> (result: bool) [
        requires advance_pre(*old(node), *old(_next_ticket), old(out).len),
            !(old(node).phase is Closed) && !old(node).waiting_incident && old(node).effect.is_none() && current_item(*old(node)) == Some(Item::Teardown(kind)),
        ensures (!result ==> final(out).len == old(out).len),
            advance_post(*old(node), *final(node),
            *old(_next_ticket), *final(_next_ticket), result),
            result == advance_enabled(*old(node), context),
            progress_gate_leaf_steps::advance_details(*old(node), *final(node),
            *old(_next_ticket), *final(_next_ticket)),
    ] {
        let n = *node;
        if n.question.is_some() {
            kernel_proof! {
                progress_gate_leaf_steps::stutter_rank(n, *_next_ticket);
                progress_gate_leaf_steps::inactive_details(n, *node, *_next_ticket, *_next_ticket);
            }
            return false; // waiting for Rob
        }
        kernel_proof! { progress_gate_leaf_steps::teardown_item(n, kind); }
        let gone = match kind {
            EffectKind::StopRun => n.run.is_none(),
            EffectKind::WithdrawJob => n.job.is_none(),
            EffectKind::ClosePr => !n.pr_open,
            EffectKind::RemoveWorktree => !n.worktree,
            _ => false,
        };
        if gone {
            let m = &mut *node;
            if kind == EffectKind::StopRun {
                m.pool = false;
            }
            m.pos += 1;
            kernel_proof! {
                progress_gate_leaf_steps::skipped_rank(n, *node, kind, *_next_ticket);
                progress_gate_leaf_steps::skipped_details(n, *node, kind, *_next_ticket);
            }
            return true;
        }
        emit_node(node, context.node, kind, out);
        kernel_proof! {
            progress_gate_leaf_steps::emitted_rank(n, *node, kind, *_next_ticket);
            progress_gate_leaf_steps::emitted_details(n, *node, kind, *_next_ticket);
        }
        true
    }
}

kernel_fn! {
    /// #771: same executable transition, partitioned for bounded solver queries.
    fn advance_idle(_node: &mut Node, _next_ticket: &mut u64,
        _context: AdvanceContext, _out: &mut Outbox) -> (result: bool) [
        requires advance_pre(*old(_node), *old(_next_ticket), old(_out).len),
            !(old(_node).phase is Closed) && !old(_node).waiting_incident && old(_node).effect.is_none() && matches!(current_item(*old(_node)), Some(Item::Await | Item::Merge | Item::Answer)),
        ensures (!result ==> final(_out).len == old(_out).len),
            advance_post(*old(_node), *final(_node),
            *old(_next_ticket), *final(_next_ticket), result),
            result == advance_enabled(*old(_node), _context),
    ] proof [reveal(advance_post); reveal(distance);] {
        false
    }
}

kernel_fn! {
    /// #771: same executable transition, partitioned for bounded solver queries.
    pub(super) fn advance_node(node: &mut Node, next_ticket: &mut u64,
        context: AdvanceContext, out: &mut Outbox) -> (result: bool) [
        requires advance_pre(*old(node), *old(next_ticket), old(out).len),
        ensures (!result ==> final(out).len == old(out).len),
            advance_post(*old(node), *final(node),
            *old(next_ticket), *final(next_ticket), result),
            result == advance_enabled(*old(node), context),
            (old(node).phase == Phase::Ready(Resume::Making) && old(node).pos == 0 && !old(node).waiting_incident && old(node).effect.is_none()) ==> {
                *final(node) == retirement_gate::slot_node(*old(node), *old(next_ticket), context.slot)
                    && *final(next_ticket) == retirement_gate::slot_ticket(*old(node), *old(next_ticket))
                    && *final(out) == *old(out)
                    && result == retirement_gate::slot_changed(*old(node), context.slot)
            },
            progress_gate_leaf_steps::advance_details(*old(node), *final(node),
            *old(next_ticket), *final(next_ticket)),
    ] {
        kernel_proof! { reveal(progress_gate_leaf_steps::advance_details); }
        let n = *node;
        if n.phase.is_closed() || n.waiting_incident || n.effect.is_some() {
            return advance_blocked(node, next_ticket, context, out);
        }
        let Some(item) = n.current() else {
            return advance_finished(node, next_ticket, context, out);
        };
        match item {
            Item::Do(kind) => advance_do(node, next_ticket, context, out, kind),
            Item::Start(role) => advance_start(node, next_ticket, context, out, role),
            Item::StartChecks => advance_checks(node, next_ticket, context, out),
            Item::StartRebase => advance_rebase(node, next_ticket, context, out),
            Item::Await | Item::Merge | Item::Answer => advance_idle(node, next_ticket, context, out),
            Item::Admit => advance_admit(node, next_ticket, context, out),
            Item::Slot => advance_slot(node, next_ticket, context, out),
            Item::Release => advance_release(node, next_ticket, context, out),
            Item::Teardown(kind) => advance_teardown(node, next_ticket, context, out, kind),
        }
    }
}
