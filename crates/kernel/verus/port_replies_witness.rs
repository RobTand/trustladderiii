//! #806 / #832-A: executable original-domain covers, including the spent audit repair.
use super::retirement::*;
use super::*;

kernel_fn! {
/// #806: each reply cover starts in Kani's two-node, two-repository, empty
/// storage construction, fully consistent and settled, with the matching run.
pub(super) fn world(case: u8) -> (result: World) [
    requires case < 10,
    ensures progress_phase_witness::original(result), accepted(result), settled_world(result),
        result.nodes@[0].phase == if case < 3 { Phase::Ingesting }
            else if case < 5 || case >= 8 { Phase::Auditing } else { Phase::Planning { must_split: false } },
        current_item(result.nodes@[0]) == Some(Item::Await),
        result.nodes@[0].run == Some(0), result.nodes@[0].generation == 0,
        result.nodes@[0].early_end.is_none(), result.nodes@[0].audit_rounds == if case >= 8 { 1u8 } else { 0u8 },
        result.nodes@[0].plan_retries == 0, result.nodes@[0].credit.start == 4,
        result.next_question == 0, result.fail_fast == (case == 9), result.file_sets@.len() == 0,
] proof [reveal(initial_node); reveal(accepted); reveal(node_rules); reveal(bounded);
    reveal(storage_ordered); reveal(footprint::ordered_table);] {
    let mut first = Node::new(RepoId(0));
    let mut second = Node::new(RepoId(1));
    first.phase = if case < 3 { Phase::Ingesting }
        else if case < 5 || case >= 8 { Phase::Auditing } else { Phase::Planning { must_split: false } };
    first.audit_rounds = if case >= 8 { 1 } else { 0 };
    first.pos = 1;
    first.paused = true;
    first.credit.ready = true;
    first.credit.start = 4;
    first.footprint = Footprint::inline(1);
    first.run = Some(0);
    first.pool = true;
    second.phase = Phase::Closed(Outcome::Stopped);
    second.footprint = Footprint::inline(1);
    let mut nodes = Vec::new();
    nodes.push(first);
    nodes.push(second);
    let mut gates = Vec::new();
    gates.push(Gate { w: 1 });
    gates.push(Gate { w: 1 });
    let w = World { nodes, gates, pool_capacity: 1, next_ticket: 0, next_question: 0,
        file_sets: Vec::new(), dependencies: Vec::new(), fail_fast: case == 9 };
    kernel_proof! {
        footprint::inline_is_valid(w, 1);
        assert forall|j: int| 0 <= j < 2 implies node_rules(w, w.nodes@[j]) by {
            assert(j == 0 || j == 1);
        }
        assert(accepted(w));
        assert forall|j: int| 0 <= j < 2 implies {
            let n = #[trigger] w.nodes@[j];
            n.generation < 1099511627776u64 && n.next_seq < 1099511627776u64
                && n.credit.start < 1099511627776u64 && n.credit.earned < 1099511627776u64
                && n.credit.added < 1099511627776u64 && n.credit.spent < 1099511627776u64
        } by { assert(j == 0 || j == 1); }
        reveal(settled_world); reveal(movable);
        assert forall|j: int| 0 <= j < 2 implies !movable(w, j) by {
            assert(j == 0 || j == 1);
        }
        assert(progress_phase_witness::original(w));
    }
    w
}
}

kernel_fn! {
/// #806 / #832-A changed-world covers: executable constructors for the eight
/// payloads run the real apply/admission/handler path and return both worlds.
/// Cases 8/9 stop the second audit disagreement with fail_fast off/on.
pub(super) fn cover(case: u8) -> (result: (World, World)) [
    requires case < 10,
    ensures progress_phase_witness::original(result.0), accepted(result.0), settled_world(result.0),
        domain(result.1), accepted(result.1), result.1.nodes@ != result.0.nodes@,
        result.0.nodes@[0].generation == 0, result.1.nodes@[0].generation == 1,
        result.1.nodes@[0].run.is_none(),
        result.1.nodes@[0].phase == match case {
            0 | 1 => Phase::Auditing,
            2 => Phase::Stopping(Outcome::NotActionable),
            3 => Phase::Unplanned,
            4 => Phase::Ingesting,
            5 => Phase::Ready(Resume::Making),
            6 => Phase::AwaitingRob,
            8 | 9 => Phase::Stopping(Outcome::Stopped),
            _ => Phase::Planning { must_split: false },
        },
        case == 1 ==> result.1.nodes@[0].early_end == Some(EarlyEnd::Duplicate),
        case == 5 ==> result.1.nodes@[0].attempt == 1,
        case == 6 ==> result.1.nodes@[0].question.is_some()
            && result.1.nodes@[0].question.unwrap().kind == QKind::NeedsSplitting,
        case >= 8 ==> result.0.fail_fast == (case == 9) && result.0.nodes@[0].audit_rounds == 1
            && result.1.nodes@[0].review_stop == Some(ReviewStop::AuditDisagreement)
            && result.1.nodes@[0].question.is_none()
            && result.1.nodes@[0].credit == result.0.nodes@[0].credit
            && result.1.next_question == result.0.next_question,
] proof [reveal(port_replies_contracts::transitioned); reveal(port_replies_contracts::plan_retry_node);
    reveal(retirement::stopped); reveal(port_replies_contracts::ingested);] {
    let w = world(case);
    let before = snapshot_world(&w);
    kernel_proof! {
        assert(before.file_sets@ == w.file_sets@);
        assert(progress_phase_witness::original(before));
        reveal(accepted);
        retirement::node_frame(w, before, w.nodes@[0]);
        retirement::node_frame(w, before, w.nodes@[1]);
        port_replies_frames::world_frame(w, before);
        assert(accepted(before));
        reveal(settled_world);
        assert forall|j: int| 0 <= j < 2 implies !movable(before, j) by {
            movable_frame(w, before, j);
        }
        assert(settled_world(before));
        progress_phase::reserves(w);
    }
    let reply = match case {
        0 => Reply::IngestDone { size: 1 },
        1 => Reply::IngestEarlyEnd(EarlyEnd::Duplicate),
        2 => Reply::ProtectedPaths,
        3 => Reply::AuditAgrees,
        4 | 8 | 9 => Reply::AuditDisagrees,
        5 => Reply::PlanLeaf { footprint: Footprint::inline(1), fits: true },
        6 => Reply::PlanSplit,
        _ => Reply::PlanCantMeet,
    };
    kernel_proof! { footprint::inline_is_valid(w, 1); }
    let (after, _) = port_replies_exec::verify_reply_apply(w, 0, reply);
    (before, after)
}
}
