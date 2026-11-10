// Worker B's finite domains and seeded candidate construction.
use std::time::Instant;

fn deterministic_world(phase: Phase, index: usize, seed: u64) -> World {
    let mut w = world_fixture(Some(phase), index, seed);
    w.nodes[0].repo = RepoId(0);
    w
}

pub fn dependency_registration() {
    assert_source_identity();
    let start = Instant::now(); let mut stats = Stats::default();
    enumerate(&[phases().len(), 3, 128], |index, d| {
        let mut rng = Prng(SEED ^ index as u64);
        let mut n = node_fixture(phases()[d[0]], d[2], rng.next());
        n.repo = RepoId(0); n.ticket = None;
        // The original one-node world has zero ticket/question counters.
        stats.case(index, SEED, None, vec![Draw::Node(n), Draw::Status(statuses()[d[1]])], check_dependency_registration);
    });
    stats.finish("apply_dependency_registration", SEED, start);
}

pub fn dependency_remove() {
    assert_source_identity(); let start = Instant::now(); let mut stats = Stats::default();
    enumerate(&[phases().len(), 2], |index, d| stats.case(index, SEED, None,
        vec![Draw::Phase(phases()[d[0]]), Draw::Bool(d[1] != 0)], check_dependency_remove));
    stats.finish("apply_dependency_remove", SEED, start);
}

pub fn dependency_cycles() {
    assert_source_identity(); let start = Instant::now(); let mut stats = Stats::default();
    enumerate(&[3,3,3,3,3], |index, d| stats.case(index, SEED, None,
        d.iter().map(|&x| Draw::U8(x as u8)).collect(), check_dependency_cycles));
    stats.finish("dependency_no_cycle", SEED, start);
}

pub fn dependency_waiting(delivery: bool) {
    assert_source_identity(); let start = Instant::now(); let mut stats = Stats::default();
    stats.case(0, SEED, None, vec![], || {
        if delivery { check_dependency_delivery(); } else { check_dependency_waiting(); }
    });
    stats.finish(if delivery { "dependency_delivered_releases" } else { "dependency_waiting_holds_nothing" }, SEED, start);
}

pub fn adoption(no_maker_credit: bool) {
    assert_source_identity(); let start = Instant::now(); let mut stats = Stats::default();
    enumerate(&[8,2,2,8,2,3,15], |index, d| {
        let mut rng = Prng(SEED ^ index as u64);
        let mut w = deterministic_world(Phase::Making, index, rng.next());
        let n = &mut w.nodes[0];
        n.pos = 0; n.paused = d[2] != 0; n.pool = false; n.run = None; n.ticket = None;
        n.proposed = Some(HeadId(d[0] as u64)); n.adopted = d[1] != 0;
        n.head = if n.adopted { Some(HeadId((7-d[0]) as u64)) } else { None };
        n.checked = None; n.approved = None;
        n.attempt = 1 + d[5] as u8; n.round = d[4] as u8;
        n.credit.ready = true; n.credit.start = [0,1,2,4,7,63,CREDIT_LIMIT-1,rng.next()%CREDIT_LIMIT][d[3]];
        n.credit.spent = if d[3] & 1 != 0 { n.credit.start } else { 0 };
        n.footprint = Footprint::inline(1 + d[6] as u64);
        n.effect = Some(InFlight { seq: n.next_seq-1, kind: EffectKind::PrepareWorktree, cancelling: false });
        stats.case(index, SEED, Some(w), vec![], || {
            if no_maker_credit { check_adoption_has_no_maker_credit(); } else { check_adoption_at_most_once(); }
        });
    });
    stats.finish(if no_maker_credit { "apply_adoption_has_no_maker_credit" } else { "apply_adoption_at_most_once" }, SEED, start);
}

fn gate_tape(phases: [u8;4], paused: usize, masks: [u64;4], tickets: [Option<u64>;4], first: Option<Phase>) -> Vec<Draw> {
    let mut draws = vec![];
    for i in 0..4 {
        draws.extend([Draw::U64(masks[i]), Draw::Bool(paused & (1<<i) != 0), Draw::U8(phases[i])]);
        let ready = if i == 0 { first.map_or(phases[i] == 0, |p| matches!(p, Phase::Ready(_))) } else { phases[i] == 0 };
        if ready { draws.push(Draw::Ticket(tickets[i])); }
    }
    draws
}

pub fn gate(width: u8, planning: bool) {
    assert_source_identity(); let start = Instant::now(); let mut stats = Stats::default();
    enumerate(&[256,16,15], |index, d| {
        let mut rng = Prng(SEED ^ index as u64); let mut shape = d[0];
        let mut phases = [0u8;4]; for p in &mut phases { *p = (shape%4) as u8; shape /= 4; }
        let masks = std::array::from_fn(|i| 1 + ((d[2] + i * (1 + index%14)) % 15) as u64);
        let mut values = [0,1,2,3,4,5,6,7];
        for i in (1..8).rev() { values.swap(i, (rng.next() as usize) % (i+1)); }
        let tickets = std::array::from_fn(|i| if (index / (i+1)) % 9 == 8 { None } else { Some(values[i]) });
        let draws = gate_tape(phases, d[1], masks, tickets, None);
        stats.case(index, SEED, None, draws, || {
            if planning { check_gate_planning(width); } else { check_gate_grant(width); }
        });
    });
    stats.finish(&format!("gate_{}_w{width}", if planning {"planning"} else {"grant"}), SEED, start);
}

pub fn rebase_gate() {
    assert_source_identity(); let start = Instant::now(); let mut stats = Stats::default();
    for pos in 0..2 {
        stats.case(pos as usize, SEED, None, vec![], || {
            let mut node = Node::new(RepoId(0));
            node.phase = Phase::Rebasing;
            node.run = Some(node.generation);
            node.pos = pos;
            kani::assume(node.pos < 2);
            let awaiting = node.current() == Some(Item::Await);
            kani::cover!(!awaiting, "the rebase has not reached Await");
            kani::cover!(awaiting, "the rebase is awaiting its reply");
            assert_eq!(rebase_progress_applies(&node, node.generation), awaiting);
        });
    }
    stats.finish("apply_rebase_progress_gate", SEED, start);
}

fn configure_actor(w: &mut World, d: &[usize], index: usize) {
    let n = &mut w.nodes[0];
    n.paused = d[1] != 0;
    n.pos = d[0] as u8;
    n.attempt = 1 + d[4] as u8; n.round = d[5] as u8; n.replanned = d[6] != 0;
    n.footprint = Footprint::inline(d[8] as u64);
    n.credit.ready = true;
    n.credit.start = [0,1,4,CREDIT_LIMIT-1][d[7]];
    n.credit.spent = if index & 16 != 0 { n.credit.start } else { 0 };
    n.run = None; n.job = None; n.pool = false; n.ticket = None; n.effect = None;
    match n.current() {
        Some(Item::Await) => {
            if n.phase == Phase::Checking { n.job = Some(n.generation); }
            else { n.run = Some(n.generation); n.pool = n.phase != Phase::Rebasing; }
            if d[2] != 0 {
                let kind = match n.phase { Phase::Checking => EffectKind::SubmitChecks,
                    Phase::Rebasing => EffectKind::StartRebase,
                    Phase::Planning { .. } => EffectKind::StartRun(Role::Plan),
                    Phase::Making => EffectKind::StartRun(Role::Make),
                    Phase::Reviewing => EffectKind::StartRun(Role::Review),
                    _ => EffectKind::StartRun(Role::Ingest) };
                n.effect = Some(InFlight { seq: n.next_seq-1, kind, cancelling: d[2] == 2 });
            }
        }
        Some(Item::Do(kind)) if d[2] != 0 => {
            n.effect = Some(InFlight { seq: n.next_seq-1, kind, cancelling: d[2] == 2 });
        }
        _ => {}
    }
    if n.phase == Phase::Publishing {
        n.pr_open = n.pos >= 3 || (n.pos == 2 && d[2] == 1);
        n.had_pr = n.pr_open;
    }
    n.review_stop = None;
}

pub fn publishing(earlier: bool) {
    assert_source_identity(); let start = Instant::now(); let mut stats = Stats::default();
    enumerate(&[4,2,3,1,3,2,2,4,16,if earlier {2} else {1}], |index, d| {
        let mut w = deterministic_world(Phase::Publishing, index, SEED ^ index as u64);
        configure_actor(&mut w, d, index);
        w.fail_fast = index & 8 != 0;
        let draws = if earlier { vec![Draw::Bool(d[9] != 0)] } else { vec![] };
        stats.case(index, SEED, Some(w), draws, || {
            if earlier { check_publishing_ci_earlier_step(); } else { check_publishing_ci_tests(); }
        });
    });
    stats.finish(if earlier {"apply_publishing_ci_earlier_step"} else {"apply_publishing_ci_tests"}, SEED, start);
}

pub fn fail_fast_divert(kind: usize) {
    assert_source_identity(); let start = Instant::now(); let mut stats = Stats::default();
    let phase = match kind { 0 => Phase::Planning { must_split: false }, 1..=3 => Phase::Making,
        4 => Phase::Checking, 5..=6 => Phase::Reviewing, 7 => Phase::AwaitingDelivery, 8 => Phase::Publishing, _ => unreachable!() };
    let payloads = match kind {0|1|7|8 => 2, 4 => 8, _ => 1};
    enumerate(&[agenda(phase).len(),2,3,2,3,2,2,4,16,payloads], |index, d| {
        let phase = if kind == 0 { Phase::Planning {must_split: d[9] != 0} } else {phase};
        let mut w = deterministic_world(phase, index, SEED ^ index as u64);
        configure_actor(&mut w, d, index);
        w.fail_fast = false;
        let generation = w.nodes[0].generation + d[3] as u64;
        let event = match kind {
            7|8 => Event::Fact {node: NodeId(0), fact: Fact::CiFailed {infra: d[9] != 0}},
            _ => Event::Reply {node: NodeId(0), generation, reply: match kind {
                0 => Reply::PlanCantMeet,
                1 => Reply::MakerNeedsFiles {files: Footprint::inline(d[8] as u64), fits: d[9] != 0},
                2 => Reply::MakerCantMeet, 3 => Reply::MakerOutOfRoom,
                4 => Reply::ChecksDone {report: d[9]&1 != 0, pass: d[9]&2 != 0, in_footprint: d[9]&4 != 0},
                5 => Reply::ChangesRequested, 6 => Reply::ReviewerOutOfRoom, _ => unreachable!() }},
        };
        let before = w.clone();
        stats.case(index, SEED, Some(w), vec![], || {
            check_fail_fast_diverts(phase, event);
            // Retain the general transition rules alongside the divert comparison.
            let mut on_before = before.clone(); on_before.fail_fast = true;
            let mut on_after = on_before.clone(); let mut out = Outbox::new();
            apply(&mut on_after, event, &mut out, &mut Report::new());
            require_consistent(&on_after); check_nodes(&on_before, &on_after);
            check_outputs(&on_before, &on_after, &out, None);
            check_fail_fast(&on_before, &on_after, event);
            check_fail_fast_funding(&on_before, &on_after, event);
        });
    });
    let names = ["plan_cant_meet","maker_needs_files","maker_cant_meet","maker_out_of_room","checks_done","changes_requested","reviewer_out_of_room","ci_failed_awaiting_delivery","ci_failed_publishing"];
    stats.finish(&format!("fail_fast_{}",names[kind]), SEED, start);
}

pub fn gate_leaf(width: u8) {
    assert_source_identity(); let start = Instant::now(); let mut stats = Stats::default();
    enumerate(&[15,15,15,15,2,2], |index, d| {
        let phases = [0,1,if d[4] == 0 {0} else {1},if d[5] == 0 {0} else {1}];
        let masks = std::array::from_fn(|i| 1+d[i] as u64);
        let draws = gate_tape(phases, 0, masks, [Some(1),None,Some(2),Some(3)], Some(Phase::Ready(Resume::Making)));
        stats.case(index, SEED, None, draws, || check_gate_leaf_progress(width));
    });
    stats.finish(&format!("gate_leaf_progress_w{width}"), SEED, start);
}

pub fn pool_grant() {
    assert_source_identity(); let start = Instant::now(); let mut stats = Stats::default();
    let permutations = [[0,1,2],[0,2,1],[1,0,2],[1,2,0],[2,0,1],[2,1,0]];
    enumerate(&[5,5,5,8,6,65,2], |index, d| {
        let mut rng = Prng(SEED ^ index as u64);
        let start_ticket = rng.next()%62;
        let mut draws = vec![Draw::U8(d[6] as u8), Draw::U64(d[5] as u64)];
        for i in 0..3 {
            draws.push(Draw::Ticket(if d[3]&(1<<i) == 0 {None} else {Some(start_ticket+permutations[d[4]][i])}));
            draws.push(Draw::Priority(priorities()[d[i]]));
        }
        stats.case(index, SEED, None, draws, check_pool_grant_order);
    });
    stats.finish("pool_grant_is_priority_ordered", SEED, start);
}

pub fn pool_strict() {
    assert_source_identity(); let start = Instant::now(); let mut stats = Stats::default();
    enumerate(&[5,5,128], |index, d| {
        let mut rng = Prng(SEED ^ d[2] as u64);
        let a = if d[2]<8 { [0,1,2,63,1<<40,u64::MAX-1,u64::MAX,7][d[2]] } else {rng.next()};
        let b = if d[2]%8 == 0 {a} else {rng.next()};
        stats.case(index, SEED, None, vec![Draw::U64(a), Draw::U64(b), Draw::Priority(priorities()[d[0]]),Draw::Priority(priorities()[d[1]])], check_pool_order_is_strict_priority);
    });
    stats.finish("pool_order_is_strict_priority", SEED, start);
}
