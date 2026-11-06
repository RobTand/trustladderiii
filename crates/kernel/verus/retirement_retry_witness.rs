//! #806: original-domain settled covers for the final six apply harnesses.
use super::*;

kernel_fn! {
/// Q50/Q54 and creation rows / #806: eight concrete branches, each one real apply.
/// Bounds: original two nodes/two repos, W1/pool1, four-file inline sets and small IDs.
/// CI cases preserve all five original covers, including a live enqueue cancellation.
fn apply_covers(case: u8) -> (result: ()) [
    requires case < 8,
] proof [
    reveal(initial_node); reveal(retirement::accepted); reveal(retirement::node_rules);
    reveal(retirement::bounded); reveal(storage_ordered); reveal(footprint::ordered_table);
    reveal(retirement_retry::fact_result); reveal(retirement_retry::failed);
    reveal(retirement_retry::rung); reveal(retirement_retry::rechecked);
    reveal(retirement_retry::recheck_core); reveal(retirement_retry::continued);
    reveal(retirement::fact_node); reveal(retirement::stopped); reveal(retirement::incident);
] {
    let mut first = Node::new(RepoId(0));
    let mut second = Node::new(RepoId(1));
    second.phase = Phase::Closed(Outcome::Stopped);
    first.phase = if case < 4 { Phase::Publishing } else { Phase::Queued };
    first.credit.ready = true;
    first.credit.start = if case == 3 { 0 } else { 4 };
    first.had_pr = true;
    first.head = Some(HeadId(1));
    first.checked = first.head;
    first.approved = first.head;
    first.attempt = 1;
    first.round = if case == 1 { 0 } else { 1 };
    first.footprint = Footprint::inline(1);
    first.holds = true;
    first.pr_open = true;
    if case < 4 {
        first.pos = 3;
        first.next_seq = 1;
        first.effect = Some(InFlight { seq: 0, kind: EffectKind::Enqueue, cancelling: false });
    }
    let mut nodes = Vec::new();
    nodes.push(first);
    nodes.push(second);
    let mut gates = Vec::new();
    gates.push(Gate { w: 1 });
    gates.push(Gate { w: 1 });
    let before = World { nodes, gates, pool_capacity: 1, next_ticket: 0, next_question: 0,
        file_sets: Vec::new(), dependencies: Vec::new(), fail_fast: case == 0 };
    kernel_proof! {
        footprint::inline_is_valid(before, 0);
        footprint::inline_is_valid(before, 1);
        assert(retirement::domain(before));
        assert(retirement::accepted(before));
        assert(retirement::bounded(before));
        reveal(settled_world); reveal(movable);
        assert forall|i: int| 0 <= i < before.nodes@.len() implies !movable(before, i) by {
            assert(i == 0 || i == 1);
        }
        assert(settled_world(before));
    }
    kernel_snapshot! { original = before }
    if case < 4 {
        let (after, _) = retirement_publishing::verify_publishing_apply(before);
        kernel_proof! {
            assert(after.nodes@[0] != original@.nodes@[0]);
            assert(original@.nodes@[0].effect.is_some());
            if case == 0 {
                assert(original@.fail_fast && original@.nodes@[0].round == 1);
                assert(original@.nodes@[0].review_stop.is_none());
                assert(after.nodes@[0].review_stop == Some(ReviewStop::CiFailed));
            } else if case == 1 {
                assert(original@.nodes@[0].round == 0 && after.nodes@[0].round == 1);
            } else if case == 2 {
                assert(original@.nodes@[0].round == 1 && after.nodes@[0].credit.spent > original@.nodes@[0].credit.spent);
            } else {
                assert(!retirement_retry::can_pay(original@.nodes@[0]));
                assert(after.nodes@[0].phase == Phase::AwaitingRob);
            }
        }
    } else if case == 4 {
        let (after, _) = retirement_retry::verify_retry_apply(before, Fact::PushByOther { head: HeadId(2) });
        kernel_proof! { assert(after.nodes@[0] != original@.nodes@[0]); }
    } else if case == 5 {
        let (after, _) = retirement::verify_selected_apply(before, Fact::UpstreamStalled);
        kernel_proof! { assert(after.nodes@[0] != original@.nodes@[0]); }
    } else {
        let head = if case == 7 { Some(HeadId(7)) } else { None };
        let (after, _) = retirement_wide::verify_new_apply(before, RepoId(0), Priority::P0, head);
        kernel_proof! {
            assert(after.nodes@.len() == original@.nodes@.len() + 1);
            if case == 7 {
                reveal(retirement_wide::creation);
                assert(after.nodes@[2].proposed == Some(HeadId(7)) && !after.nodes@[2].adopted);
            }
        }
    }
}
}
