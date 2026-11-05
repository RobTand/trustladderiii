//! #806: independently gated safety obligations over the unchanged real kernel.
use super::retirement::*;
use super::*;

verus! {
/// State-table global consistency / #806: node-local facts cannot acquire
/// a slot, pool grant, conflicting footprint or duplicate ticket.
pub(super) proof fn fact_preserves_world(before: World, after: World, fact: Fact)
    requires domain(before), accepted(before), selected(fact), fact_pre(before.nodes@[0], fact),
        node_headroom(before.nodes@[0]),
        after.nodes@ == before.nodes@.update(0, fact_node(before.nodes@[0], fact)),
        after.gates@ == before.gates@, after.file_sets@ == before.file_sets@,
        after.next_ticket == before.next_ticket, after.next_question == before.next_question,
        after.pool_capacity == before.pool_capacity, after.dependencies@ == before.dependencies@,
    ensures domain(after), accepted(after),
{
    reveal(accepted); reveal(fact_node);
    fact_preserves_node(before, before.nodes@[0], fact);
    node_frame(before, after, after.nodes@[0]);
    node_frame(before, after, after.nodes@[1]);
    assert forall|r: int| 0 <= r < 2 implies
        holders(after, RepoId(r as u64), 2) <= after.gates@[r].w by {
        assert(holders(after, RepoId(r as u64), 2) <= holders(before, RepoId(r as u64), 2));
    }
    footprint::storage_frame(before, after, before.nodes@[0].footprint, before.nodes@[1].footprint);
}
}

kernel_fn! {
/// State-table selected safety fact rows / #806.
/// Bound: one real apply on the original two-node construction, with no unwind.
/// Settledness is unnecessary. Full consistency, all shared Kani node assertions,
/// notice-only outputs, truthful merges and unapproved-merge notes are preserved.
pub(super) fn verify_selected_apply(mut w: World, fact: Fact) -> (result: (World, Outbox)) [
    requires domain(w), accepted(w), bounded(w), storage_ordered(w),
        selected(fact), fact_pre(w.nodes@[0], fact),
    ensures domain(result.0), accepted(result.0),
        result.0.nodes@ == w.nodes@.update(0, fact_node(w.nodes@[0], fact)),
        notices_only(result.1),
        fact is Priority ==> result.1.len == 0,
        match fact {
            Fact::Conflict => {
                let n = w.nodes@[0];
                let after = result.0.nodes@[0];
                if matches!(n.phase, Phase::Publishing | Phase::Queued) {
                    after.phase == Phase::Rebasing && after.generation == n.generation + 1
                        && after.credit == n.credit && after.holds == n.holds
                } else { after == n }
            },
            Fact::PrMerged { head, checks_passed } => w.nodes@[0].had_pr
                && w.nodes@[0].merged.is_none() && !(w.nodes@[0].phase is Closed)
                ==> result.0.nodes@[0].merged == Some(head)
                    && result.0.nodes@[0].phase == Phase::Stopping(Outcome::Merged)
                    && (!(w.nodes@[0].approved == Some(head) && checks_passed)
                        ==> unapproved_note(result.1, 0)),
            Fact::CiFailed { infra: true } => result.0.nodes@[0].credit == w.nodes@[0].credit
                && (result.0.nodes@[0].waiting_incident
                    || result.0.nodes@[0].run_failures > w.nodes@[0].run_failures)
                && match w.nodes@[0].effect {
                    None => true,
                    Some(e) => match result.0.nodes@[0].effect {
                        Some(current) => current.seq == e.seq && current.cancelling, None => false,
                    },
                },
            _ => true,
        },
] proof [
    reveal(world_headroom); reveal_with_fuel(open_prefix, 3); reveal(storage_ordered);
    reveal(empty_outbox); reveal(retirement_domain); reveal(retirement_accepted);
    reveal(accepted); reveal(bounded); reveal(notices_only); reveal(fact_node); reveal(stopped); reveal(incident);
] {
    kernel_snapshot! { before = w }
    kernel_proof! {
        open_prefix_monotone(w.nodes@, 0, w.nodes@.len());
        assert forall|j: int| 0 <= j < w.nodes@.len() implies node_headroom(w.nodes@[j]) by {}
        assert(world_headroom(w));
    }
    kernel_proof! { dependencies::indexable_definition(w); }
    let checked_before = w.check();
    assert!(checked_before.is_ok());
    kernel_proof! { world_checked_definition(w); }
    let mut out = Outbox::new();
    apply(&mut w, Event::Fact { node: NodeId(0), fact }, &mut out);
    kernel_proof! {
        fact_preserves_world(before@, w, fact);
    }
    let checked = w.check();
    assert!(checked.is_ok());
    (w, out)
}
}
