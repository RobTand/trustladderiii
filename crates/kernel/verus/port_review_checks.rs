//! #806: original-domain ChecksDone cover and the exact rebase applicability gate.
use super::*;

kernel_fn! {
/// Checking / #806: a settled bounded original-domain world awaiting its checks job.
fn world() -> (result: World) [
    ensures progress_phase_witness::original(result), retirement::domain(result),
        retirement::accepted(result), retirement::bounded(result), storage_ordered(result),
        settled_world(result), result.nodes@[0].phase == Phase::Checking,
        current_item(result.nodes@[0]) == Some(Item::Await),
        result.nodes@[0].job == Some(0), result.nodes@[0].generation == 0,
        result.nodes@[0].head == Some(HeadId(1)),
] proof [
    reveal(initial_node); reveal(retirement::accepted); reveal(retirement::node_rules);
    reveal(retirement::bounded); reveal(storage_ordered); reveal(footprint::ordered_table);
] {
    let mut first = Node::new(RepoId(0));
    let mut second = Node::new(RepoId(1));
    first.phase = Phase::Checking;
    first.pos = 1;
    first.holds = true;
    first.paused = true;
    first.job = Some(0);
    first.credit.ready = true;
    first.credit.start = 4;
    first.attempt = 1;
    first.footprint = Footprint::inline(1);
    first.head = Some(HeadId(1));
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
/// ChecksDone / #806: the original event-changes-world cover runs real apply and checker.
fn cover() -> (result: (Ghost<World>, World, Outbox)) [
    ensures progress_phase_witness::original(result.0@), retirement::accepted(result.0@),
        settled_world(result.0@), result.0@.nodes@[0].phase == Phase::Checking,
        result.1.nodes@[0] != result.0@.nodes@[0],
        retirement::accepted(result.1), retirement::notices_only(result.2),
] {
    let w = world();
    kernel_snapshot! { before = w }
    let reply = Reply::ChecksDone { report: true, pass: true, in_footprint: true };
    let (after, out) = port_review_world::verify_apply(w, 0, reply);
    kernel_proof! {
        reveal(port_review_reply::reply_post); reveal(port_review_reply::reply_result);
        reveal(port_review_reply::accepted_reply); reveal(port_review_reply::transition_post);
        reveal(port_review_reply::transition);
        assert(after.nodes@[0].phase == Phase::Reviewing);
    }
    (before, after, out)
}
}

kernel_fn! {
/// Rebase progress gate / #806: exact original Node::new domain, both agenda positions.
/// This executes real Node::current, then the original applicability expression and assertion.
fn rebase_gate(pos: u8) -> (result: (Node, bool)) [
    requires pos < 2,
    ensures result.0.phase == Phase::Rebasing, result.0.pos == pos,
        result.0.run == Some(result.0.generation), result.0.generation == 0,
        result.1 == (current_item(result.0) == Some(Item::Await)),
        result.1 == (pos == 1),
        initial_node(Node { phase: Phase::Ingesting, pos: 0, run: None, ..result.0 }, RepoId(0)),
] proof [reveal(initial_node);] {
    let mut node = Node::new(RepoId(0));
    node.phase = Phase::Rebasing;
    node.run = Some(node.generation);
    node.pos = pos;
    let awaiting = node.current() == Some(Item::Await);
    let applies = node.phase == Phase::Rebasing
        && node.current() == Some(Item::Await) && node.run == Some(node.generation);
    assert!(applies == awaiting);
    (node, applies)
}
}

kernel_fn! {
/// #806 original cover: the rebase has not reached Await.
fn before_await() -> (result: (Node, bool)) [
    ensures result.0.phase == Phase::Rebasing, result.0.pos == 0,
        result.0.run == Some(result.0.generation), result.0.generation == 0, !result.1,
        current_item(result.0) != Some(Item::Await),
        initial_node(Node { phase: Phase::Ingesting, pos: 0, run: None, ..result.0 }, RepoId(0)),
] { rebase_gate(0) }
}

kernel_fn! {
/// #806 original cover: the rebase is awaiting its reply.
fn awaiting() -> (result: (Node, bool)) [
    ensures result.0.phase == Phase::Rebasing, result.0.pos == 1,
        result.0.run == Some(result.0.generation), result.0.generation == 0, result.1,
        current_item(result.0) == Some(Item::Await),
        initial_node(Node { phase: Phase::Ingesting, pos: 0, run: None, ..result.0 }, RepoId(0)),
] { rebase_gate(1) }
}
