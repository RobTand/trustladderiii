//! Q30/Q39 / #805: real scheduler queries and dispatcher.
use super::*;

kernel_fn! {
/// #771: source-connected scheduler wrapper. Context queries are read at the
/// original branches; the verified local transition writes only this node.
pub(super) fn advance(w: &mut World, i: usize, out: &mut Outbox) -> (result: bool) [
    requires i < old(w).nodes@.len(), i <= u64::MAX as int,
        scheduling_safe(*old(w)),
        advance_pre(old(w).nodes@[i as int], old(w).next_ticket, old(out).len),
    ensures (!result ==> final(out).len == old(out).len),
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
        old(w).nodes@[i as int].phase == Phase::Queued && !old(w).nodes@[i as int].waiting_incident
            ==> !result,
        progress_gate_leaf_steps::advance_details(old(w).nodes@[i as int], final(w).nodes@[i as int],
            old(w).next_ticket, final(w).next_ticket),
        (old(w).nodes@[i as int].phase == Phase::Ready(Resume::Making) && old(w).nodes@[i as int].pos == 0
            && !old(w).nodes@[i as int].waiting_incident && old(w).nodes@[i as int].effect.is_none()) ==> {
            final(w).nodes@[i as int] == retirement_gate::slot_node(old(w).nodes@[i as int], old(w).next_ticket,
                retirement_gate::slot(*old(w), i as int) && retirement_gate::line(*old(w), i as int))
                && final(w).next_ticket == retirement_gate::slot_ticket(old(w).nodes@[i as int], old(w).next_ticket)
                && *final(out) == *old(out)
                && result == retirement_gate::slot_changed(old(w).nodes@[i as int],
                    retirement_gate::slot(*old(w), i as int) && retirement_gate::line(*old(w), i as int))
        },
] {
    kernel_snapshot! { before_advance = *w }
    kernel_proof! { dependencies::prerequisite_definition(*w, NodeId(i as u64)); }
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
    let changed = advance_node(&mut w.nodes[i], &mut w.next_ticket, context, out);
    kernel_proof! {
        if before_advance@.nodes@[i as int].phase is Ready && before_advance@.nodes@[i as int].pos == 0 {
            reveal(retirement_gate::slot_node);
        }
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
        result == retirement_gate::slot(*w, i as int),
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
            holders == retirement_gate::slot_count(*w, i as int, j as nat),
            forall|k: int| 0 <= k < j && k != i && w.nodes@[k].holds && w.nodes@[k].repo == me.repo
                ==> footprint::disjoint(*w, w.nodes@[k].footprint, me.footprint),
            ((forall|k: int| 0 <= k < w.nodes@.len() && k != i ==> !w.nodes@[k].holds)
                ==> holders == 0),
        decreases w.nodes.len() - j,
    ] {
        let n = &w.nodes[j];
        kernel_proof! {
            reveal(retirement_gate::slot_count);
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
    ensures result == retirement_gate::overlap(*w, i as int, ticket),
] {
    let waiter = &w.nodes[i];
    let mut index = 0usize;
    kernel_while! { (index < w.nodes.len()) [
        invariant index <= w.nodes.len(), i < w.nodes.len(), scheduling_safe(*w),
            *waiter == w.nodes@[i as int],
            forall|k: int| 0 <= k < index && w.nodes@[k].phase is Ready && w.nodes@[k].repo == waiter.repo
                && (match w.nodes@[k].ticket { Some(t) => t < ticket, None => false })
                ==> footprint::disjoint(*w, w.nodes@[k].footprint, waiter.footprint),
        decreases w.nodes.len() - index,
    ] {
        let older = &w.nodes[index];
        if matches!(older.phase, Phase::Ready(_)) && older.repo == waiter.repo {
            if let Some(theirs) = older.ticket {
                if theirs < ticket && !w.footprints_disjoint(older.footprint, waiter.footprint) {
                    kernel_proof! {
                        assert(retirement_gate::overlapping_before(*w, i as int, index as int, ticket));
                    }
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
        w.nodes@[i as int].phase is Ready ==> result == retirement_gate::line(*w, i as int),
] {
    let me = &w.nodes[i];
    let Some(mine) = me.ticket else { return false };
    if !matches!(me.phase, Phase::Ready(_)) {
        let mut index = 0usize;
        kernel_while! { (index < w.nodes.len()) [
            invariant index <= w.nodes.len(), i < w.nodes.len(),
                *me == w.nodes@[i as int], !(me.phase is Ready), me.ticket == Some(mine),
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
            first_runnable <= mine,
            (first_runnable == mine) == retirement_gate::line_prefix(*w, i as int, j as int),
        decreases w.nodes.len() - j,
    ] {
        kernel_proof! { retirement_gate::prefix_step(*w, i as int, j as int); }
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
