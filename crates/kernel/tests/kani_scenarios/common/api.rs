pub use sampling::{Draw, Prng, SEED, Stats, enumerate};

pub fn phases() -> Vec<Phase> {
    let mut result = vec![Phase::Ingesting, Phase::Auditing, Phase::Unplanned,
        Phase::Planning { must_split: false }, Phase::Planning { must_split: true },
        Phase::Making, Phase::Checking, Phase::Reviewing, Phase::Publishing,
        Phase::AwaitingDelivery, Phase::Rebasing, Phase::AwaitingRob];
    for r in [Resume::Making, Resume::Checking, Resume::Reviewing, Resume::Publishing, Resume::Rebasing] {
        result.push(Phase::Ready(r));
    }
    for o in [Outcome::Merged, Outcome::Stopped, Outcome::Replaced, Outcome::Duplicate, Outcome::AlreadyDone, Outcome::NotActionable] {
        result.extend([Phase::Stopping(o), Phase::Closed(o)]);
    }
    result
}
pub fn priorities() -> [Priority; 5] { [Priority::P0, Priority::P1, Priority::P2, Priority::P3, Priority::Unranked] }
pub fn statuses() -> [DependencyStatus; 3] { [DependencyStatus::Pending, DependencyStatus::Abandoned, DependencyStatus::Delivered] }

pub fn bounded_counter(index: usize, random: u64) -> u64 {
    match index % 8 { 0 => 0, 1 => 1, 2 => 2, 3 => 7, 4 => 63,
        5 => (1 << 40) - 1024, _ => random % (1 << 40) }
}

/// Generate candidate shapes, then retain the original consistency/settled filters.
/// Finite axes are visited by the callers; large counters use SplitMix64.
pub fn node_fixture(phase: Phase, index: usize, random: u64) -> Node {
    let mut n = Node::new(RepoId((index % 2) as u64));
    n.phase = phase;
    n.generation = bounded_counter(index, random);
    n.next_seq = bounded_counter(index / 8 + 1, random.rotate_left(9)) + 1;
    n.last_cmd = bounded_counter(index / 64, random.rotate_left(17));
    n.credit.ready = phase != Phase::Ingesting || index & 1 != 0;
    if n.credit.ready {
        n.credit.start = match index % 6 { 0 => 0, 1 => 1, 2 => 4, 3 => 7,
            4 => CREDIT_LIMIT - 1, _ => random % CREDIT_LIMIT };
        n.credit.spent = if index & 4 != 0 { n.credit.start } else { 0 };
    }
    n.footprint = Footprint::inline((index as u64 / 3) % 16);
    n.attempt = 1 + (index % 3) as u8;
    n.round = ((index / 3) % 2) as u8;
    n.replanned = index & 8 != 0;
    n.plan_retries = (index % 2) as u8;
    n.rechecks = (index % 5) as u8;
    n.review_restarted = index & 16 != 0;
    n.audit_rounds = (index % 2) as u8;
    n.run_failures = (index % 2) as u8;
    n.run_incidents = (index % 3) as u8;
    n.fx_failures = ((index / 2) % 2) as u8;
    n.fx_incidents = ((index / 3) % 3) as u8;
    n.priority = priorities()[index % 5];
    n.paused = index & 1 != 0;
    n.planning_started = index & 32 != 0;
    n.proposed = if index & 2 != 0 { Some(HeadId((index % 8) as u64)) } else { None };
    n.adopted = n.proposed.is_some() && index & 4 != 0;
    let ready = phase.holding() || matches!(phase, Phase::Ready(_) | Phase::AwaitingDelivery);
    if ready {
        n.head = Some(HeadId((index % 8) as u64));
        n.checked = n.head; n.approved = n.head;
        n.worktree = true;
    }
    n.holds = phase.holding();
    let length = agenda(phase).len();
    n.pos = if length == 0 { 0 } else { ((index / 2) % length) as u8 };
    if phase == Phase::AwaitingDelivery || (phase == Phase::Publishing && n.pos >= 3) {
        n.pr_open = true; n.had_pr = true;
    }
    if phase == Phase::AwaitingRob {
        n.question = Some(Question { id: 0, kind: QKind::CantMeet,
            cont: Cont::Planning { must_split: index & 2 != 0 }, cost: 1, clarified: index & 4 != 0 });
        n.credit.ready = true;
    }
    match n.current() {
        Some(Item::Await) => {
            if phase == Phase::Checking { n.job = Some(n.generation); }
            else { n.run = Some(n.generation); n.pool = phase != Phase::Rebasing && index & 2 != 0; }
        }
        Some(Item::Do(kind) | Item::Teardown(kind)) if index & 2 != 0 => {
            n.effect = Some(InFlight { seq: n.next_seq - 1, kind, cancelling: index & 4 != 0 });
            if kind == EffectKind::OpenPr && !n.effect.unwrap().cancelling {
                n.pr_open = true; n.had_pr = true;
            }
        }
        Some(Item::Slot | Item::Start(_)) if index & 2 != 0 => n.ticket = Some(index as u64 % 64),
        _ => {}
    }
    if let Phase::Stopping(o) | Phase::Closed(o) = phase {
        n.holds = false; n.worktree = false; n.run = None; n.job = None; n.pool = false;
        n.pr_open = false;
        if o == Outcome::Merged {
            n.merged = Some(HeadId(1)); n.had_pr = true; n.credit.ready = true;
            if phase.is_closed() { n.credit.merge_credited = true; }
        }
        if phase.is_closed() { n.effect = None; }
    }
    n
}

pub fn world_fixture(phase: Option<Phase>, index: usize, random: u64) -> World {
    let mut w = World::new(&[1 + (index % 3) as u8, 1 + ((index / 3) % 3) as u8], 1 + (index % 2) as u8);
    w.fail_fast = index & 64 != 0;
    w.next_ticket = 128;
    w.next_question = 128;
    w.nodes.push(node_fixture(phase.unwrap_or_else(|| phases()[index % phases().len()]), index, random));
    let mut other = node_fixture(phases()[(index / phases().len()) % phases().len()], index + 1, random.rotate_left(32));
    // Enumerate an inert companion as well as another active node.
    if index % 2 == 0 { other = Node::new(RepoId(1)); other.phase = Phase::Closed(Outcome::Stopped); }
    w.nodes.push(other);
    w
}

fn any_world(phase: Option<Phase>) -> World {
    let world = sampling::world(phase);
    kani::assume(world.pool_capacity <= 2);
    kani::assume(world.gates[0].w <= 3 && world.gates[1].w <= 3);
    kani::assume(world.next_ticket < LIMIT && world.next_question < LIMIT);
    kani::assume(small(&world.nodes[0]) && small(&world.nodes[1]));
    if world.dependencies.is_empty() {
        kani::assume(!world.nodes[0].credit.withheld && !world.nodes[1].credit.withheld);
    }
    kani::assume(world.consistent());
    world
}
fn any_settled_world(phase: Option<Phase>) -> World {
    let world = any_world(phase);
    kani::assume(world.is_settled());
    world
}

pub fn assert_source_identity() {
    assert_eq!(include_str!("../../../src/lib.rs"), include_str!("kernel_original.txt"),
        "kernel lib.rs changed: refresh the test scope with common/sync.py");
}
pub fn assert_apply(event: Event) -> World { check_apply(event) }
pub fn assert_advance(phase: Phase, moves: bool) { check_advance(phase, moves); }
pub fn assert_consistent(world: &World) { require_consistent(world); }
pub fn assert_nodes(before: &World, after: &World) { check_nodes(before, after); }
pub fn assert_outputs(before: &World, after: &World, out: &Outbox, exempt: Option<usize>) { check_outputs(before, after, out, exempt); }
pub fn assert_fail_fast(before: &World, after: &World, event: Event) { check_fail_fast(before, after, event); check_fail_fast_funding(before, after, event); }
pub fn apply_event(w: &mut World, e: Event, out: &mut Outbox) { apply(w, e, out, &mut Report::new()); }
pub fn advance_node_for_test(w: &mut World, i: usize, out: &mut Outbox) -> bool { advance(w, i, out, &mut Report::new()) }
pub fn is_consistent(w: &World) -> bool { w.consistent() }
pub fn is_settled(w: &World) -> bool { w.is_settled() }
pub fn node_rank(n: &Node) -> u64 { n.settle_rank() }
pub fn push_dependency(w: &mut World, row: Dependency) { w.dependencies.push(row); }
pub fn at_await(n: &Node) -> bool { n.current() == Some(Item::Await) }
pub fn at_effect(n: &Node, kind: EffectKind) -> bool { n.current() == Some(Item::Do(kind)) }
pub fn positions(phase: Phase) -> Vec<u8> { (0..agenda(phase).len()).map(|p| p as u8).collect() }
