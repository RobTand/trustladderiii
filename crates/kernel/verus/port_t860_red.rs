//! #860: RED reproduction of the stale pre-#832-A fail-fast rule.
use super::*;

kernel_fn! {
/// Auditing / Q27 #832-A: the old flag-off rule rejects the approved second stop.
/// Bound: two original-domain nodes, two gates, one real apply, counters below 8.
fn stale_flag_off_rule() -> (result: World) [] proof [
    reveal(initial_node); reveal(world_headroom); reveal(node_headroom);
    reveal(arithmetic_consistent); reveal(footprint::ordered_table);
    reveal_with_fuel(open_prefix, 3);
] {
    let mut n = Node::new(RepoId(0));
    n.phase = Phase::Auditing;
    n.pos = 1;
    n.credit.ready = true;
    n.credit.start = 4;
    n.run = Some(0);
    n.pool = true;
    n.audit_rounds = 1;
    let mut other = Node::new(RepoId(1));
    other.phase = Phase::Closed(Outcome::Stopped);
    let mut nodes = Vec::new();
    nodes.push(n);
    nodes.push(other);
    let mut gates = Vec::new();
    gates.push(Gate { w: 1 });
    gates.push(Gate { w: 1 });
    let mut w = World { nodes, gates, pool_capacity: 1, next_ticket: 0, next_question: 7,
        file_sets: Vec::new(), dependencies: Vec::new(), fail_fast: false };
    let mut out = Outbox::new();
    kernel_proof! {
        footprint::inline_is_valid(w, 0);
        dependencies::indexable_definition(w);
        assert forall|i: int| 0 <= i < 2 implies node_headroom(w.nodes@[i]) by {}
        assert(world_headroom(w));
    }
    apply(&mut w, Event::Reply { node: NodeId(0), generation: 0,
        reply: Reply::AuditDisagrees }, &mut out);
    kernel_proof! {
        assert(w.nodes@[0].review_stop == Some(ReviewStop::AuditDisagreement));
        // The retired harness permits only PublicationBody with fail_fast off.
        assert(w.nodes@[0].review_stop.is_none());
    }
    w
}
}
