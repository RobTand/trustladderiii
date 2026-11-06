//! #806: compose local step contracts with the complete two-node world checker.
use super::*;
use super::port_advance_contracts::*;

verus! {
/// #806: Footprint::Arbitrary generates exactly the four-file inline domain.
pub(super) open spec fn inline_nodes(w: World) -> bool {
    forall|i: int| 0 <= i < w.nodes@.len() ==> {
        let n = #[trigger] w.nodes@[i];
        exists|bits: u64| bits < 16 && n.footprint == footprint::inline_footprint(bits)
    }
}

/// Q30 / #806: reverse the real inline lock check to the checker's pair order.
pub(super) proof fn inline_symmetric(w: World, a: u64, b: u64)
    ensures footprint::disjoint(w, footprint::inline_footprint(a), footprint::inline_footprint(b))
        == footprint::disjoint(w, footprint::inline_footprint(b), footprint::inline_footprint(a)),
{
    footprint::inline_disjoint(w, a, b); footprint::inline_disjoint(w, b, a);
    assert((a & b) == (b & a)) by (bit_vector);
}

/// #806: exact nonlocal frame exported by scheduler::advance on request zero.
pub(super) open spec fn frame(w: World, v: World) -> bool {
    v.nodes@ == w.nodes@.update(0, v.nodes@[0])
        && v.gates@ == w.gates@ && v.file_sets@ == w.file_sets@
        && v.dependencies@ == w.dependencies@ && v.next_question == w.next_question
        && v.pool_capacity == w.pool_capacity && v.fail_fast == w.fail_fast
}

/// Q30 / Q39 / #806: only the corresponding granted step can acquire a resource.
pub(super) proof fn resource_frame(n: Node, ticket: u64, c: AdvanceContext)
    ensures node_after(n, ticket, c).repo == n.repo,
        node_after(n, ticket, c).footprint == n.footprint,
        node_after(n, ticket, c).holds ==> n.holds || c.slot,
        node_after(n, ticket, c).pool ==> n.pool || c.pool,
        !n.credit.withheld ==> !node_after(n, ticket, c).credit.withheld,
        node_after(n, ticket, c).ticket.is_none()
            || node_after(n, ticket, c).ticket == n.ticket
            || node_after(n, ticket, c).ticket == Some(ticket),
        node_after(n, ticket, c).generation as int <= n.generation as int + 1,
        node_after(n, ticket, c).next_seq as int <= n.next_seq as int + 1,
        node_after(n, ticket, c).credit.start == n.credit.start,
        node_after(n, ticket, c).credit.earned as int <= n.credit.earned as int + 1,
        node_after(n, ticket, c).credit.added == n.credit.added,
        node_after(n, ticket, c).credit.spent == n.credit.spent,
{
    reveal(node_after);
}

/// #806: full consistency, including gate capacity, pool count and ticket uniqueness.
pub(super) proof fn consistent(w: World, v: World, c: AdvanceContext, before: Outbox, out: Outbox)
    requires retirement::domain(w), retirement::accepted(w), frame(w, v),
        advance_pre(w.nodes@[0], w.next_ticket, before.len), before.len == 0,
        inline_nodes(w), context_safe(w, 0, c), exact(w.nodes@[0], v.nodes@[0], w.next_ticket, v.next_ticket, c, before, out),
    ensures retirement::domain(v), retirement::accepted(v),
{
    reveal(exact); reveal(retirement::accepted);
    let n = w.nodes@[0];
    let m = v.nodes@[0];
    resource_frame(n, w.next_ticket, c);
    port_advance_node_rules::node_rules(w, v, n, c);
    port_advance_node_rules::environment_frame(w, v, w.nodes@[1]);
    footprint::storage_frame(w, v, n.footprint, w.nodes@[1].footprint);
    reveal_with_fuel(potential_prefix, 3);
    assert forall|r: int| 0 <= r < 2 implies
        retirement::holders(v, RepoId(r as u64), 2) <= v.gates@[r].w by {
        if !n.holds && m.holds && n.repo == RepoId(r as u64) {
            assert(c.slot);
            assert(slot_safe(w, 0));
        } else {
            assert(retirement::holders(v, RepoId(r as u64), 2)
                <= retirement::holders(w, RepoId(r as u64), 2));
        }
    }
    assert(m.holds && v.nodes@[1].holds && m.repo == v.nodes@[1].repo
        ==> footprint::disjoint(v, m.footprint, v.nodes@[1].footprint)) by {
        if !n.holds && m.holds {
            assert(slot_safe(w, 0));
            let a = choose|bits: u64| bits < 16 && n.footprint == footprint::inline_footprint(bits);
            let b = choose|bits: u64| bits < 16 && w.nodes@[1].footprint == footprint::inline_footprint(bits);
            inline_symmetric(w, a, b);
        }
    }
    assert((if m.pool { 1int } else { 0int }) + (if v.nodes@[1].pool { 1int } else { 0int })
        <= v.pool_capacity);
    assert(m.ticket.is_some() ==> m.ticket != v.nodes@[1].ticket) by {
        reveal(retirement::node_rules); reveal(node_checked);
    }
    assert forall|j: int| 0 <= j < 2 implies retirement::node_rules(v, v.nodes@[j]) by {
        assert(j == 0 || j == 1);
    }
}

/// #806: the original small counters leave enough room for the real poststep checker.
pub(super) proof fn checker_reserves(w: World, v: World, c: AdvanceContext,
    before: Outbox, out: Outbox)
    requires retirement::domain(w), retirement::bounded(w), frame(w, v),
        exact(w.nodes@[0], v.nodes@[0], w.next_ticket, v.next_ticket, c, before, out),
    ensures world_headroom(v),
{
    resource_frame(w.nodes@[0], w.next_ticket, c);
    reveal(exact); reveal(retirement::bounded); reveal(world_headroom);
    open_prefix_monotone(v.nodes@, 0, v.nodes@.len());
    reveal(open_prefix);
    assert forall|j: int| 0 <= j < v.nodes@.len() implies node_headroom(v.nodes@[j]) by {
        assert(j == 0 || j == 1);
    }
}

/// #806: complete original consistency and small counters discharge scheduler/checker scopes.
pub(super) proof fn prepared(w: World)
    requires retirement::domain(w), retirement::accepted(w), retirement::bounded(w), storage_ordered(w),
    ensures scheduling_safe(w), world_headroom(w), advance_pre(w.nodes@[0], w.next_ticket, 0),
{
    reveal(retirement::accepted); reveal(retirement::bounded); reveal(storage_ordered);
    reveal(retirement::node_rules); reveal(node_checked); reveal(world_headroom);
    open_prefix_monotone(w.nodes@, 0, w.nodes@.len());
    reveal(open_prefix);
    local_capacities(w.nodes@[0]);
    assert forall|j: int| 0 <= j < w.nodes@.len() implies node_headroom(w.nodes@[j]) by {}
}

}
