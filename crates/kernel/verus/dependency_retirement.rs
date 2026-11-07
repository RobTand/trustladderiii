//! #693 / #839: full-domain ports of apply_dependency and
//! dependency_abandoned_never_releases. One node and one external declaration
//! are the original bounds; all Node fields and counters remain unrestricted.
use super::*;

verus! {
/// #839: the original World::new(&[3], 1) domain, also allowing the waiting
/// constructor's W=1. Neither counter headroom nor settledness is assumed.
pub(super) open spec fn domain(w: World) -> bool {
    w.nodes@.len() == 1 && w.gates@.len() == 1 && w.gates@[0].w > 0
        && w.pool_capacity == 1 && w.next_ticket == 0 && w.next_question == 0
        && w.file_sets@.len() == 0 && edge_domain(w)
}

pub(super) open spec fn edge_domain(w: World) -> bool {
    w.nodes@.len() == 1 && w.dependencies@.len() == 1
        && w.dependencies@[0].node == NodeId(0) && !w.dependencies@[0].rejected
        && w.dependencies@[0].target == (DependencyTarget {
            kind: PrerequisiteKind::Issue, number: 2761, request: None })
}

/// #693: every restriction of the REAL dependency checker in this domain,
/// including question continuations and ingest/audit credit, not just progress.
pub(super) open spec fn edge_rules(w: World) -> bool {
    let n = w.nodes@[0];
    w.dependencies@[0].status == DependencyStatus::Delivered || (
        !n.planning_started
        && matches!(n.phase, Phase::Ingesting | Phase::Auditing | Phase::Unplanned
            | Phase::AwaitingRob | Phase::Stopping(_) | Phase::Closed(_))
        && (match n.question { None => true, Some(q) => matches!(q.cont,
            Cont::Ingesting | Cont::Auditing | Cont::Unplanned | Cont::Stop(_) | Cont::Nowhere) })
        && (matches!(n.phase, Phase::Ingesting | Phase::Auditing | Phase::Unplanned)
            ==> dependencies::zero_credit(n.credit))
        && (n.phase == Phase::Unplanned ==> !n.holds && !n.pool
            && n.ticket.is_none() && n.run.is_none()))
}

/// #839: equivalent to every World::check assertion for the original domain.
pub(super) open spec fn accepted(w: World) -> bool {
    retirement::node_rules(w, w.nodes@[0]) && edge_rules(w)
}

/// #839 / Q49: a provisional delivery is revocable only in a wait-safe phase.
pub(super) open spec fn revocable(n: Node) -> bool {
    !n.planning_started
        && matches!(n.phase, Phase::Ingesting | Phase::Auditing | Phase::Unplanned
            | Phase::AwaitingRob | Phase::Stopping(_) | Phase::Closed(_))
        && (match n.question { None => true, Some(q) => matches!(q.cont,
            Cont::Ingesting | Cont::Auditing | Cont::Unplanned | Cont::Stop(_) | Cont::Nowhere) })
        && (n.phase != Phase::Unplanned || (!n.holds && !n.pool
            && n.ticket.is_none() && n.run.is_none()))
}

/// Exact symbolic-status assertion from check_dependency_fact, without bounds.
pub(super) open spec fn expected(w: World, incoming: DependencyStatus) -> DependencyStatus {
    let previous = w.dependencies@[0].status;
    if previous == DependencyStatus::Pending || incoming == DependencyStatus::Delivered
        || (revocable(w.nodes@[0]) && incoming == DependencyStatus::Pending) {
        incoming
    } else { previous }
}

/// #839: actual prepare_dependency return, including duplicate Pending no-op.
pub(super) open spec fn prepared(w: World, incoming: DependencyStatus,
    result: Option<(Option<usize>, Dependency)>) -> bool {
    let previous = w.dependencies@[0];
    let allowed = previous.status == DependencyStatus::Pending
        || (previous.status == DependencyStatus::Abandoned && incoming == DependencyStatus::Delivered)
        || (revocable(w.nodes@[0]) && incoming == DependencyStatus::Pending);
    result == if allowed && !(previous.status == DependencyStatus::Pending
        && incoming == DependencyStatus::Pending) {
        Some((Some(0usize), Dependency { status: incoming, ..previous }))
    } else { None }
}

/// #839: exact real-handler frame and note, sufficient for both retired harnesses.
pub(super) open spec fn fact_result(before: World, after: World, incoming: DependencyStatus,
    out_before: Outbox, out_after: Outbox) -> bool {
    edge_domain(after)
        && after.dependencies@[0].status == expected(before, incoming)
        && after.nodes@[0].phase == before.nodes@[0].phase
        && (prepared(before, incoming, None) ==> after.nodes@ == before.nodes@
            && out_after.len == out_before.len && out_after.items@ == out_before.items@)
        && (!prepared(before, incoming, None) ==> {
            let blocked = incoming != DependencyStatus::Delivered;
            after.nodes@[0] == (Node {
                waiting_prerequisite: if blocked { Some(2761u64) } else { None },
                credit: Credit { withheld: before.nodes@[0].credit.withheld
                    || (blocked && !before.nodes@[0].planning_started
                        && (match before.nodes@[0].phase { Phase::Ingesting | Phase::Auditing | Phase::Unplanned => true, _ => false })
                        && before.nodes@[0].credit.ready), ..before.nodes@[0].credit },
                ..before.nodes@[0]
            }) && if incoming == DependencyStatus::Abandoned {
                out_after.len == out_before.len + 1 && out_after.items@[out_before.len as int]
                    == Some(Out::Notice { node: NodeId(0), notice: Notice::Note(
                        NoteKind::DependencyAbandoned { number: 2761 }) })
            } else { out_after.len == out_before.len && out_after.items@ == out_before.items@ }
        })
}
}

verus! {
/// #839 / apply_dependency: all original consistency assertions, no counter
/// bounds. Only wait metadata and withheld credit can change on the real path.
pub(super) proof fn preserves(before: World, after: World, incoming: DependencyStatus,
    a: Outbox, b: Outbox)
    requires domain(before), accepted(before),
        dependencies::dependency_frame(before, after, 0), fact_result(before, after, incoming, a, b),
    ensures domain(after), accepted(after),
{
    reveal(dependencies::dependency_frame);
    reveal(retirement::node_rules);
    reveal(node_checked);
    reveal(arithmetic_consistent);
    reveal(dependencies::zero_credit);
    footprint::storage_frame(before, after, before.nodes@[0].footprint,
        before.nodes@[0].footprint);
    assert(after.nodes@[0].phase == before.nodes@[0].phase);
    if !prepared(before, incoming, None) {
        if after.nodes@[0].credit.withheld && !before.nodes@[0].credit.withheld {
            let n = before.nodes@[0];
            if let Some(q) = n.question {
                assert(q.kind != QKind::CantMeet && q.kind != QKind::OutOfCredit);
            }
        }
    }
    assert(node_checked(after, after.nodes@[0]));
    assert(retirement::node_rules(after, after.nodes@[0]));
    assert(edge_rules(after));
}

/// #693 / #839: the exact arbitrary-node/status fact harness, through the real
/// production handler. One node/edge is the old bound; counters are unbounded.
pub(super) fn verify_fact(mut world: World, status: DependencyStatus) -> (result: (World, Outbox))
    requires domain(world), accepted(world),
    ensures domain(result.0), accepted(result.0),
        dependencies::dependency_frame(world, result.0, 0),
        pristine(world.nodes@[0]) ==> pristine(result.0.nodes@[0]),
        result.0.dependencies@[0].status == expected(world, status),
        result.0.nodes@[0].phase == world.nodes@[0].phase,
        fact_result(world, result.0, status, Outbox { items: [None; OUTBOX], len: 0 }, result.1),
{
    let ghost before = world;
    let mut out = Outbox::new();
    proof {
        reveal(empty_outbox);
        dependencies::indexable_definition(world);
    }
    let target = world.dependencies[0].target;
    let ghost empty = out;
    proof {
        let canonical = Outbox { items: [None; OUTBOX], len: 0 };
        assert(canonical.items@ =~= empty.items@);
    }
    world.dependency_fact(0, target, status, &mut out);
    proof { preserves(before, world, status, empty, out); }
    proof { reveal(storage_ordered); reveal(footprint::ordered_table);
        reveal(dependency_retirement_domain); reveal(dependency_retirement_accepted); }
    proof { reveal(dependency_retirement_domain); reveal(dependency_retirement_accepted); }
    let checked = world.check();
    assert(checked.is_ok());
    (world, out)
}

/// #693: original constructor's remaining fields, with only dependency/admission
/// fields abstracted so the same facts survive both real updates.
pub(super) open spec fn pristine(n: Node) -> bool {
    initial_node(Node { phase: Phase::Ingesting, generation: 0, planning_started: false,
        waiting_prerequisite: None, credit: Credit { ready: false, start: 0, earned: 0,
            added: 0, spent: 0, merge_credited: false, withheld: false }, ..n }, RepoId(0))
}

/// #693: original one-node waiting constructor, no loop/unwind.
pub(super) fn waiting_world() -> (world: World)
    ensures domain(world), accepted(world), pristine(world.nodes@[0]),
        world.nodes@[0].generation == 0, !world.nodes@[0].planning_started,
        world.nodes@[0].phase == Phase::Unplanned,
        world.nodes@[0].credit == (Credit { ready: true, start: 4, earned: 0, added: 0,
            spent: 0, merge_credited: false, withheld: true }),
        world.nodes@[0].waiting_prerequisite == Some(2761),
        world.dependencies@[0].status == DependencyStatus::Pending,
{
    let mut node = Node::new(RepoId(0));
    node.phase = Phase::Unplanned;
    node.credit.ready = true;
    node.credit.start = 4;
    node.credit.withheld = true;
    node.waiting_prerequisite = Some(2761);
    let mut nodes = Vec::new(); nodes.push(node);
    let mut gates = Vec::new(); gates.push(Gate { w: 1 });
    let mut dependencies = Vec::new(); dependencies.push(Dependency {
        node: NodeId(0), target: DependencyTarget { kind: PrerequisiteKind::Issue,
            number: 2761, request: None }, status: DependencyStatus::Pending, rejected: false });
    let world = World { nodes, gates, dependencies, pool_capacity: 1,
        next_ticket: 0, next_question: 0, file_sets: Vec::new(), fail_fast: false };
    proof {
        reveal(initial_node); reveal(retirement::node_rules); reveal(dependencies::zero_credit);
        footprint::inline_is_valid(world, 0);
    }
    world
}

/// #693 / #839: every assertion of check_dependency_abandoned, including the
/// note, empty second output, symbolic status, exact advance/phase, and full
/// consistency. The old waiter cover is reached by Pending or Abandoned input.
pub(super) fn verify_abandoned(status: DependencyStatus) -> (result: (World, bool))
    ensures domain(result.0), accepted(result.0),
        result.0.dependencies@[0].status == status,
        result.1 == (status == DependencyStatus::Delivered),
        result.0.nodes@[0].phase == if status == DependencyStatus::Delivered {
            Phase::Planning { must_split: false }
        } else { Phase::Unplanned },
        (status != DependencyStatus::Delivered ==> result.0.nodes@[0].waiting_prerequisite.is_some()),
{
    let world = waiting_world();
    let (world, note) = verify_fact(world, DependencyStatus::Abandoned);
    assert(note.len == 1);
    assert(note.items@[0] == Some(Out::Notice { node: NodeId(0),
        notice: Notice::Note(NoteKind::DependencyAbandoned { number: 2761 }) }));
    let (mut world, out) = verify_fact(world, status);
    assert(out.len == 0);
    assert(world.dependencies@[0].status == status);
    let ghost before = world;
    let mut out = Outbox::new();
    proof {
        reveal(empty_outbox); reveal(storage_ordered); reveal(footprint::ordered_table);
        reveal(scheduling_safe); reveal_with_fuel(open_prefix, 2);
        reveal_with_fuel(potential_prefix, 2);
        reveal(advance_pre); reveal(initial_node); reveal(retirement::node_rules);
        reveal(dependencies::dependency_frame);
        dependencies::delivered_definition(world.dependencies@, NodeId(0));
        footprint::inline_is_valid(world, 0);
    }
    let admitted = advance(&mut world, 0, &mut out, &mut Report::new());
    assert(admitted == (status == DependencyStatus::Delivered));
    proof {
        reveal(advance_post); reveal(initial_node); reveal(retirement::node_rules);
        reveal(dependencies::zero_credit);
        footprint::inline_is_valid(world, 0);
        if admitted { assert(world.nodes@[0].phase == (Phase::Planning { must_split: false })); }
        else { assert(world.nodes@[0] == before.nodes@[0]); }
        assert(accepted(world));
    }
    proof { reveal(dependency_retirement_domain); reveal(dependency_retirement_accepted); }
    let checked = world.check();
    assert(checked.is_ok());
    (world, admitted)
}

/// #693 / #839: executable witnesses for all four apply_dependency covers.
/// Cases: Closed+Delivered, Ingesting+Abandoned, Making, Queued. Each calls the
/// real handler and full checker. No symbolic domain is removed by these covers.
fn fact_witness(case: u8) -> (result: World)
    requires case < 4,
    ensures domain(result), accepted(result), result.nodes@[0].phase == match case {
        0 => Phase::Closed(Outcome::Stopped), 1 => Phase::Ingesting,
        2 => Phase::Making, _ => Phase::Queued,
    },
{
    let mut node = Node::new(RepoId(0));
    node.phase = match case { 0 => Phase::Closed(Outcome::Stopped), 1 => Phase::Ingesting,
        2 => Phase::Making, _ => Phase::Queued };
    if case >= 2 {
        node.credit.ready = true; node.credit.start = 4;
        node.holds = true; node.attempt = 1; node.footprint = Footprint::inline(1);
        node.head = Some(HeadId(1)); node.checked = node.head; node.approved = node.head;
        if case == 3 { node.had_pr = true; node.pr_open = true; }
    }
    let mut nodes = Vec::new(); nodes.push(node);
    let mut gates = Vec::new(); gates.push(Gate { w: 3 });
    let mut dependencies = Vec::new(); dependencies.push(Dependency {
        node: NodeId(0), target: DependencyTarget { kind: PrerequisiteKind::Issue,
            number: 2761, request: None }, status: DependencyStatus::Delivered, rejected: false });
    let world = World { nodes, gates, dependencies, pool_capacity: 1, next_ticket: 0,
        next_question: 0, file_sets: Vec::new(), fail_fast: false };
    proof {
        reveal(initial_node); reveal(retirement::node_rules);
        footprint::inline_is_valid(world, 0);
        footprint::inline_is_valid(world, 1);
        assert(accepted(world));
    }
    let status = if case == 0 { DependencyStatus::Delivered } else { DependencyStatus::Abandoned };
    let (world, _) = verify_fact(world, status);
    world
}

/// #693: nonvacuity of the original abandoned waiter cover after real advance.
fn abandoned_witness() -> (result: World)
    ensures result.nodes@[0].waiting_prerequisite.is_some(), accepted(result),
{
    let (world, admitted) = verify_abandoned(DependencyStatus::Abandoned);
    assert(!admitted);
    world
}
}
