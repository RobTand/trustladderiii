//! Reachability for the real Q27 empty-allowed transition, both fail_fast values.
use super::*;

kernel_fn! {
/// The first reply starts a fresh generation. A second reply stops, with the
/// same credit ledger, no question, and no second re-plan. Storage is one node;
/// reply_empty_replan's quantified contract has no node-count bound.
fn check_empty_allowed_replan(fail_fast: bool) -> (result: ()) []
proof [reveal(initial_node); reveal(node_headroom); reveal(arithmetic_consistent);
    reveal(reply_bounds); reveal(retirement::phased);] {
    let mut n = Node::new(RepoId(0));
    n.phase = Phase::Planning { must_split: false };
    n.credit.ready = true;
    n.credit.start = 4;
    let mut nodes = Vec::new();
    nodes.push(n);
    let mut gates = Vec::new();
    gates.push(Gate { w: 1 });
    let mut w = World { nodes, gates, pool_capacity: 1, next_ticket: 0,
        next_question: 7, file_sets: Vec::new(), dependencies: Vec::new(), fail_fast };
    let mut out = Outbox::new();
    let mut budget = ReplanBudget::new(NodeId(0));
    empty_replan::reply_empty_replan(&mut w, 0, &mut budget, &mut out);
    let consumed = budget.used();
    kernel_proof! {
        assert(w.nodes@[0].phase == Phase::Planning { must_split: false });
        assert(w.nodes@[0].generation == 1);
        assert(consumed);
        assert(w.nodes@[0].credit == n.credit);
        assert(w.nodes@[0].question.is_none());
        assert(w.next_question == 7);
        assert(empty_outbox(out));
    }
    empty_replan::reply_empty_replan(&mut w, 0, &mut budget, &mut out);
    let consumed = budget.used();
    kernel_proof! {
        assert(w.nodes@[0].phase == Phase::Stopping(Outcome::Stopped));
        assert(consumed);
        assert(w.nodes@[0].credit == n.credit);
        assert(w.nodes@[0].question.is_none());
        assert(w.next_question == 7);
        assert(empty_outbox(out));
    }
}
}

kernel_fn! {
/// Q27 reachability includes both fail_fast settings.
fn apply_empty_allowed_replan() -> (result: ()) [] {
    check_empty_allowed_replan(false);
    check_empty_allowed_replan(true);
}
}
