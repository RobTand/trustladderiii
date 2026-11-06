//! #806 W7: both original adoption harnesses over real matching acknowledgements.
use super::port_deps_adoption::{acknowledged, agenda, confirmed as confirmed_spec, preparing};
use super::*;

verus! {
/// #806 adoption: exact preparation preserves every complete node-checker rule.
pub(super) proof fn node_preserved(w: World, n: Node)
    requires retirement::node_rules(w, n), preparing(n), node_headroom(n),
    ensures retirement::node_rules(w, acknowledged(n)),
        acknowledged(n).adopted, acknowledged(n).credit == n.credit,
        acknowledged(n).attempt == n.attempt, acknowledged(n).round == n.round,
        acknowledged(n).run.is_none(), !acknowledged(n).pool,
        acknowledged(n).holds == n.holds,
        acknowledged(n).footprint == n.footprint,
        acknowledged(n).phase == if n.adopted { Phase::Making } else { Phase::Checking },
        acknowledged(n).head == if n.adopted { n.head } else { n.proposed },
{
    reveal(preparing); reveal(acknowledged); reveal(confirmed_spec); reveal(agenda);
    reveal(retirement::phased); reveal(retirement::node_rules);
}

/// #806 adoption: the local frame preserves both original roots and all shared resources.
pub(super) proof fn world_preserved(w: World, v: World)
    requires retirement::domain(w), retirement::accepted(w), retirement::bounded(w),
        preparing(w.nodes@[0]),
        v.nodes@ == w.nodes@.update(0, acknowledged(w.nodes@[0])),
        v.gates@ == w.gates@, v.file_sets@ == w.file_sets@,
        v.next_ticket == w.next_ticket, v.next_question == w.next_question,
        v.pool_capacity == w.pool_capacity, v.dependencies@ == w.dependencies@,
    ensures retirement::domain(v), retirement::accepted(v),
{
    reveal(retirement::accepted); reveal(retirement::bounded);
    node_preserved(w, w.nodes@[0]);
    retirement::node_frame(w, v, acknowledged(w.nodes@[0]));
    retirement::node_frame(w, v, w.nodes@[1]);
    reveal(preparing); reveal(acknowledged); reveal(confirmed_spec); reveal(agenda); reveal(retirement::phased);
    footprint::storage_frame(w, v, w.nodes@[0].footprint, w.nodes@[1].footprint);
    assert forall|r: int| 0 <= r < 2 implies
        #[trigger] retirement::holders(v, RepoId(r as u64), 2) == retirement::holders(w, RepoId(r as u64), 2) by {}
}

/// #806 adoption assertions: execute real on_ack or real apply and both complete checker calls.
pub(super) fn adoption(mut w: World, via_apply: bool) -> (result: (World, Outbox))
    requires retirement::domain(w), retirement::accepted(w), retirement::bounded(w),
        storage_ordered(w), preparing(w.nodes@[0]),
    ensures retirement::domain(result.0), retirement::accepted(result.0), result.1.len == 0,
        result.0.nodes@[1] == w.nodes@[1], result.0.nodes@[0].adopted,
        result.0.nodes@[0].phase == if w.nodes@[0].adopted { Phase::Making } else { Phase::Checking },
        result.0.nodes@[0].head == if w.nodes@[0].adopted { w.nodes@[0].head } else { w.nodes@[0].proposed },
        result.0.nodes@[0].credit == w.nodes@[0].credit,
        result.0.nodes@[0].attempt == w.nodes@[0].attempt,
        result.0.nodes@[0].round == w.nodes@[0].round,
        result.0.nodes@[0].run.is_none(), !result.0.nodes@[0].pool,
        result.0.nodes@[0].review_stop == w.nodes@[0].review_stop,
{
    let ghost before = w;
    proof {
        reveal(retirement_domain); reveal(retirement_accepted); reveal(retirement::bounded);
        reveal(retirement::accepted); reveal(retirement::node_rules); reveal(world_headroom);
        reveal_with_fuel(open_prefix, 3); reveal(storage_ordered); reveal(preparing);
        dependencies::indexable_definition(w);
    }
    let checked = w.check(); assert(checked.is_ok());
    let seq = w.nodes[0].effect.unwrap().seq;
    let mut out = Outbox::new();
    proof { reveal(empty_outbox); }
    let ghost original_out = out;
    if via_apply {
        apply(&mut w, Event::Ack { node: NodeId(0), seq, ack: Ack::Confirmed }, &mut out);
        proof {
            reveal(port_acks_specs::ack_post); reveal(port_acks_specs::ack_local);
            reveal(port_acks_specs::world_frame);
            port_deps_adoption::local_fields(before.nodes@[0], w.nodes@[0],
                before.next_question, w.next_question, 0, seq, original_out, out);
        }
    } else {
        on_ack(&mut w, 0, seq, Ack::Confirmed, &mut out);
        proof { reveal(port_deps_adoption::fields); }
    }
    proof {
        port_deps_adoption::from_fields(before.nodes@[0], w.nodes@[0]);
        assert(w.nodes@ =~= before.nodes@.update(0, acknowledged(before.nodes@[0])));
        world_preserved(before, w); node_preserved(before, before.nodes@[0]);
        reveal(retirement::bounded); reveal(world_headroom); reveal_with_fuel(open_prefix, 3);
        reveal(acknowledged); reveal(confirmed_spec); reveal(agenda); reveal(retirement::phased);
        reveal(storage_ordered); reveal(footprint::ordered_table);
    }
    let checked = w.check(); assert(checked.is_ok());
    (w, out)
}

/// #806 original adoption covers: construct both small two-root preparation worlds.
/// The reusable predicate is the exact any_world LIMIT/CREDIT_LIMIT and four-file domain.
pub(super) fn adoption_world(already: bool) -> (result: World)
    ensures port_advance_witness::original_domain(result), retirement::bounded(result),
        storage_ordered(result), preparing(result.nodes@[0]),
        result.nodes@[0].adopted == already,
        result.nodes@[0].head == if already { Some(HeadId(1)) } else { None },
        result.nodes@[0].proposed == Some(HeadId(7)),
        result.nodes@[0].credit.earned == 0,
        result.nodes@[0].attempt == 1, result.nodes@[0].round == 0,
{
    let mut n = Node::new(RepoId(0));
    n.phase = Phase::Making; n.credit.ready = true; n.credit.start = 4;
    n.attempt = 1; n.holds = true; n.footprint = Footprint::inline(1);
    n.next_seq = 1; n.proposed = Some(HeadId(7)); n.adopted = already;
    n.head = if already { Some(HeadId(1)) } else { None };
    n.effect = Some(InFlight { seq: 0, kind: EffectKind::PrepareWorktree, cancelling: false });
    let mut other = Node::new(RepoId(1)); other.phase = Phase::Closed(Outcome::Stopped);
    let mut nodes = Vec::new(); nodes.push(n); nodes.push(other);
    let mut gates = Vec::new(); gates.push(Gate { w: 1 }); gates.push(Gate { w: 1 });
    let w = World { nodes, gates, pool_capacity: 1, next_ticket: 0, next_question: 0,
        file_sets: Vec::new(), dependencies: Vec::new(), fail_fast: false };
    proof {
        reveal(initial_node); reveal(preparing); reveal(retirement::accepted);
        reveal(retirement::node_rules); reveal(retirement::bounded); reveal(storage_ordered);
        reveal(footprint::ordered_table); footprint::inline_is_valid(w, 0); footprint::inline_is_valid(w, 1);
        assert forall|i: int| 0 <= i < 2 implies port_advance_witness::small(w.nodes@[i]) by {
            assert(i == 0 || i == 1);
        }
        assert forall|i: int| 0 <= i < w.nodes@.len() implies {
            let node = #[trigger] w.nodes@[i];
            exists|bits: u64| bits < 16 && node.footprint == footprint::inline_footprint(bits)
        } by { assert(i == 0 || i == 1); }
        assert forall|i: int| 0 <= i < 2 implies retirement::node_rules(w, w.nodes@[i]) by {
            assert(i == 0 || i == 1);
        }
        assert(port_advance_witness::original_domain(w));
    }
    w
}

/// #806 adoption covers: return the original world's first/later real transition.
/// Both booleans are universal, covering direct on_ack and its real apply dispatch.
pub(super) fn adoption_cover(already: bool, via_apply: bool) -> (result: (World, Outbox))
    ensures result.0.nodes@.len() == 2, result.0.nodes@[0].adopted,
        result.0.nodes@[0].phase == if already { Phase::Making } else { Phase::Checking },
        result.0.nodes@[0].head == Some(HeadId(if already { 1 } else { 7 })),
        result.0.nodes@[0].credit.earned == 0,
        result.0.nodes@[0].attempt == 1, result.0.nodes@[0].round == 0,
        result.0.nodes@[0].run.is_none(), !result.0.nodes@[0].pool, result.1.len == 0,
        retirement::accepted(result.0),
{
    let w = adoption_world(already);
    adoption(w, via_apply)
}
}
