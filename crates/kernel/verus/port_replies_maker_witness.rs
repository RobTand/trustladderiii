//! #806: executable original-domain witnesses for all four maker harnesses.
use super::retirement::*;
use super::*;

kernel_fn! {
/// #806 maker covers: the original consistent, settled two-node construction,
/// including four-file inline storage, small heads/counters and both funding cases.
pub(super) fn world(unfunded: bool) -> (result: World) [
    ensures progress_phase_witness::original(result), accepted(result), settled_world(result),
        bounded(result), storage_ordered(result),
        result.nodes@[0].phase == Phase::Making, current_item(result.nodes@[0]) == Some(Item::Await),
        result.nodes@[0].run == Some(0), result.nodes@[0].generation == 0,
        result.nodes@[0].credit.start == 4,
        result.nodes@[0].credit.spent == if unfunded { 4u64 } else { 0u64 },
        port_review_retry::funded(result.nodes@[0]) == !unfunded,
        result.nodes@[0].attempt == 1, !result.nodes@[0].replanned,
        result.nodes@[0].footprint == footprint::inline_footprint(1),
        result.next_question == 0, result.fail_fast == unfunded, result.file_sets@.len() == 0,
] proof [reveal(initial_node); reveal(accepted); reveal(node_rules); reveal(bounded);
    reveal(storage_ordered); reveal(footprint::ordered_table);] {
    let mut first = Node::new(RepoId(0));
    let mut second = Node::new(RepoId(1));
    first.phase = Phase::Making;
    first.pos = 2;
    first.paused = true;
    first.holds = true;
    first.credit.ready = true;
    first.credit.start = 4;
    first.credit.spent = if unfunded { 4 } else { 0 };
    first.attempt = 1;
    first.footprint = Footprint::inline(1);
    first.head = Some(HeadId(1));
    first.checked = first.head;
    first.approved = first.head;
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
        file_sets: Vec::new(), dependencies: Vec::new(), fail_fast: unfunded };
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
/// #806 changed-world covers: each original harness's event runs real apply.
/// Case 4 additionally reaches the Q54 unfunded-extension credit question.
pub(super) fn cover(case: u8) -> (result: (World, World)) [
    requires case < 5,
    ensures progress_phase_witness::original(result.0), accepted(result.0), settled_world(result.0),
        domain(result.1), accepted(result.1), result.1.nodes@ != result.0.nodes@,
        result.0.nodes@[0].generation == 0, result.1.nodes@[0].generation == 1,
        result.1.nodes@[0].run.is_none(),
        result.1.nodes@[0].phase == match case {
            0 => Phase::Checking, 1 => Phase::Making,
            2 => Phase::Planning { must_split: true },
            3 => Phase::Ready(Resume::Making), _ => Phase::AwaitingRob,
        },
        case == 0 ==> result.1.nodes@[0].head == Some(HeadId(2)) && result.1.nodes@[0].checked.is_none(),
        case == 1 ==> result.1.nodes@[0].attempt == 2,
        case == 3 || case == 4 ==> result.1.nodes@[0].footprint == footprint::inline_footprint(3),
        case == 4 ==> result.1.nodes@[0].question.is_some()
            && result.1.nodes@[0].question.unwrap().kind == QKind::OutOfCredit
            && result.1.nodes@[0].question.unwrap().cont == Cont::Ready(Resume::Making)
            && result.1.nodes@[0].review_stop.is_none()
            && result.1.nodes@[0].credit == result.0.nodes@[0].credit,
] {
    let w = world(case == 4);
    kernel_snapshot! { original = w }
    let before = snapshot_world(&w);
    kernel_proof! {
        assert(before.file_sets@ == w.file_sets@);
        assert(progress_phase_witness::original(before));
        reveal(accepted);
        retirement::node_frame(w, before, w.nodes@[0]);
        retirement::node_frame(w, before, w.nodes@[1]);
        port_replies_frames::world_frame(w, before);
        reveal(settled_world);
        assert forall|j: int| 0 <= j < 2 implies !movable(before, j) by {
            movable_frame(w, before, j);
        }
        progress_phase::reserves(w);
    }
    let reply = match case {
        0 => Reply::MakerDone { head: HeadId(2) },
        1 => Reply::MakerCantMeet,
        2 => Reply::MakerOutOfRoom,
        _ => Reply::MakerNeedsFiles { files: Footprint::inline(2), fits: true },
    };
    kernel_proof! {
        footprint::inline_is_valid(w, 2);
        assert(port_replies_maker_admission::inline_input(w, reply));
    }
    let (after, out) = port_replies_maker_exec::verify_apply(w, 0, reply);
    kernel_proof! {
        let outputs = choose|o: Outbox| empty_outbox(o)
            && port_review_reply::reply_post(original@, after, 0, 0, reply, o, out);
        reveal(port_review_reply::reply_post); reveal(port_review_reply::reply_result);
        reveal(port_review_reply::transition_post); reveal(port_replies_contracts::transition_post);
        reveal(port_replies_maker::transition_post); reveal(port_review_reply::accepted_reply);
        reveal(port_replies_maker::work_result); reveal(port_review_retry::rung); reveal(port_review_retry::room);
        reveal(port_replies_maker::files_post); reveal(port_replies_maker::union_post);
        if case >= 3 {
            port_replies_maker_frames::inline_files(original@, after, original@.nodes@[0],
                1, 2, true, outputs, out);
            assert(1u64 | 2u64 == 3u64) by (bit_vector);
        }
        if case == 0 {
            assert(after.nodes@[0].phase == Phase::Checking);
        } else if case == 1 {
            assert(after.nodes@[0].phase == Phase::Making);
        } else if case == 2 {
            assert(after.nodes@[0].phase == Phase::Planning { must_split: true });
        } else if case == 3 {
            assert(after.nodes@[0].phase == Phase::Ready(Resume::Making));
        } else {
            assert(after.nodes@[0].phase == Phase::AwaitingRob);
        }
    }
    (before, after)
}
}
