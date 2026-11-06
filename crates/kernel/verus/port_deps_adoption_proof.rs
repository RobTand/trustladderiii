//! #806 W7: both original adoption harnesses over real matching acknowledgements.
use super::port_deps_adoption::{acknowledged, agenda, confirmed as confirmed_spec, preparing};
use super::*;

verus! {
/// #771 / #806: clearing a matching delivery preserves the established progress step.
/// Isolate the inherited distance expansion from the complete on_ack frame.
pub(super) proof fn clear_effect(n: Node)
    requires !(n.phase is Closed), n.effect.is_some(),
        !n.paused || n.phase is Stopping,
    ensures ({ let cleared = Node { effect: None, ..n };
        if n.effect.unwrap().cancelling || matches!(n.effect.unwrap().kind,
            EffectKind::StartRun(_) | EffectKind::SubmitChecks | EffectKind::StartRebase) {
            distance(cleared) < distance(n)
        } else { distance(cleared) <= distance(n) + 1 }
    }),
{
    reveal(distance);
}

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

/// #806 adoption assertions: execute real on_ack and both complete checker calls.
pub(super) fn adoption(mut w: World) -> (result: (World, Outbox))
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
{
    let ghost before = w;
    proof {
        reveal(retirement_domain); reveal(retirement_accepted); reveal(retirement::bounded);
        reveal(retirement::accepted); reveal(retirement::node_rules); reveal(world_headroom);
        reveal_with_fuel(open_prefix, 3); reveal(storage_ordered); reveal(preparing);
    }
    let checked = w.check(); assert(checked.is_ok());
    let seq = w.nodes[0].effect.unwrap().seq;
    let mut out = Outbox::new();
    proof { reveal(empty_outbox); }
    on_ack(&mut w, 0, seq, Ack::Confirmed, &mut out);
    proof {
        reveal(port_deps_adoption::ack_post);
        assert(w.nodes@ =~= before.nodes@.update(0, acknowledged(before.nodes@[0])));
        world_preserved(before, w); node_preserved(before, before.nodes@[0]);
        reveal(retirement::bounded); reveal(world_headroom); reveal_with_fuel(open_prefix, 3);
        reveal(acknowledged); reveal(confirmed_spec); reveal(agenda); reveal(retirement::phased);
        reveal(storage_ordered); reveal(footprint::ordered_table);
    }
    let checked = w.check(); assert(checked.is_ok());
    (w, out)
}

/// #806 original adoption covers: construct two small roots and run real on_ack.
pub(super) fn adoption_cover(already: bool) -> (result: (World, Outbox))
    ensures result.0.nodes@.len() == 2, result.0.nodes@[0].adopted,
        result.0.nodes@[0].phase == if already { Phase::Making } else { Phase::Checking },
        result.0.nodes@[0].head == Some(HeadId(if already { 1 } else { 7 })),
        result.0.nodes@[0].credit.earned == 0,
        result.0.nodes@[0].attempt == 1, result.0.nodes@[0].round == 0,
        result.0.nodes@[0].run.is_none(), !result.0.nodes@[0].pool, result.1.len == 0,
        retirement::accepted(result.0),
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
        assert forall|i: int| 0 <= i < 2 implies retirement::node_rules(w, w.nodes@[i]) by {
            assert(i == 0 || i == 1);
        }
    }
    adoption(w)
}
}
