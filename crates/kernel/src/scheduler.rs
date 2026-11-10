//! Q30/Q39 / #805: real scheduler queries and dispatcher.
use super::*;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

#[cfg(verus_keep_ghost)]
verus! {
/// K4c gate slot: slot holders in `repo` among the first `end` nodes, other than `skip`,
/// counted exactly as the executable slot scan counts them.
pub(super) open spec fn other_holders(nodes: Seq<Node>, end: nat, repo: RepoId, skip: int) -> nat
    decreases end,
{
    if end == 0 { 0 } else {
        other_holders(nodes, (end - 1) as nat, repo, skip)
            + if end - 1 != skip && nodes[end - 1].holds && nodes[end - 1].repo == repo { 1nat } else { 0nat }
    }
}

/// K4c gate slot: exactly when the executable slot scan grants node i a slot:
/// a valid repository, room in its width, and no overlapping other holder.
pub(super) open spec fn slot_spec(w: World, i: int) -> bool {
    w.nodes@[i].repo.0 < w.gates@.len()
        && other_holders(w.nodes@, w.nodes@.len(), w.nodes@[i].repo, i)
            < w.gates@[w.nodes@[i].repo.0 as int].w
        && forall|j: int| 0 <= j < w.nodes@.len() && j != i && w.nodes@[j].holds
            && w.nodes@[j].repo == w.nodes@[i].repo
            ==> footprint::disjoint(w, w.nodes@[j].footprint, w.nodes@[i].footprint)
}

/// K4c gate fairness: some same-repository slot waiter older than `ticket`
/// needs a file node i also needs.
pub(super) open spec fn overlapped_by_older(w: World, i: int, ticket: u64) -> bool {
    exists|k: int| 0 <= k < w.nodes@.len() && w.nodes@[k].phase is Ready
        && w.nodes@[k].repo == w.nodes@[i].repo && w.nodes@[k].ticket.is_some()
        && w.nodes@[k].ticket.unwrap() < ticket
        && !footprint::disjoint(w, w.nodes@[k].footprint, w.nodes@[i].footprint)
}
}

kernel_fn! {
/// #771: source-connected scheduler wrapper. Context queries are read at the
/// original branches; the verified local transition writes only this node.
pub(super) fn advance(w: &mut World, i: usize, out: &mut Outbox, report: &mut Report) -> (result: bool) [
    requires i < old(w).nodes@.len(), i <= u64::MAX as int,
        scheduling_safe(*old(w)),
        advance_pre(old(w).nodes@[i as int], old(w).next_ticket, old(out).len),
    ensures report::no_new_stop(old(w).nodes@[i as int].phase, final(w).nodes@[i as int].phase,
            old(report).entries@, final(report).entries@),
        (!result ==> final(out).len == old(out).len),
        advance_post(old(w).nodes@[i as int], final(w).nodes@[i as int],
            old(w).next_ticket, final(w).next_ticket, result),
        final(w).nodes@ == old(w).nodes@.update(i as int, final(w).nodes@[i as int]),
        final(w).next_question == old(w).next_question,
        final(w).gates@ == old(w).gates@,
        final(w).pool_capacity == old(w).pool_capacity,
        final(w).fail_fast == old(w).fail_fast,
        final(w).file_sets@ == old(w).file_sets@,
        final(w).dependencies@ == old(w).dependencies@,
        movable(*old(w), i as int) ==> result,
        node_total(final(w).nodes@) <= node_total(old(w).nodes@),
        potential_prefix(final(w).nodes@, final(w).nodes@.len(), |n: Node| internal_height(n))
            <= potential_prefix(old(w).nodes@, old(w).nodes@.len(), |n: Node| internal_height(n)),
        result ==> potential_prefix(final(w).nodes@, final(w).nodes@.len(), |n: Node| internal_height(n))
            < potential_prefix(old(w).nodes@, old(w).nodes@.len(), |n: Node| internal_height(n)),
        final(w).next_ticket as int + potential_prefix(final(w).nodes@, final(w).nodes@.len(), |n: Node| ticket_capacity(n))
            <= old(w).next_ticket as int + potential_prefix(old(w).nodes@, old(w).nodes@.len(), |n: Node| ticket_capacity(n)),
        open_prefix(final(w).nodes@, final(w).nodes@.len())
            <= open_prefix(old(w).nodes@, old(w).nodes@.len()),
        (forall|j: int| 0 <= j < old(w).nodes@.len() ==> settle_shape(old(w).nodes@[j]))
            ==> scheduling_safe(*final(w))
                && (forall|j: int| 0 <= j < final(w).nodes@.len() ==> settle_shape(final(w).nodes@[j])),
        final(w).nodes@[i as int].last_cmd == old(w).nodes@[i as int].last_cmd,
        old(w).nodes@[i as int].phase == Phase::Unplanned && old(w).nodes@[i as int].pos == 0
            && old(w).nodes@[i as int].ticket.is_none() ==> {
                (result ==> final(w).nodes@[i as int].phase == (Phase::Planning { must_split: false }))
                && result == (!old(w).nodes@[i as int].paused
                    && !old(w).nodes@[i as int].waiting_incident && old(w).nodes@[i as int].effect.is_none()
                    && dependencies::delivered(old(w).dependencies@, NodeId(i as u64))
                    && old(w).nodes@[i as int].repo.0 < old(w).gates@.len()
                    && slot_waiters(old(w).nodes@, old(w).nodes@.len(), old(w).nodes@[i as int].repo)
                        < old(w).gates@[old(w).nodes@[i as int].repo.0 as int].w)
            },
        old(w).next_ticket <= final(w).next_ticket,
        advance_family_post::ready_unobstructed(old(w).nodes@[i as int])
            && slot_spec(*old(w), i as int) && oldest_waiter(*old(w), i as int)
            ==> result && final(w).nodes@[i as int].holds && final(w).nodes@[i as int].ticket.is_none(),
        advance_family_post::ready_unobstructed(old(w).nodes@[i as int]) && !result
            ==> final(w).nodes@[i as int] == old(w).nodes@[i as int],
        advance_family_post::ready_unobstructed(old(w).nodes@[i as int]) && result
            ==> slot_spec(*old(w), i as int),
        advance_family_post::ready_unobstructed(old(w).nodes@[i as int])
            ==> forall|older: int| 0 <= older < old(w).nodes@.len()
                && old(w).nodes@[older].phase is Ready
                && old(w).nodes@[older].repo == old(w).nodes@[i as int].repo
                && old(w).nodes@[older].ticket.is_some()
                && old(w).nodes@[older].ticket.unwrap() < old(w).nodes@[i as int].ticket.unwrap()
                && slot_spec(*old(w), older)
                && !overlapped_by_older(*old(w), older, old(w).nodes@[older].ticket.unwrap())
                ==> !result && final(w).nodes@[i as int] == old(w).nodes@[i as int],
        advance_family_post::start_unobstructed(old(w).nodes@[i as int])
            && pool_users(old(w).nodes@, old(w).nodes@.len()) < old(w).pool_capacity
            && oldest_waiter(*old(w), i as int)
            ==> result && final(w).nodes@[i as int].pool
                && final(w).nodes@[i as int].ticket.is_none()
                && final(w).nodes@[i as int].run == Some(old(w).nodes@[i as int].generation),
        advance_family_post::start_unobstructed(old(w).nodes@[i as int]) && !result
            ==> final(w).nodes@[i as int] == old(w).nodes@[i as int],
        advance_family_post::start_unobstructed(old(w).nodes@[i as int]) && result
            ==> pool_users(old(w).nodes@, old(w).nodes@.len()) < old(w).pool_capacity
                && oldest_waiter(*old(w), i as int),
        !old(w).nodes@[i as int].holds && final(w).nodes@[i as int].holds
            ==> !old(w).nodes@[i as int].paused && slot_spec(*old(w), i as int)
                && final(w).nodes@[i as int].repo == old(w).nodes@[i as int].repo
                && final(w).nodes@[i as int].footprint == old(w).nodes@[i as int].footprint
                && final(w).nodes@[i as int].ticket.is_none()
                && final(w).nodes@[i as int].pool == old(w).nodes@[i as int].pool
                && final(w).nodes@[i as int].run != old(w).nodes@[i as int].run,
        !old(w).nodes@[i as int].pool && final(w).nodes@[i as int].pool
            ==> pool_users(old(w).nodes@, old(w).nodes@.len()) < old(w).pool_capacity
                && oldest_waiter(*old(w), i as int)
                && !old(w).nodes@[i as int].paused && old(w).nodes@[i as int].ticket.is_some()
                && final(w).nodes@[i as int].ticket.is_none(),
] {
    kernel_snapshot! { before_advance = *w }
    kernel_proof! { dependencies::prerequisite_definition(*w, NodeId(i as u64)); }
    kernel_proof! {
        if advance_family_post::start_unobstructed(w.nodes@[i as int]) {
            advance_family_post::start_item(w.nodes@[i as int]);
        }
    }
    let n = &w.nodes[i];
    let mut context = AdvanceContext {
        node: i,
        admit: false,
        pool: false,
        slot: false,
    };
    if !n.phase.is_closed() && !n.waiting_incident && n.effect.is_none() {
        match n.current() {
            Some(Item::Admit) if !n.paused => {
                context.admit = w.dependencies_delivered(NodeId(i as u64)) && admits(w, n.repo);
            }
            Some(Item::Start(_)) if !n.paused && n.ticket.is_some() => {
                context.pool = pool_free(w) && first_in_line(w, i);
            }
            Some(Item::Slot)
                if matches!(n.phase, Phase::Ready(_)) && !n.paused && n.ticket.is_some() =>
            {
                context.slot = slot_free(w, i) && first_in_line(w, i);
            }
            _ => {}
        }
    }
    kernel_proof! {
        assert(movable(*w, i as int) ==> advance_enabled(w.nodes@[i as int], context));
    }
    let changed = advance_node(&mut w.nodes[i], &mut w.next_ticket, context, out, report);
    kernel_proof! {
        reveal(advance_family_post::grant_facts);
        reveal(advance_family_post::pool_grant);
        reveal(advance_family_post::slot_grant);
        reveal(advance_post);
        advance_sum_bounds(before_advance@.nodes@, w.nodes@, i as int,
            before_advance@.next_ticket, w.next_ticket, changed);
        if forall|j: int| 0 <= j < before_advance@.nodes@.len()
            ==> settle_shape(before_advance@.nodes@[j]) {
            assert forall|j: int| 0 <= j < w.nodes@.len()
                implies footprint::valid_footprint(*w, w.nodes@[j].footprint) by {
                footprint::valid_under_table_growth(before_advance@, *w, w.nodes@[j].footprint);
            }
            assert forall|j: int| 0 <= j < w.nodes@.len() implies settle_shape(w.nodes@[j]) by {}
            reveal(footprint::ordered_table);
        }
    }
    changed
}
}

kernel_fn! {
/// Q39 / #771: exact pool occupancy, with the checked open-node scan bound.
pub(super) fn pool_free(w: &World) -> (result: bool) [
    requires open_prefix(w.nodes@, w.nodes@.len()) <= u32::MAX as int,
        forall|i: int| 0 <= i < w.nodes@.len()
            ==> (w.nodes@[i].pool ==> !(w.nodes@[i].phase is Closed)),
    ensures result == (pool_users(w.nodes@, w.nodes@.len()) < w.pool_capacity),
] {
    let mut runs = 0u32;
    let mut i = 0usize;
    kernel_while! { (i < w.nodes.len()) [
        invariant
            i <= w.nodes.len(),
            runs as int == pool_users(w.nodes@, i as nat),
            runs as int <= open_prefix(w.nodes@, i as nat),
            open_prefix(w.nodes@, w.nodes@.len()) <= u32::MAX as int,
            forall|j: int| 0 <= j < w.nodes@.len()
                ==> (w.nodes@[j].pool ==> !(w.nodes@[j].phase is Closed)),
        decreases w.nodes.len() - i,
    ] {
        let n = &w.nodes[i];
        kernel_proof! {
            reveal(open_prefix);
            open_prefix_monotone(w.nodes@, i as nat + 1, w.nodes@.len());
        }
        if n.pool {
            runs += 1;
        }
        i += 1;
    } }
    runs < w.pool_capacity as u32
}
}

kernel_fn! {
/// Q30 / #771: planning admission uses the actual slot-waiter count.
pub(super) fn admits(w: &World, repo: RepoId) -> (result: bool) [
    requires open_prefix(w.nodes@, w.nodes@.len()) <= u32::MAX as int,
    ensures result == if (repo.0 as int) < w.gates@.len() {
        slot_waiters(w.nodes@, w.nodes@.len(), repo) < w.gates@[repo.0 as int].w
    } else { false },
] {
    kernel_proof! { vstd::std_specs::vec::axiom_spec_len(&w.gates); }
    let Some(r) = repo.index() else {
        return false;
    };
    let Some(gate) = w.gates.get(r) else {
        return false;
    };
    let mut waiting = 0u32;
    let mut i = 0usize;
    kernel_while! { (i < w.nodes.len()) [
        invariant
            i <= w.nodes.len(),
            waiting as int == slot_waiters(w.nodes@, i as nat, repo),
            waiting as int <= open_prefix(w.nodes@, i as nat),
            open_prefix(w.nodes@, w.nodes@.len()) <= u32::MAX as int,
            r < w.gates.len(), r as int == repo.0 as int,
            *gate == w.gates@[r as int],
        decreases w.nodes.len() - i,
    ] {
        let n = &w.nodes[i];
        kernel_proof! {
            reveal(open_prefix);
            open_prefix_monotone(w.nodes@, i as nat + 1, w.nodes@.len());
        }
        if n.repo == repo && matches!(n.phase, Phase::Ready(_)) && n.ticket.is_some() {
            waiting += 1;
        }
        i += 1;
    } }
    waiting < gate.w as u32
}
}

kernel_fn! {
/// Q30 / #771: a request has a free slot when no other request holds one.
/// The full scan still rejects overlaps and saturated widths at their old sites.
pub(super) fn slot_free(w: &World, i: usize) -> (result: bool) [
    requires i < w.nodes@.len(), scheduling_safe(*w),
    ensures (w.nodes@[i as int].repo.0 < w.gates@.len()
        && w.gates@[w.nodes@[i as int].repo.0 as int].w > 0
        && (forall|j: int| 0 <= j < w.nodes@.len() && j != i ==> !w.nodes@[j].holds))
        ==> result,
        // K4c gate slot: a free slot leaves room in the width and overlaps no other holder.
        result ==> w.nodes@[i as int].repo.0 < w.gates@.len()
            && other_holders(w.nodes@, w.nodes@.len(), w.nodes@[i as int].repo, i as int)
                < w.gates@[w.nodes@[i as int].repo.0 as int].w
            && forall|j: int| 0 <= j < w.nodes@.len() && j != i && w.nodes@[j].holds
                && w.nodes@[j].repo == w.nodes@[i as int].repo
                ==> footprint::disjoint(*w, w.nodes@[j].footprint, w.nodes@[i as int].footprint),
        result == slot_spec(*w, i as int),
] {
    let me = &w.nodes[i];
    let _gate_count = w.gates.len();
    let Some(repo) = me.repo.index() else { return false; };
    let Some(gate) = w.gates.get(repo) else { return false; };
    let mut holders = 0u32;
    let mut j = 0usize;
    kernel_while! { (j < w.nodes.len()) [
        invariant j <= w.nodes.len(), i < w.nodes.len(), scheduling_safe(*w),
            *me == w.nodes@[i as int], repo < w.gates.len(),
            repo as int == me.repo.0 as int, *gate == w.gates@[repo as int],
            holders as int <= open_prefix(w.nodes@, j as nat),
            ((forall|k: int| 0 <= k < w.nodes@.len() && k != i ==> !w.nodes@[k].holds)
                ==> holders == 0),
            holders as int == other_holders(w.nodes@, j as nat, me.repo, i as int),
            forall|k: int| 0 <= k < j && k != i && w.nodes@[k].holds && w.nodes@[k].repo == me.repo
                ==> footprint::disjoint(*w, w.nodes@[k].footprint, me.footprint),
        decreases w.nodes.len() - j,
    ] {
        let n = &w.nodes[j];
        kernel_proof! {
            reveal(open_prefix);
            open_prefix_monotone(w.nodes@, j as nat + 1, w.nodes@.len());
        }
        if j != i && n.holds && n.repo == me.repo {
            if !w.footprints_disjoint(n.footprint, me.footprint) {
                return false;
            }
            holders += 1;
        }
        j += 1;
    } }
    holders < gate.w as u32
}
}

kernel_fn! {
/// Q39 / #771: the declared priority order followed by the older pool ticket.
pub(super) fn earlier_in_pool(a: &Node, b: &Node) -> (result: bool) [
    ensures result == pool_precedes(*a, *b),
] {
    if matches!(a.phase, Phase::Ready(_)) { return false; }
    match (a.ticket, b.ticket) {
        (Some(theirs), Some(mine)) => a.priority.rank() < b.priority.rank()
            || (a.priority == b.priority && theirs < mine),
        _ => false,
    }
}
}

kernel_fn! {
/// Q30 / #771: this is the original inner overlap scan, with explicit indices.
// The pinned Verus frontend requires nested if-let instead of let chains.
#[allow(clippy::collapsible_if)]
pub(super) fn older_overlap(w: &World, i: usize, ticket: u64) -> (result: bool) [
    requires i < w.nodes@.len(), scheduling_safe(*w),
    ensures result == overlapped_by_older(*w, i as int, ticket),
] {
    let waiter = &w.nodes[i];
    let mut index = 0usize;
    kernel_while! { (index < w.nodes.len()) [
        invariant index <= w.nodes.len(), i < w.nodes.len(), scheduling_safe(*w),
            *waiter == w.nodes@[i as int],
            forall|k: int| 0 <= k < index ==> !(w.nodes@[k].phase is Ready
                && w.nodes@[k].repo == waiter.repo && w.nodes@[k].ticket.is_some()
                && w.nodes@[k].ticket.unwrap() < ticket
                && !footprint::disjoint(*w, w.nodes@[k].footprint, waiter.footprint)),
        decreases w.nodes.len() - index,
    ] {
        let older = &w.nodes[index];
        if matches!(older.phase, Phase::Ready(_)) && older.repo == waiter.repo {
            if let Some(theirs) = older.ticket {
                if theirs < ticket && !w.footprints_disjoint(older.footprint, waiter.footprint) {
                    return true;
                }
            }
        }
        index += 1;
    } }
    false
}
}

kernel_fn! {
/// Older slot waiters block runnable or overlapping younger leaves.
/// Q30/Q39 / #771: the oldest waiter is first in its actual executable line.
pub(super) fn first_in_line(w: &World, i: usize) -> (result: bool) [
    requires i < w.nodes@.len(), scheduling_safe(*w),
    ensures oldest_waiter(*w, i as int) ==> result,
        result ==> w.nodes@[i as int].ticket.is_some(),
        // K4c pool order: the pool line is exactly the declared priority/ticket order.
        !(w.nodes@[i as int].phase is Ready) ==> result == oldest_waiter(*w, i as int),
        // K4c gate fairness: no grant passes an older overlapping same-repository waiter.
        result && w.nodes@[i as int].phase is Ready ==> forall|j: int| 0 <= j < w.nodes@.len()
            && w.nodes@[j].phase is Ready && w.nodes@[j].repo == w.nodes@[i as int].repo
            && w.nodes@[j].ticket.is_some()
            && w.nodes@[j].ticket.unwrap() < w.nodes@[i as int].ticket.unwrap()
            ==> footprint::disjoint(*w, w.nodes@[j].footprint, w.nodes@[i as int].footprint),
        // K4c gate fairness: no grant passes an older same-repository waiter that could
        // itself take a slot now (free slot, no older overlapping waiter of its own).
        result && w.nodes@[i as int].phase is Ready ==> forall|j: int| 0 <= j < w.nodes@.len()
            && w.nodes@[j].phase is Ready && w.nodes@[j].repo == w.nodes@[i as int].repo
            && w.nodes@[j].ticket.is_some()
            && w.nodes@[j].ticket.unwrap() < w.nodes@[i as int].ticket.unwrap()
            ==> !(slot_spec(*w, j) && !overlapped_by_older(*w, j, w.nodes@[j].ticket.unwrap())),
] {
    let me = &w.nodes[i];
    let Some(mine) = me.ticket else { return false };
    if !matches!(me.phase, Phase::Ready(_)) {
        let mut index = 0usize;
        kernel_while! { (index < w.nodes.len()) [
            invariant index <= w.nodes.len(), i < w.nodes.len(),
                *me == w.nodes@[i as int], !(me.phase is Ready), me.ticket == Some(mine),
                forall|k: int| 0 <= k < index ==> !pool_precedes(w.nodes@[k], w.nodes@[i as int]),
            decreases w.nodes.len() - index,
        ] {
            if earlier_in_pool(&w.nodes[index], me) { return false; }
            index += 1;
        } }
        return true;
    }
    let mut first_runnable = mine;
    let mut j = 0usize;
    kernel_while! { (j < w.nodes.len()) [
        invariant j <= w.nodes.len(), i < w.nodes.len(), scheduling_safe(*w),
            *me == w.nodes@[i as int], me.phase is Ready, me.ticket == Some(mine),
            oldest_waiter(*w, i as int) ==> first_runnable == mine,
            forall|k: int| 0 <= k < j && w.nodes@[k].phase is Ready
                && w.nodes@[k].repo == me.repo && w.nodes@[k].ticket.is_some()
                && w.nodes@[k].ticket.unwrap() < mine
                ==> footprint::disjoint(*w, w.nodes@[k].footprint, me.footprint),
            first_runnable <= mine,
            forall|k: int| 0 <= k < j && w.nodes@[k].phase is Ready
                && w.nodes@[k].repo == me.repo && w.nodes@[k].ticket.is_some()
                && w.nodes@[k].ticket.unwrap() < mine
                && slot_spec(*w, k) && !overlapped_by_older(*w, k, w.nodes@[k].ticket.unwrap())
                ==> first_runnable <= w.nodes@[k].ticket.unwrap(),
        decreases w.nodes.len() - j,
    ] {
        let index = j;
        let waiter = &w.nodes[j];
        j += 1;
        let Some(theirs) = waiter.ticket else { continue; };
        if !matches!(waiter.phase, Phase::Ready(_)) || waiter.repo != me.repo || theirs >= mine {
            continue;
        }
        kernel_proof! { assert(!oldest_waiter(*w, i as int)); }
        if !w.footprints_disjoint(waiter.footprint, me.footprint) { return false; }
        if theirs >= first_runnable || !slot_free(w, index) { continue; }
        if !older_overlap(w, index, theirs) { first_runnable = theirs; }
    } }
    first_runnable == mine
}
}
