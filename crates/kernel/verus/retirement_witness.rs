//! #806: constructive covers for the five retired Kani harnesses.
use super::retirement::*;
use super::*;

kernel_fn! {
/// State-table incident, dequeue, infrastructure and merge rows / #806.
/// Six finite witnesses cover five harnesses and both infrastructure outcomes.
/// The original two-node, two-repo, four-file and small-counter bounds hold.
fn witness_world(case: u8) -> (result: World) [
    requires case < 6,
    ensures domain(result), accepted(result), bounded(result), storage_ordered(result),
        result.nodes@[0].phase == match case {
            0 => Phase::Ingesting, 1 => Phase::Queued,
            2 | 3 => Phase::Publishing, 4 => Phase::Ingesting, _ => Phase::Auditing,
        },
        result.nodes@[0].had_pr == (case > 0), result.nodes@[0].merged.is_none(),
        result.nodes@[0].approved == if case < 4 && case > 0 { Some(HeadId(1)) } else { None },
        case == 0 ==> result.nodes@[0].waiting_incident && result.nodes@[0].incident_needs_rob,
        2 <= case <= 3 ==> fact_pre(result.nodes@[0], Fact::CiFailed { infra: true })
            && result.nodes@[0].run_failures == case - 2 && result.nodes@[0].effect.is_some(),
] proof [
    reveal(initial_node); reveal(accepted); reveal(node_rules); reveal(bounded);
    reveal(storage_ordered); reveal(footprint::ordered_table);

] {
    let mut first = Node::new(RepoId(0));
    let mut second = Node::new(RepoId(1));
    second.phase = Phase::Closed(Outcome::Stopped);
    if case == 0 {
        first.waiting_incident = true;
        first.incident_needs_rob = true;
    } else {
        first.credit.ready = true;
        first.credit.start = 4;
        first.had_pr = true;
        first.next_seq = 1;
        if case < 4 {
            first.phase = if case == 1 { Phase::Queued } else { Phase::Publishing };
            first.attempt = 1;
            first.footprint = Footprint::inline(1);
            first.head = Some(HeadId(1));
            first.checked = first.head;
            first.approved = first.head;
            first.pr_open = true;
            first.holds = true;
            if case > 1 {
                first.pos = 3;
                first.run_failures = case - 2;
                first.effect = Some(InFlight { seq: 0, kind: EffectKind::Enqueue, cancelling: false });
            }
        } else {
            first.phase = if case == 4 { Phase::Ingesting } else { Phase::Auditing };
            first.pos = 1;
            first.run = Some(0);
            first.pool = true;
        }
    }
    let mut nodes = Vec::new();
    nodes.push(first);
    nodes.push(second);
    let mut gates = Vec::new();
    gates.push(Gate { w: 1 });
    gates.push(Gate { w: 1 });
    let w = World { nodes, gates, pool_capacity: 1, next_ticket: 0, next_question: 0,
        file_sets: Vec::new(), dependencies: Vec::new(), fail_fast: false };
    kernel_proof! {
        footprint::inline_is_valid(w, 0);
        footprint::inline_is_valid(w, 1);
        reveal(bounded);
        assert forall|i: int| 0 <= i < w.nodes@.len() implies {
            let n = #[trigger] w.nodes@[i];
            n.generation < 1099511627776u64 && n.next_seq < 1099511627776u64
                && n.credit.start < 1099511627776u64 && n.credit.earned < 1099511627776u64
                && n.credit.added < 1099511627776u64 && n.credit.spent < 1099511627776u64
        } by { assert(i == 0 || i == 1); }
        assert(bounded(w));
        assert(node_rules(w, w.nodes@[0]));
        assert(node_rules(w, w.nodes@[1]));
        assert(accepted(w));

    }
    w
}
}

kernel_fn! {
/// #806 covers: incident clears, dequeue changes the world, infrastructure
/// retries once then raises an incident, and both merge phases record a merge.
/// Every case calls the real apply composition with a satisfiable precondition.
fn changed_world_witness(case: u8) -> (result: ()) [
    requires case < 6,
] proof [reveal(fact_node); reveal(stopped); reveal(incident);] {
    let w = witness_world(case);
    kernel_snapshot! { before = w }
    let fact = if case == 0 { Fact::IncidentCleared { by_rob: true } }
        else if case == 1 { Fact::PrDequeuedByPerson }
        else if case < 4 { Fact::CiFailed { infra: true } }
        else { Fact::PrMerged { head: HeadId(2), checks_passed: false } };
    let (after, out) = verify_selected_apply(w, fact);
    kernel_proof! {
        assert(after.nodes@[0] != before@.nodes@[0]);
        if 2 <= case <= 3 {
            assert(after.nodes@[0].waiting_incident == (case == 3));
            assert(before@.nodes@[0].effect.is_some());
        }
        if case >= 4 { assert(unapproved_note(out, 0)); }
    }
}
}
