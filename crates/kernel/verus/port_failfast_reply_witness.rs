//! Q54 / #440 / #806: executable diverted and unchanged covers for six replies.
use super::*;

verus! {
/// Q54: exactly the six retired payloads, with a failed second checks round.
pub(super) open spec fn case_reply(case: u8) -> Reply {
    match case {
        0 => Reply::MakerCantMeet,
        1 => Reply::MakerOutOfRoom,
        2 => Reply::MakerNeedsFiles { files: footprint::inline_footprint(2), fits: false },
        3 => Reply::ChangesRequested,
        4 => Reply::ReviewerOutOfRoom,
        _ => Reply::ChecksDone { report: true, pass: false, in_footprint: true },
    }
}

/// Q54: cover input fixes only original-domain values that reach a work retry.
pub(super) open spec fn input(w: World, case: u8) -> bool {
    !w.fail_fast && w.next_question == 0 && w.file_sets@.len() == 0
        && port_failfast_replies::selected(w.nodes@[0], case_reply(case))
        && w.nodes@[0].generation == 0 && current_item(w.nodes@[0]) == Some(Item::Await)
        && w.nodes@[0].run == if case == 5 { None } else { Some(0) }
        && w.nodes@[0].job == if case == 5 { Some(0) } else { None }
        && w.nodes@[0].round == 1 && w.nodes@[0].attempt == 1
        && w.nodes@[0].review_restarted && w.nodes@[0].review_stop.is_none()
        && w.nodes@[0].credit.start == 4 && w.nodes@[0].credit.spent == 0
        && w.nodes@[0].credit.earned == 0 && w.nodes@[0].credit.added == 0
        && !w.nodes@[0].credit.withheld
}

/// Q54 diverted covers: extract the actual accepted nodes under both flags.
/// This proves reachability from real reply_post relations, never from an assumed stop.
pub(super) proof fn changed(before: World, fast: World, off: World, on: World, case: u8,
    off_out: Outbox, on_out: Outbox)
    requires case < 6, retirement::domain(before), input(before, case),
        fast.fail_fast, port_failfast_admission::same_world(before, fast),
        port_review_reply::reply_post(before, off, 0, 0, case_reply(case),
            Outbox { items: [None; OUTBOX], len: 0 }, off_out),
        port_review_reply::reply_post(fast, on, 0, 0, case_reply(case),
            Outbox { items: [None; OUTBOX], len: 0 }, on_out),
    ensures !port_failfast_admission::same_world(off, on),
        on.nodes@[0].phase == Phase::Stopping(Outcome::Stopped),
        on.nodes@[0].review_stop == Some(port_failfast_replies::reason(case_reply(case))),
        on.nodes@[0].question.is_none(), on_out.len == 0,
{
    let reply = case_reply(case);
    let initial = Outbox { items: [None; OUTBOX], len: 0 };
    footprint::inline_is_valid(before, 2);
    footprint::storage_frame(before, fast,
        footprint::inline_footprint(2), footprint::inline_footprint(2));
    reveal(port_review_reply::accepted_reply);
    port_failfast_replies::admitted(before, off, 0, reply, initial, off_out);
    port_failfast_replies::admitted(fast, on, 0, reply, initial, on_out);
    reveal(port_replies_maker::transition_post); reveal(port_replies_maker::work_result);
    reveal(port_replies_maker::files_post); reveal(port_review_reply::transition);
    reveal(port_review_retry::round); reveal(port_review_retry::room);
    reveal(port_review_retry::rung); reveal(retirement::stopped);
    assert(off.nodes@[0].phase == if matches!(case, 1 | 2 | 4) {
        Phase::Planning { must_split: true }
    } else { Phase::Making });
}
}

kernel_fn! {
/// Q54 original cover domain: two consistent settled nodes, two repositories,
/// four-file inline footprints, pool=1, widths=1, small heads, ledgers and IDs.
pub(super) fn world(case: u8) -> (result: World) [
    requires case < 6,
    ensures progress_phase_witness::original(result), retirement::accepted(result),
        retirement::bounded(result), settled_world(result), storage_ordered(result), input(result, case),
] proof [reveal(initial_node); reveal(retirement::accepted); reveal(retirement::node_rules);
    reveal(retirement::bounded); reveal(storage_ordered); reveal(footprint::ordered_table);] {
    let mut first = Node::new(RepoId(0));
    let mut second = Node::new(RepoId(1));
    first.phase = if case < 3 { Phase::Making }
        else if case < 5 { Phase::Reviewing } else { Phase::Checking };
    first.pos = if case < 3 { 2 } else { 1 };
    first.paused = true;
    first.holds = true;
    first.run = if case == 5 { None } else { Some(0) };
    first.job = if case == 5 { Some(0) } else { None };
    first.pool = case != 5;
    first.credit.ready = true;
    first.credit.start = 4;
    first.round = 1;
    first.attempt = 1;
    first.review_restarted = true;
    first.footprint = Footprint::inline(1);
    first.head = Some(HeadId(1));
    first.checked = first.head;
    second.phase = Phase::Closed(Outcome::Stopped);
    second.footprint = Footprint::inline(1);
    let mut nodes = Vec::new();
    nodes.push(first); nodes.push(second);
    let mut gates = Vec::new();
    gates.push(Gate { w: 1 }); gates.push(Gate { w: 1 });
    let w = World { nodes, gates, pool_capacity: 1, next_ticket: 0, next_question: 0,
        dependencies: Vec::new(), file_sets: Vec::new(), fail_fast: false };
    kernel_proof! {
        footprint::inline_is_valid(w, 1);
        assert forall|i: int| 0 <= i < 2 implies retirement::node_rules(w, w.nodes@[i]) by {
            assert(i == 0 || i == 1);
        }
        assert(retirement::accepted(w));
        assert forall|i: int| 0 <= i < 2 implies {
            let n = #[trigger] w.nodes@[i];
            n.generation < 1099511627776u64 && n.next_seq < 1099511627776u64
                && n.credit.start < 1099511627776u64 && n.credit.earned < 1099511627776u64
                && n.credit.added < 1099511627776u64 && n.credit.spent < 1099511627776u64
        } by { assert(i == 0 || i == 1); }
        assert(retirement::bounded(w));
        assert(progress_phase_witness::original(w));
        reveal(settled_world); reveal(movable);
        assert forall|i: int| 0 <= i < 2 implies !movable(w, i) by {
            assert(i == 0 || i == 1);
        }
    }
    w
}
}

kernel_fn! {
/// Q54 / #440: for each of the six retired harnesses, execute both real apply
/// calls and return the original settled input plus a proved diverted or equal pair.
/// Stale generation 1 reaches the unchanged cover; generation 0 reaches the stop.
pub(super) fn cover(case: u8, stale: bool)
    -> (result: (Ghost<World>, (World, Outbox), (World, Outbox))) [
    requires case < 6,
    ensures progress_phase_witness::original(result.0@), retirement::accepted(result.0@),
        settled_world(result.0@), input(result.0@, case),
        port_failfast_admission::same_world(result.1.0, result.2.0) == stale,
        !stale ==> result.2.0.nodes@[0].phase == Phase::Stopping(Outcome::Stopped)
            && result.2.0.nodes@[0].review_stop == Some(port_failfast_replies::reason(case_reply(case)))
            && result.2.0.nodes@[0].question.is_none() && result.2.1.len == 0,
        port_failfast_replies::diverted(result.0@, result.1.0, result.2.0, case_reply(case), result.2.1),
] {
    let w = world(case);
    kernel_snapshot! { before = w }
    let reply = match case {
        0 => Reply::MakerCantMeet,
        1 => Reply::MakerOutOfRoom,
        2 => Reply::MakerNeedsFiles { files: Footprint::inline(2), fits: false },
        3 => Reply::ChangesRequested,
        4 => Reply::ReviewerOutOfRoom,
        _ => Reply::ChecksDone { report: true, pass: false, in_footprint: true },
    };
    kernel_proof! {
        footprint::inline_is_valid(w, 2);
        assert(port_replies_maker_admission::inline_input(w, reply));
    }
    let (off, on, fast) = port_failfast_reply_apply::verify_reply_apply(w, if stale { 1 } else { 0 }, reply);
    kernel_proof! {
        if !stale { changed(before@, fast@, off.0, on.0, case, off.1, on.1); }
    }
    (before, off, on)
}
}
