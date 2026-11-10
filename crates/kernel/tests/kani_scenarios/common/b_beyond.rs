//! The same assertions on enumerated and sampled inputs beyond Kani's shapes.
use super::*;
use std::time::Instant;
include!("b_beyond_checks.rs");

const BIG_SEED: u64 = 0x4b35_1110_beef_b001;
const SAMPLES: usize = 48;

// Only generator bounds are lifted. Consistency, settledness and scenario
// assumptions remain in force in the verbatim assertion functions above.
fn any_world(phase: Option<Phase>) -> World {
    let w = sampling::world(phase);
    kani::assume(w.consistent());
    w
}
fn any_settled_world(phase: Option<Phase>) -> World {
    let w = any_world(phase);
    kani::assume(w.is_settled());
    w
}
fn waiting_dependency_world() -> World { any_world(None) }
fn gate_world(width: u8) -> World {
    let w = any_world(None);
    assert_eq!(w.gates[0].w, width);
    w
}
fn gate_world_in_phase(width: u8, first: Option<Phase>) -> World {
    let w = gate_world(width);
    if let Some(phase) = first { assert_eq!(w.nodes[0].phase, phase); }
    w
}
fn pool_world() -> World { any_world(None) }

fn cold_node(repo: u64) -> Node {
    let mut n = Node::new(RepoId(repo));
    n.phase = Phase::Closed(Outcome::Stopped);
    n
}

fn large_world(actor: Node, index: usize) -> World {
    let count = [8, 12, 72][index % 3];
    let mut w = World::new(&[2, 3, 4], 6);
    w.next_ticket = 1024;
    w.next_question = 1024;
    w.nodes.push(actor);
    for i in 1..count { w.nodes.push(cold_node((i % 3) as u64)); }
    for i in 0..count {
        let offset = if i < 2 { 0 } else { 512 * i as u64 };
        let files = (0..(65 + index % 16)).map(|j| FileId(offset + j as u64));
        let footprint = w.intern_footprint(files);
        w.nodes[i].footprint = footprint;
    }
    // Exercise all four row states on actual owners in each repository.
    for i in 1..count {
        w.dependencies.push(Dependency {
            node: NodeId(i as u64),
            target: DependencyTarget { kind: PrerequisiteKind::Issue,
                number: 10000 + i as u64, request: None },
            status: statuses()[i % 3], rejected: i % 4 == 0,
        });
    }
    w
}

fn actor(phase: Phase, index: usize) -> Node {
    let mut w = super::deterministic_world(phase, index, BIG_SEED ^ index as u64);
    let len = agenda(phase).len();
    let pos = if len == 0 {0} else {index % len};
    let d = [pos, 0, index % 3, 0, index % 3, index / 3 % 2,
        index / 6 % 2, index / 12 % 4, 1];
    super::configure_actor(&mut w, &d, index);
    let mut n = w.nodes[0];
    n.waiting_incident = false; n.incident_needs_rob = false;
    n
}

fn run_cases(name: &str, mut visit: impl FnMut(usize, &mut Stats)) {
    assert_source_identity();
    let start = Instant::now(); let mut stats = Stats::default();
    for index in 0..SAMPLES { visit(index, &mut stats); }
    stats.finish(name, BIG_SEED, start);
}

pub fn dependencies(kind: usize) {
    run_cases(&format!("beyond_dependency_{kind}"), |index, stats| {
        let mut n = Node::new(RepoId(0));
        n.phase = if kind == 0 { phases()[index % phases().len()] } else {Phase::Unplanned};
        n.credit.ready = true; n.credit.start = if kind >= 3 {4} else {0};
        let mut w = large_world(n, index);
        if kind >= 3 {
            w.nodes[0].credit.withheld = true;
            w.nodes[0].waiting_prerequisite = Some(2761);
            // H4's empty footprint is a semantic premise of its assertion.
            w.nodes[0].footprint = Footprint::inline(0);
            w.dependencies.insert(0, Dependency { node: NodeId(0),
                target: DependencyTarget { kind: PrerequisiteKind::Issue, number: 2761, request: None },
                status: DependencyStatus::Pending, rejected: false });
        }
        if kind == 1 {
            w.nodes[0].phase = Phase::Ingesting;
            w.dependencies.push(Dependency {node: NodeId(0),
                target: DependencyTarget {kind: PrerequisiteKind::Issue, number: 2761, request: None},
                status: DependencyStatus::Pending, rejected: false});
        }
        if kind == 2 {
            w.nodes[0].phase = Phase::Ingesting;
            // A >64-node linked chain takes the heap traversal in dependency_path.
            for i in 1..w.nodes.len() { w.nodes[i] = Node::new(RepoId(0)); }
            w.dependencies.clear();
            for i in 1..w.nodes.len()-1 {
                w.dependencies.push(Dependency {node: NodeId(i as u64),
                    target: DependencyTarget {kind: PrerequisiteKind::Issue, number: i as u64,
                        request: Some(NodeId(i as u64+1))}, status: DependencyStatus::Pending, rejected: false});
            }
        }
        stats.case(index, BIG_SEED, Some(w), vec![], || {
            if kind == 3 { check_dependency_waiting(); return; }
            if kind == 4 { check_dependency_delivery(); return; }
            let before = any_world(None);
            let event = Event::Fact { node: NodeId(0), fact: if kind == 1 {
                Fact::DependencyRemove {kind: PrerequisiteKind::Issue, number: 2761}
            } else {
                Fact::Dependency {target: DependencyTarget {kind: PrerequisiteKind::Issue, number: 2762,
                    request: if kind == 2 {Some(NodeId(1))} else {None}},
                    status: statuses()[index % 3]}
            }};
            check_apply(event);
            let mut after = before.clone();
            apply(&mut after, event, &mut Outbox::new(), &mut Report::new());
            assert!(after.dependency_graph_acyclic());
            if kind == 1 { assert_eq!(after.dependencies.len(), before.dependencies.len()-1); }
            if kind == 0 {
                let added = !before.nodes[0].planning_started && matches!(before.nodes[0].phase,
                    Phase::Ingesting | Phase::Auditing | Phase::Unplanned);
                assert_eq!(after.dependencies.len(), before.dependencies.len()+usize::from(added));
            }
        });
    });
}

pub fn adoption() {
    run_cases("beyond_adoption", |index, stats| {
        let mut n = actor(Phase::Making, index);
        n.pos = 0; n.run = None; n.pool = false; n.ticket = None;
        n.proposed = Some(HeadId(1000 + index as u64)); n.adopted = index % 2 == 0;
        n.effect = Some(InFlight {seq: n.next_seq-1, kind: EffectKind::PrepareWorktree, cancelling: false});
        let w = large_world(n, index);
        stats.case(index, BIG_SEED, Some(w), vec![], || {
            check_adoption_at_most_once();
            if index % 2 != 0 {check_adoption_has_no_maker_credit();}
        });
    });
}

fn ready(index: usize) -> Node {
    let mut n = Node::new(RepoId(0));
    n.phase = Phase::Ready(Resume::Making); n.ticket = Some(10 + index as u64);
    n.credit.ready = true; n.credit.start = 4; n.attempt = 1;
    n
}

pub fn gate(planning: bool) {
    run_cases(if planning {"beyond_gate_planning"} else {"beyond_gate_grant"}, |index, stats| {
        let mut n = ready(0);
        if planning {n.phase = Phase::Unplanned; n.ticket = None;}
        let mut w = large_world(n, index);
        w.gates[0].w = 2 + (index % 2) as u8;
        // Larger active queues, with younger tickets and overlapping interned sets.
        for i in 1..w.nodes.len() {
            if i % 3 == 0 { let files = w.nodes[i].footprint; w.nodes[i] = ready(i); w.nodes[i].footprint = files; }
        }
        let width = w.gates[0].w;
        stats.case(index, BIG_SEED, Some(w), vec![], || {
            if planning {check_gate_planning(width);} else {check_gate_grant(width);}
        });
    });
}

pub fn rebase_gate() {
    run_cases("beyond_rebase_gate", |index, stats| {
        let w = large_world(actor(Phase::Rebasing, index), index);
        stats.case(index, BIG_SEED, Some(w), vec![], || {
            let w = any_world(None);
            let n = &w.nodes[0];
            let generation = n.generation + (index / 2 % 2) as u64;
            let expected = n.run == Some(generation) && n.phase == Phase::Rebasing && n.pos == 1;
            assert_eq!(rebase_progress_applies(n, generation), expected);
            check_apply(Event::Reply {node: NodeId(0), generation, reply: Reply::RebaseNeedsChanges});
        });
    });
}

pub fn publishing(earlier: bool) {
    run_cases(if earlier {"beyond_publishing_earlier"} else {"beyond_publishing_ci"}, |index, stats| {
        let w = large_world(actor(Phase::Publishing, index), index);
        let draws = if earlier {vec![Draw::Bool(index % 2 == 0)]} else {vec![]};
        stats.case(index, BIG_SEED, Some(w), draws, || {
            if earlier {check_publishing_ci_earlier_step();} else {check_publishing_ci_tests();}
        });
    });
}

pub fn fail_fast() {
    run_cases("beyond_fail_fast", |index, stats| {
        for kind in 0..9 {
            let phase = match kind {0 => Phase::Planning {must_split: index % 2 == 0}, 1..=3 => Phase::Making,
                4 => Phase::Checking, 5..=6 => Phase::Reviewing, 7 => Phase::AwaitingDelivery, _ => Phase::Publishing};
            let w = large_world(actor(phase, index), index);
            let generation = w.nodes[0].generation;
            let event = match kind {
                7|8 => fact(Fact::CiFailed {infra: index % 2 == 0}),
                _ => Event::Reply {node: NodeId(0), generation, reply: match kind {
                    0 => Reply::PlanCantMeet, 1 => Reply::MakerNeedsFiles {files: w.nodes[0].footprint, fits: index % 2 == 0},
                    2 => Reply::MakerCantMeet, 3 => Reply::MakerOutOfRoom,
                    4 => Reply::ChecksDone {report: index & 1 != 0, pass: index & 2 != 0, in_footprint: index & 4 != 0},
                    5 => Reply::ChangesRequested, _ => Reply::ReviewerOutOfRoom}},
            };
            stats.case(index * 9 + kind, BIG_SEED, Some(w), vec![], || {check_fail_fast_diverts(phase, event);});
        }
    });
}

pub fn gate_leaf() {
    run_cases("beyond_gate_leaf", |index, stats| {
        let mut w = large_world(ready(0), index);
        w.nodes[0].ticket = Some(1);
        let mut blocker = actor(Phase::Rebasing, index);
        blocker.pos = 1; blocker.run = Some(blocker.generation); blocker.pool = false;
        blocker.effect = None; blocker.ticket = None; blocker.pr_open = true; blocker.had_pr = true;
        blocker.footprint = w.nodes[0].footprint;
        w.nodes[1] = blocker;
        for i in 2..4 { let files = w.nodes[i].footprint; w.nodes[i] = ready(i); w.nodes[i].footprint = files; }
        let width = w.gates[0].w;
        stats.case(index, BIG_SEED, Some(w), vec![], || {check_gate_leaf_progress(width);});
    });
}

pub fn pool() {
    run_cases("beyond_pool_grant", |index, stats| {
        let mut n = Node::new(RepoId(0)); n.ticket = Some(10); n.priority = priorities()[index % 5];
        let mut w = large_world(n, index);
        // Keep capacity one because the original assertion states a free
        // one-place pool. Increase queue length and vary all priority levels.
        w.pool_capacity = 1;
        for i in 1..w.nodes.len() {
            let files = w.nodes[i].footprint;
            w.nodes[i] = Node::new(RepoId((i % 3) as u64)); w.nodes[i].footprint = files;
            w.nodes[i].ticket = Some(10+i as u64); w.nodes[i].priority = priorities()[(i+index) % 5];
        }
        stats.case(index, BIG_SEED, Some(w), vec![], check_pool_grant_order);
    });
}

pub fn pool_order() {
    run_cases("beyond_pool_order", |index, stats| {
        let w = large_world(Node::new(RepoId(0)), index);
        let mut rng = Prng(BIG_SEED ^ index as u64);
        let a = rng.next(); let b = a.wrapping_add(1);
        stats.case(index, BIG_SEED, Some(w), vec![Draw::U64(a), Draw::U64(b),
            Draw::Priority(priorities()[index % 5]), Draw::Priority(priorities()[index / 5 % 5])], || {
            check_pool_order_is_strict_priority();
            let w = any_world(None);
            for pair in w.nodes.windows(2) {
                let mut a = pair[0]; let mut b = pair[1];
                a.phase = Phase::Ingesting; b.phase = Phase::Ingesting;
                a.ticket = Some(1); b.ticket = Some(2);
                assert_eq!(pool_key(&a).unwrap() < pool_key(&b).unwrap(),
                    a.priority < b.priority || a.priority == b.priority);
            }
        });
    });
}
