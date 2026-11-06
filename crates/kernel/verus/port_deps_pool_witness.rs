//! Q39 / #806 W7: original pool worlds, built from real Node::new values.
use super::port_deps_specs::*;
use super::*;

verus! {
/// #806 original pool node: only priority and optional ticket differ from new.
pub(super) open spec fn pool_node(n: Node) -> bool {
    initial_node(Node { priority: Priority::Unranked, ticket: None, ..n }, RepoId(0))
}

/// #806 original three-root pool domain, retaining all symbolic priorities and
/// tickets. Complete consistency excludes the original zero-capacity branch.
pub(super) open spec fn pool_fixture(w: World) -> bool {
    w.nodes@.len() == 3 && (forall|i: int| 0 <= i < 3 ==> pool_node(w.nodes@[i]))
        && w.gates@ == seq![Gate { w: 3 }] && w.pool_capacity == 1
        && w.next_ticket <= 64 && w.next_question == 0 && !w.fail_fast
        && w.file_sets@.len() == 0 && w.dependencies@.len() == 0
        && (forall|i: int| 0 <= i < 3 && w.nodes@[i].ticket.is_some()
            ==> w.nodes@[i].ticket.unwrap() < w.next_ticket)
        && (forall|i: int, j: int| 0 <= i < j < 3 && w.nodes@[i].ticket.is_some()
            ==> w.nodes@[i].ticket != w.nodes@[j].ticket)
}

/// #806: all original valid pool inputs satisfy the complete production
/// checker premises. No priority relation or ticket order is assumed.
pub(super) proof fn pool_is_checked(w: World)
    requires pool_fixture(w),
    ensures checker_accepted(w), world_headroom(w), storage_ordered(w),
{
    reveal(pool_fixture); reveal(pool_node); reveal(initial_node);
    reveal(checker_accepted); reveal(retirement::node_rules);
    reveal(world_headroom); reveal(storage_ordered); reveal(footprint::ordered_table);
    reveal(dependencies::zero_credit); footprint::inline_is_valid(w, 0);
    reveal_with_fuel(open_prefix, 4); reveal_with_fuel(potential_prefix, 4);
    reveal(pool_users);
    assert forall|i: int| 0 <= i < 3
        implies retirement::node_rules(w, w.nodes@[i]) && !w.nodes@[i].holds by {
        assert(i == 0 || i == 1 || i == 2);
    }
}

/// #806: executable original-domain constructor with arbitrary priorities,
/// optional distinct tickets below next_ticket, and a real complete check.
pub(super) fn pool_world(p0: Priority, p1: Priority, p2: Priority,
    t0: Option<u64>, t1: Option<u64>, t2: Option<u64>, next: u64) -> (world: World)
    requires next <= 64,
        (forall|i: int| 0 <= i < 3 && seq![t0, t1, t2][i].is_some()
            ==> seq![t0, t1, t2][i].unwrap() < next),
        (forall|i: int, j: int| 0 <= i < j < 3 && seq![t0, t1, t2][i].is_some()
            ==> seq![t0, t1, t2][i] != seq![t0, t1, t2][j]),
    ensures pool_fixture(world), world_checked(world), world_headroom(world), storage_ordered(world),
        world.nodes@[0].priority == p0 && world.nodes@[0].ticket == t0,
        world.nodes@[1].priority == p1 && world.nodes@[1].ticket == t1,
        world.nodes@[2].priority == p2 && world.nodes@[2].ticket == t2,
{
    let mut a = Node::new(RepoId(0)); a.priority = p0; a.ticket = t0;
    let mut b = Node::new(RepoId(0)); b.priority = p1; b.ticket = t1;
    let mut c = Node::new(RepoId(0)); c.priority = p2; c.ticket = t2;
    let mut nodes = Vec::new(); nodes.push(a); nodes.push(b); nodes.push(c);
    let mut gates = Vec::new(); gates.push(Gate { w: 3 });
    let world = World { nodes, gates, pool_capacity: 1, next_ticket: next,
        next_question: 0, fail_fast: false, file_sets: Vec::new(), dependencies: Vec::new() };
    proof {
        reveal(pool_fixture); reveal(pool_node); reveal(initial_node);
        assert forall|i: int| 0 <= i < 3 implies pool_node(world.nodes@[i]) by {
            assert(i == 0 || i == 1 || i == 2);
        }
        assert forall|i: int| 0 <= i < 3 && world.nodes@[i].ticket.is_some()
            implies world.nodes@[i].ticket.unwrap() < next by {
            assert(i == 0 || i == 1 || i == 2);
            assert(world.nodes@[i].ticket == seq![t0, t1, t2][i]);
        }
        assert forall|i: int, j: int| 0 <= i < j < 3 && world.nodes@[i].ticket.is_some()
            implies world.nodes@[i].ticket != world.nodes@[j].ticket by {
            assert(i == 0 || i == 1); assert(j == 1 || j == 2);
            assert(world.nodes@[i].ticket == seq![t0, t1, t2][i]);
            assert(world.nodes@[j].ticket == seq![t0, t1, t2][j]);
        }
        assert(pool_fixture(world)); pool_is_checked(world); reveal(port_deps_checker_accepted);
    }
    let accepted = world.check();
    assert(accepted.is_ok());
    world
}
}
