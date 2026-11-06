//! #806: executable original-domain covers for the six review/rebase harnesses.
use super::*;

verus! {
/// #806 original covers: 0 approval, 1 changes, 2..6 review exhaustion,
/// 7..9 clean rebase, 10..12 conflict fix, 13 failed run.
pub(super) open spec fn case_reply(case: u8) -> Reply {
    if case == 0 { Reply::Approved { head: HeadId(1) } }
    else if case == 1 { Reply::ChangesRequested }
    else if case < 7 { Reply::ReviewerOutOfRoom }
    else if case < 10 { Reply::RebaseClean { head: HeadId(2) } }
    else if case < 13 { Reply::RebaseNeedsChanges }
    else { Reply::RunFailed }
}

/// Original reviewer/rebase cover predicates, including each free/funded partition.
pub(super) open spec fn cover_node(n: Node, case: u8) -> bool {
    n.round == 0 && n.run_failures == 0
        && n.phase == if 7 <= case < 13 { Phase::Rebasing } else { Phase::Reviewing }
        && current_item(n) == Some(Item::Await) && n.run == Some(0) && n.generation == 0
        && n.review_restarted == (case == 5 || case == 6)
        && n.rechecks == if matches!(case, 3 | 4 | 8 | 9 | 11) { FREE_RECHECKS } else { 0u8 }
        && port_review_retry::funded(n) == !matches!(case, 4 | 6 | 9 | 12)
        && n.head == Some(HeadId(1)) && n.checked == n.head
}

/// #806: each constructor fixes fail_fast off and retains all original input partitions.
pub(super) open spec fn cover_input(w: World, case: u8) -> bool {
    !w.fail_fast && cover_node(w.nodes@[0], case)
}

/// #806 changed-world covers: node-local equations make every constructed case change.
pub(super) proof fn changed_node(n: Node, after: Node, id: u64, case: u8)
    requires case < 14, cover_node(n, case),
        after == if case < 13 {
            port_review_reply::transition(port_review_reply::ended(n), false, id, case_reply(case)).0
        } else { port_review_retry::failed_run(n) },
    ensures after != n,
{
    reveal(port_review_reply::transition); reveal(port_review_retry::round);
    reveal(port_review_retry::room); reveal(port_review_retry::rechecked);
    reveal(port_review_retry::failed_run); reveal(retirement::incident);
    if case == 0 { assert(after.phase == Phase::Publishing); }
    else if case == 1 { assert(after.phase == Phase::Making); }
    else if case < 7 {
        if n.review_restarted { assert(after.phase != n.phase); }
        else { assert(after.review_restarted); }
    } else if case < 13 { assert(after.phase != n.phase); }
    else { assert(after.run.is_none()); }
}

/// #806 covers: extract only the actor equation from the opaque real-apply relation.
pub(super) proof fn changed_world(before: World, after: World, out: Outbox, case: u8)
    requires case < 14, cover_input(before, case),
        port_review_reply::reply_post(before, after, 0, 0, case_reply(case),
            Outbox { items: [None; OUTBOX], len: 0 }, out),
    ensures after.nodes@[0] != before.nodes@[0],
{
    reveal(port_review_reply::reply_post); reveal(port_review_reply::reply_result);
    reveal(port_review_reply::accepted_reply); reveal(port_review_reply::transition_post);
    changed_node(before.nodes@[0], after.nodes@[0], before.next_question, case);
}

}

kernel_fn! {
/// #806: fourteen finite settled worlds cover all seventeen original covers.
/// Each uses exactly two nodes, two repositories, four-file inline footprints,
/// pool<=2, W<=3, heads<8, ledger<2^30, and counters/IDs<2^40.
fn world(case: u8) -> (result: World) [
    requires case < 14,
    ensures progress_phase_witness::original(result), retirement::domain(result),
        retirement::accepted(result), retirement::bounded(result), storage_ordered(result),
        settled_world(result), cover_input(result, case),
] proof [
    reveal(initial_node); reveal(retirement::accepted); reveal(retirement::node_rules);
    reveal(retirement::bounded); reveal(storage_ordered); reveal(footprint::ordered_table);
] {
    let mut first = Node::new(RepoId(0));
    let mut second = Node::new(RepoId(1));
    first.phase = if 7 <= case && case < 13 { Phase::Rebasing } else { Phase::Reviewing };
    first.pos = 1;
    first.holds = true;
    first.paused = true;
    first.run = Some(0);
    first.pool = case < 7 || case == 13;
    first.credit.ready = true;
    first.credit.start = if matches!(case, 4 | 6 | 9 | 12) { 0 } else { 4 };
    first.rechecks = if matches!(case, 3 | 4 | 8 | 9 | 11) { FREE_RECHECKS } else { 0 };
    first.review_restarted = case == 5 || case == 6;
    first.attempt = 1;
    first.footprint = Footprint::inline(1);
    first.head = Some(HeadId(1));
    first.checked = first.head;
    second.phase = Phase::Closed(Outcome::Stopped);
    second.footprint = Footprint::inline(1);
    let mut nodes = Vec::new();
    nodes.push(first);
    nodes.push(second);
    let mut gates = Vec::new();
    gates.push(Gate { w: 1 });
    gates.push(Gate { w: 1 });
    let w = World { nodes, gates, pool_capacity: 1, next_ticket: 0, next_question: 0,
        dependencies: Vec::new(), file_sets: Vec::new(), fail_fast: false };
    kernel_proof! {
        footprint::inline_is_valid(w, 1);
        assert forall|i: int| 0 <= i < 2 implies retirement::node_rules(w, w.nodes@[i]) by {
            assert(i == 0 || i == 1);
        }
        assert(retirement::accepted(w));
        assert forall|i: int| 0 <= i < w.nodes@.len() implies {
            let n = #[trigger] w.nodes@[i];
            n.generation < 1099511627776u64 && n.next_seq < 1099511627776u64
                && n.credit.start < 1099511627776u64 && n.credit.earned < 1099511627776u64
                && n.credit.added < 1099511627776u64 && n.credit.spent < 1099511627776u64
        } by { assert(i == 0 || i == 1); }
        assert(retirement::bounded(w));
        assert(progress_phase_witness::original(w));
        reveal(settled_world); reveal(movable);
        assert forall|i: int| 0 <= i < w.nodes@.len() implies !movable(w, i) by {
            assert(i == 0 || i == 1);
        }
    }
    w
}
}

kernel_fn! {
/// #806: constructive reachability, returning the original world and the real
/// apply result. Each case establishes the original input partition and a change.
fn cover(case: u8) -> (result: (Ghost<World>, World, Outbox)) [
    requires case < 14,
    ensures progress_phase_witness::original(result.0@), retirement::accepted(result.0@),
        settled_world(result.0@), cover_input(result.0@, case),
        result.1.nodes@[0] != result.0@.nodes@[0],
        retirement::accepted(result.1), retirement::notices_only(result.2),
] {
    let w = world(case);
    kernel_snapshot! { before = w }
    let reply = if case == 0 { Reply::Approved { head: HeadId(1) } }
        else if case == 1 { Reply::ChangesRequested }
        else if case < 7 { Reply::ReviewerOutOfRoom }
        else if case < 10 { Reply::RebaseClean { head: HeadId(2) } }
        else if case < 13 { Reply::RebaseNeedsChanges }
        else { Reply::RunFailed };
    let (after, out) = port_review_world::verify_apply(w, 0, reply);
    kernel_proof! { changed_world(before@, after, out, case); }
    (before, after, out)
}
}
