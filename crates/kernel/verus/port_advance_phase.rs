//! #806: parametric replacement of the original per-phase settle-step assertions.
use super::retirement::{accepted, bounded, domain};
use super::*;

verus! {
/// #806: every assertion of the original check_nodes, for one corresponding node.
pub(super) open spec fn node_post(before: Node, after: Node) -> bool {
    after.proposed == before.proposed && (before.adopted ==> after.adopted)
        && after.next_seq >= before.next_seq
        && (before.phase is Closed ==> after.phase == before.phase)
        && after.credit.earned as int <= before.credit.earned as int + 1
        && (after.credit.earned > before.credit.earned
            ==> !before.credit.merge_credited && after.credit.merge_credited)
}

/// #806: World PartialEq observes vector contents and every scalar field.
pub(super) open spec fn same_world(before: World, after: World) -> bool {
    before.nodes@ == after.nodes@ && before.gates@ == after.gates@
        && before.file_sets@ == after.file_sets@ && before.dependencies@ == after.dependencies@
        && before.next_ticket == after.next_ticket && before.next_question == after.next_question
        && before.pool_capacity == after.pool_capacity && before.fail_fast == after.fail_fast
}

/// #806: exactly the first seven moving phase harnesses, with every flag/Resume.
pub(super) open spec fn slice_one(phase: Phase) -> bool {
    matches!(phase, Phase::Ingesting | Phase::Auditing | Phase::Unplanned
        | Phase::Planning { .. } | Phase::Ready(_) | Phase::Making | Phase::Checking)
}
}

kernel_fn! {
/// State-table internal transitions / #806: one real advance on node zero.
/// Original two-node domain, with weaker safe-counter bounds and no search unwind.
/// All full-checker, node, effect, rank and false-result assertions are retained.
pub(super) fn verify_phase(mut w: World) -> (result: (World, Outbox, bool)) [
    requires domain(w), accepted(w), bounded(w), storage_ordered(w),
        port_advance_world::inline_nodes(w),
    ensures domain(result.0), accepted(result.0), port_advance_world::frame(w, result.0),
        result.0.nodes@[1] == w.nodes@[1],
        forall|i: int| 0 <= i < 2 ==> node_post(w.nodes@[i], result.0.nodes@[i]),
        forall|slot: int| 0 <= slot < OUTBOX ==> match #[trigger] result.1.items@[slot] {
            Some(output) => port_advance_outputs::effect_rule(w.nodes@[0], result.0.nodes@[0], 0, output),
            None => true,
        },
        result.2 ==> result.0.nodes@[0] != w.nodes@[0]
            && 4 * node_distance(result.0.nodes@[0]) + pending_rank(result.0.nodes@[0])
                < 4 * node_distance(w.nodes@[0]) + pending_rank(w.nodes@[0]),
        !result.2 ==> same_world(w, result.0) && result.1.len == 0,
        movable(w, 0) ==> result.2,
] proof [reveal(advance_post); reveal(empty_outbox); reveal(accepted);
    reveal(retirement_domain); reveal(retirement_accepted);
    reveal(storage_ordered); reveal(footprint::ordered_table);] {
    kernel_snapshot! { before = w }
    kernel_proof! { port_advance_world::prepared(w); }
    let checked_before = w.check();
    assert!(checked_before.is_ok());
    kernel_proof! { ranks::node_distance_definition(w.nodes@[0]); }
    let rank = w.nodes[0].settle_rank();
    let mut out = Outbox::new();
    kernel_snapshot! { empty = out }
    let changed = advance(&mut w, 0, &mut out);
    kernel_proof! {
        let c = choose|c: AdvanceContext| c.node == 0
            && changed == advance_enabled(before@.nodes@[0], c)
            && port_advance_contracts::context_safe(before@, 0, c)
            && port_advance_contracts::pool_context(before@, 0, c)
            && port_advance_contracts::exact(before@.nodes@[0], w.nodes@[0], before@.next_ticket,
                w.next_ticket, c, empty@, out);
        port_advance_world::consistent(before@, w, c, empty@, out);
        port_advance_world::checker_reserves(before@, w, c, empty@, out);
        port_advance_contracts::transition_rules(before@.nodes@[0], w.nodes@[0], before@.next_ticket,
            w.next_ticket, c, empty@, out);
        port_advance_outputs::output_rules(before@, before@.nodes@[0], w.nodes@[0], before@.next_ticket,
            w.next_ticket, c, empty@, out);
        if !changed {
            reveal(port_advance_contracts::exact);
            port_advance_outputs::stutter(before@.nodes@[0], c, empty@, out);
            assert(w.nodes@[0] == before@.nodes@[0]);
            assert(w.next_ticket == before@.next_ticket);
            assert(w.nodes@ =~= before@.nodes@);
            assert(same_world(before@, w));
            assert(out.len == 0);
        }
        assert forall|i: int| 0 <= i < 2 implies node_post(before@.nodes@[i], w.nodes@[i]) by {
            assert(i == 0 || i == 1);
        }
        if changed { assert(w.nodes@[0] != before@.nodes@[0]); }
        ranks::node_distance_definition(w.nodes@[0]);
    }
    let after_rank = w.nodes[0].settle_rank();
    if changed { assert!(after_rank < rank); }
    let checked_after = w.check();
    assert!(checked_after.is_ok());
    (w, out, changed)
}
}
