//! Worker A input domains. Assertions and counted filters are in B's common scope.
use crate::common::*;
use std::time::Instant;

pub const A_SEED: u64 = 0x4b35_1110_5eed_a001;

fn outcomes() -> [Outcome; 6] {
    [Outcome::Merged, Outcome::Stopped, Outcome::Replaced, Outcome::Duplicate,
        Outcome::AlreadyDone, Outcome::NotActionable]
}

fn resumes() -> [Resume; 5] {
    [Resume::Making, Resume::Checking, Resume::Reviewing, Resume::Publishing, Resume::Rebasing]
}

fn continuations() -> Vec<Cont> {
    let mut v = vec![Cont::Ingesting, Cont::Auditing, Cont::Unplanned,
        Cont::Planning { must_split: false }, Cont::Planning { must_split: true }, Cont::Nowhere];
    v.extend(resumes().map(Cont::Ready));
    v.extend(outcomes().map(Cont::Stop));
    v
}

fn kinds() -> [QKind; 6] {
    [QKind::AuditDisagreement, QKind::NeedsSplitting, QKind::CantMeet,
        QKind::OutOfCredit, QKind::EffectFailed, QKind::CleanupFailed]
}

fn effects() -> Vec<EffectKind> {
    let mut v = vec![EffectKind::SubmitChecks, EffectKind::StartRebase,
        EffectKind::PrepareWorktree, EffectKind::Push, EffectKind::PostApproval,
        EffectKind::OpenPr, EffectKind::Enqueue, EffectKind::StopRun,
        EffectKind::WithdrawJob, EffectKind::ClosePr, EffectKind::RemoveWorktree];
    v.extend([Role::Ingest, Role::Audit, Role::Plan, Role::Make, Role::Review].map(EffectKind::StartRun));
    v
}

/// A candidate, not an assertion of validity. Original assumes filter it.
fn anchor(phase: Phase, pos: u8) -> World {
    let mut w = world_fixture(Some(phase), 0, 1);
    w.gates = vec![Gate { w: 3 }, Gate { w: 3 }];
    w.pool_capacity = 2;
    w.next_ticket = 1024;
    w.next_question = 1024;
    let mut n = node_fixture(phase, 0, 1);
    n.repo = RepoId(0);
    n.phase = phase;
    n.pos = pos;
    n.generation = 7;
    n.next_seq = 8;
    n.last_cmd = 2;
    n.credit = Credit { ready: true, start: 4, ..Credit::default() };
    n.footprint = Footprint::inline(1);
    n.attempt = 1;
    n.head = Some(HeadId(1));
    n.checked = n.head;
    n.approved = n.head;
    n.run = None;
    n.job = None;
    n.pool = false;
    n.effect = None;
    n.ticket = None;
    n.paused = false;
    n.holds = phase.holding();
    n.worktree = phase.holding() || matches!(phase, Phase::Ready(_));
    n.had_pr = phase == Phase::AwaitingDelivery || (phase == Phase::Publishing && pos >= 3);
    n.pr_open = n.had_pr;
    if phase == Phase::AwaitingRob {
        n.question = Some(Question { id: 1, kind: QKind::CantMeet,
            cont: Cont::Planning { must_split: false }, cost: 1, clarified: false });
    }
    // Detect Await through the real agenda, without duplicating its definition.
    if at_await(&n) {
        if phase == Phase::Checking { n.job = Some(n.generation); }
        else { n.run = Some(n.generation); n.pool = phase != Phase::Rebasing; }
    }
    if let Phase::Stopping(o) | Phase::Closed(o) = phase {
        n.holds = false;
        n.worktree = false;
        n.pr_open = false;
        if o == Outcome::Merged {
            n.merged = Some(HeadId(1));
            n.had_pr = true;
            n.credit.merge_credited = phase.is_closed();
        }
    }
    w.nodes[0] = n;
    let mut inert = Node::new(RepoId(1));
    inert.phase = Phase::Closed(Outcome::Stopped);
    w.nodes[1] = inert;
    w
}

/// Complete finite one-axis sweeps; this is not a Cartesian Node enumeration.
fn candidates(phase: Option<Phase>, mut visit: impl FnMut(World)) {
    let all = phase.map_or_else(phases, |p| vec![p]);
    for p in all {
        for pos in 0..=u8::MAX { visit(anchor(p, pos)); }
        let positions = positions(p);
        let positions = if positions.is_empty() { vec![0] } else { positions };
        for pos in positions {
            let base = anchor(p, pos);
            visit(base.clone());
            // Every bool on both values and every small bounded integer on
            // all values, independently, with the same phase/agenda anchor.
            for axis in 0..19 {
                for value in [false, true] {
                    let mut w = base.clone(); let n = &mut w.nodes[0];
                    match axis {
                        0 => n.paused = value, 1 => n.holds = value, 2 => n.pool = value,
                        3 => n.replanned = value, 4 => n.review_restarted = value,
                        5 => n.adopted = value, 6 => n.pr_open = value, 7 => n.had_pr = value,
                        8 => n.worktree = value, 9 => n.waiting_incident = value,
                        10 => n.incident_needs_rob = value, 11 => n.credit.ready = value,
                        12 => n.credit.merge_credited = value, 13 => n.credit.withheld = value,
                        14 => n.planning_started = value,
                        15 => { if let Some(q) = &mut n.question { q.clarified = value; } },
                        16 => w.fail_fast = value,
                        17 => n.run = if value { Some(n.generation) } else { None },
                        _ => n.job = if value { Some(n.generation) } else { None },
                    }
                    visit(w);
                }
            }
            for axis in 0..10 {
                for value in 0..=u8::MAX {
                    let mut w = base.clone(); let n = &mut w.nodes[0];
                    match axis {
                        0 => n.attempt = value, 1 => n.round = value,
                        2 => n.plan_retries = value, 3 => n.audit_rounds = value,
                        4 => n.run_failures = value, 5 => n.run_incidents = value,
                        6 => n.fx_failures = value, 7 => n.fx_incidents = value,
                        8 => n.rechecks = value,
                        _ => { if let Some(q) = &mut n.question { q.cost = value; } },
                    }
                    visit(w);
                }
            }
            for mask in 0..16 {
                let mut w = base.clone(); w.nodes[0].footprint = Footprint::inline(mask); visit(w);
            }
            for priority in priorities() {
                let mut w = base.clone(); w.nodes[0].priority = priority; visit(w);
            }
            for early in [None, Some(EarlyEnd::Duplicate), Some(EarlyEnd::AlreadyDone), Some(EarlyEnd::NotActionable)] {
                let mut w = base.clone(); w.nodes[0].early_end = early; visit(w);
            }
            for stop in [None, Some(ReviewStop::ChecksFailed), Some(ReviewStop::ChangesRequested),
                Some(ReviewStop::CiFailed), Some(ReviewStop::MakerCantMeet), Some(ReviewStop::MakerNeedsFiles),
                Some(ReviewStop::MakerOutOfRoom), Some(ReviewStop::ReviewerOutOfRoom), Some(ReviewStop::PlanCantMeet),
                Some(ReviewStop::PublicationBody), Some(ReviewStop::AuditDisagreement)] {
                let mut w = base.clone(); w.nodes[0].review_stop = stop; visit(w);
            }
            for axis in 0..5 {
                for head in std::iter::once(None).chain((0..8).map(|v| Some(HeadId(v)))) {
                    let mut w = base.clone(); let n = &mut w.nodes[0];
                    match axis { 0 => n.head = head, 1 => n.checked = head,
                        2 => n.approved = head, 3 => n.proposed = head, _ => n.merged = head }
                    visit(w);
                }
            }
            for ticket in [None, Some(0), Some(1023)] {
                let mut w = base.clone(); w.nodes[0].ticket = ticket; visit(w);
            }
            // Exhaust the retry/funding corners together. Independent random
            // credit entries almost never produce exactly exhausted credit.
            enumerate(&[3, 2, 2, 5, 3, 2, 2, 3], |_, d| {
                let mut w = base.clone(); let n = &mut w.nodes[0];
                n.credit.start = [0, 1, 4][d[0]];
                w.fail_fast = d[1] != 0;
                n.review_restarted = d[2] != 0;
                n.rechecks = d[3] as u8;
                n.attempt = 1 + d[4] as u8;
                n.round = d[5] as u8;
                n.replanned = d[6] != 0;
                n.plan_retries = d[7] as u8;
                visit(w);
            });
            for kind in effects() {
                for cancelling in [false, true] {
                    let mut w = base.clone();
                    w.nodes[0].effect = Some(InFlight { seq: 7, kind, cancelling });
                    if kind == EffectKind::OpenPr && !cancelling {
                        w.nodes[0].pr_open = true; w.nodes[0].had_pr = true;
                    }
                    visit(w);
                }
            }
            for pool in 0..=2 {
                for a in 0..=3 {
                    for b in 0..=3 {
                        let mut w = base.clone();
                        w.pool_capacity = pool; w.gates[0].w = a; w.gates[1].w = b; visit(w);
                    }
                }
            }
            // Full pair of phase shapes, with both repository choices. Other
            // axes are independent sweeps or samples, not a hidden full product.
            for other in phases() {
                for repo in 0..2 {
                    let mut w = base.clone();
                    w.nodes[1] = anchor(other, 0).nodes[0];
                    w.nodes[1].repo = RepoId(repo);
                    visit(w);
                }
            }
        }
        if p == Phase::AwaitingRob || matches!(p, Phase::Stopping(_)) {
            for kind in kinds() {
                for cont in continuations() {
                    for clarified in [false, true] {
                        let mut w = anchor(p, 0);
                        w.nodes[0].question = Some(Question { id: 1, kind, cont,
                            cost: u8::from(matches!(kind, QKind::CantMeet | QKind::OutOfCredit)), clarified });
                        visit(w);
                    }
                }
            }
        }
    }
    let mut rng = Prng(A_SEED);
    for i in 0..4096 {
        let mut w = world_fixture(phase, i, rng.next());
        // Sample the issued-id spaces as well as individual node counters.
        // Translate related ids together so samples reach live operations and
        // questions instead of almost always failing equality requirements.
        let ticket_shift = rng.next() % ((1 << 40) - 1024);
        let question_shift = rng.next() % ((1 << 40) - 1024);
        w.next_ticket += ticket_shift;
        w.next_question += question_shift;
        for n in &mut w.nodes {
            let generation = rng.next() % ((1 << 40) - 1024);
            if n.run == Some(n.generation) { n.run = Some(generation); }
            if n.job == Some(n.generation) { n.job = Some(generation); }
            n.generation = generation;
            let seq_shift = rng.next() % ((1 << 40) - 1024);
            n.next_seq = seq_shift + 8;
            if let Some(e) = &mut n.effect { e.seq = seq_shift + 7; }
            n.last_cmd = rng.next() % ((1 << 40) - 1024);
            if let Some(t) = &mut n.ticket { *t += ticket_shift; }
            if let Some(q) = &mut n.question { q.id += question_shift; }
        }
        let n = &mut w.nodes[0];
        // Correlated boundary and interior wide samples, not u64 truncation.
        n.credit.earned = rng.next() % (1 << 30);
        n.credit.added = rng.next() % (1 << 30);
        n.credit.spent = rng.next() % (1 << 30);
        if i % 17 == 0 { n.generation = 1 << 40; }
        if i % 19 == 0 { w.next_ticket = 1 << 40; }
        if i % 23 == 0 { n.credit.start = 1 << 30; }
        visit(w);
    }
}

fn event(h: usize, w: &World, payload: usize, mode: usize) -> Event {
    let n = w.nodes[0];
    let wide = A_SEED ^ n.generation.rotate_left(17) ^ n.next_seq.rotate_left(31) ^ payload as u64;
    let generation = if mode == 0 { n.generation } else if mode == 1 { n.generation + 1 } else { wide };
    let seq = if mode == 2 { wide } else { n.effect.map_or(n.next_seq, |e| e.seq) + u64::from(mode != 0) };
    let id = if mode == 2 { wide } else { n.last_cmd + u64::from(mode == 0) };
    let question = if mode == 2 { wide } else { n.question.map_or(0, |q| q.id) + u64::from(mode != 0) };
    let head = HeadId(if mode == 2 { wide } else { (payload % 8) as u64 });
    let reply = match h {
        7 => Some(Reply::IngestDone { size: payload as u8 }),
        8 => Some(Reply::IngestEarlyEnd([EarlyEnd::Duplicate, EarlyEnd::AlreadyDone, EarlyEnd::NotActionable][payload % 3])),
        9 => Some(Reply::ProtectedPaths), 10 => Some(Reply::AuditAgrees),
        11 => Some(Reply::PlanLeaf { footprint: Footprint::inline((payload % 16) as u64), fits: payload / 16 != 0 }),
        12 => Some(Reply::PlanSplit), 13 => Some(Reply::PlanCantMeet),
        14 => Some(Reply::MakerDone { head }),
        15 => Some(Reply::MakerNeedsFiles { files: Footprint::inline((payload % 16) as u64), fits: payload / 16 != 0 }),
        16 => Some(Reply::MakerCantMeet), 17 => Some(Reply::MakerOutOfRoom),
        18 => Some(Reply::ChecksDone { report: payload & 1 != 0, pass: payload & 2 != 0, in_footprint: payload & 4 != 0 }),
        19 => Some(Reply::Approved { head }),
        20 => Some(Reply::ReviewerOutOfRoom), 21 => Some(Reply::ChangesRequested),
        22 => Some(Reply::RebaseClean { head }),
        23 => Some(Reply::RebaseNeedsChanges), 24 => Some(Reply::RunFailed), _ => None,
    };
    if let Some(reply) = reply { return Event::Reply { node: NodeId(0), generation, reply }; }
    if (25..=29).contains(&h) {
        return Event::Ack { node: NodeId(0), seq,
            ack: [Ack::Confirmed, Ack::DeliveryAccepted, Ack::Retryable, Ack::Permanent, Ack::BodyPolicyRefused][h - 25] };
    }
    match h {
        6 | 56 => {
            let repo = RepoId(if mode == 2 { wide } else { [0, 1, 2, u64::MAX][payload / 5 % 4] });
            let priority = priorities()[payload % 5];
            if h == 6 { Event::NewRequest { repo, priority } }
            else { Event::NewAdoption { repo, head: HeadId(7), priority } }
        }
        30 => Event::Fact { node: NodeId(0), fact: Fact::PushByOther { head } },
        31 => Event::Fact { node: NodeId(0), fact: Fact::CiFailed { infra: payload != 0 } },
        32..=35 => Event::Command { node: NodeId(0), id,
            command: [Command::Stop, Command::Pause, Command::Resume, Command::ChangeRequest][h - 32] },
        36..=43 => Event::Command { node: NodeId(0), id, command: Command::Answer { question,
            answer: match h {
                36 => Answer::Continue, 37 => Answer::TryAgain, 38 => Answer::AddCredit(payload as u8),
                39 => Answer::ChangeRequest, 40 => Answer::Stop, 41 => Answer::Unclear,
                42 => Answer::Retry, _ => Answer::MarkHandled,
            } } },
        _ => unreachable!("event harness {h}"),
    }
}

fn payload_count(h: usize) -> usize {
    match h { 6 | 56 => 20, 7 | 38 => 256, 8 => 3, 11 | 15 => 32,
        14 | 18 | 19 | 22 | 30 => 8, 31 => 2, _ => 1 }
}

/// Retain the event-wrapper assumes even though payloads are supplied directly.
fn event_filter(stats: &mut Stats, e: Event) -> bool {
    let (allowed, site) = match e {
        Event::Reply { reply: Reply::MakerDone { head } | Reply::Approved { head } | Reply::RebaseClean { head }, .. } =>
            (head.0 < 8, "proofs.rs:582 head.0 < 8"),
        Event::Fact { fact: Fact::PushByOther { head }, .. } =>
            (head.0 < 8, "proofs.rs:807 head.0 < 8"),
        _ => (true, ""),
    };
    if !allowed {
        stats.attempted += 1;
        stats.skipped += 1;
        *stats.filters.entry(site.into()).or_default() += 1;
    }
    allowed
}

fn body_policy_refused(before: &World, event: Event) {
    let Event::Ack { seq, .. } = event else { unreachable!() };
    let b = &before.nodes[0];
    let open_pr = b.effect.is_some_and(|e| e.seq == seq && !e.cancelling && e.kind == EffectKind::OpenPr);
    let mut refused = before.clone();
    let mut refused_out = Outbox::new();
    apply_event(&mut refused, event, &mut refused_out);
    if open_pr {
        assert!(refused_out.is_empty(), "publication refusal never asks Rob or raises an incident");
    } else {
        let mut permanent = before.clone();
        let mut permanent_out = Outbox::new();
        apply_event(&mut permanent, Event::Ack { node: NodeId(0), seq, ack: Ack::Permanent }, &mut permanent_out);
        assert_eq!(refused, permanent);
        assert_eq!(refused_out, permanent_out);
    }
}

fn extra_covers(stats: &mut Stats, h: usize, w: &World, event: Event) {
    let b = w.nodes[0];
    let accepted = match event {
        Event::Reply { generation, .. } => at_await(&b) && b.run == Some(generation),
        _ => false,
    };
    let funded = b.credit.balance() >= 1;
    let free = b.rechecks.saturating_add(1) <= FREE_RECHECKS;
    let mut hit = |condition, label| { if condition { stats.covers.insert(label); } };
    match h {
        20 if accepted && b.phase == Phase::Reviewing => {
            hit(!b.review_restarted && free, "first exhaustion: free review");
            hit(!b.review_restarted && !free && funded, "first exhaustion: paid review");
            hit(!b.review_restarted && !free && !funded, "first exhaustion: needs credit");
            hit(b.review_restarted && funded, "second exhaustion: paid split");
            hit(b.review_restarted && !funded, "second exhaustion: split needs credit");
        }
        22 if accepted && b.phase == Phase::Rebasing => {
            hit(free, "clean rebase uses the free allowance");
            hit(!free && funded, "clean rebase spends credit");
            hit(!free && !funded, "clean rebase needs credit");
        }
        23 if accepted && b.phase == Phase::Rebasing => {
            hit(free && funded, "conflict fix pays despite free allowance");
            hit(!free && funded, "conflict fix spends credit after allowance");
            hit(!funded, "conflict fix needs credit");
        }
        29 => {
            let Event::Ack { seq, .. } = event else { unreachable!() };
            let open_pr = b.effect.is_some_and(|e| e.seq == seq && !e.cancelling && e.kind == EffectKind::OpenPr);
            hit(open_pr && w.fail_fast, "OpenPr refusal with fail_fast");
            hit(open_pr && !w.fail_fast, "OpenPr refusal without fail_fast");
            hit(b.effect.is_some_and(|e| e.seq != seq), "a stale refusal is ignored");
            hit(b.effect.is_some_and(|e| e.seq == seq && e.cancelling), "a cancelled effect only clears its delivery");
            hit(!open_pr && b.effect.is_some_and(|e| e.seq == seq && !e.cancelling), "other effect kinds retain permanent failure handling");
        }
        56 => {
            let Event::NewAdoption { repo, .. } = event else { unreachable!() };
            hit(repo.0 < w.gates.len() as u64, "new adoption retains the requested head");
        }
        _ => {},
    }
}

pub fn run_event(h: usize, name: &str) {
    assert_source_identity();
    let start = Instant::now();
    let mut stats = Stats::default();
    let mut index = 0;
    candidates(None, |w| {
        let payload = index % payload_count(h);
        for mode in 0..3 {
            let e = event(h, &w, payload, mode);
            if event_filter(&mut stats, e) {
                let passed = stats.passed;
                stats.case(index, A_SEED, Some(w.clone()), vec![], || {
                    let before = assert_apply(e);
                    if h == 29 { body_policy_refused(&before, e); }
                });
                if stats.passed > passed { extra_covers(&mut stats, h, &w, e); }
            }
            index += 1;
        }
    });
    // Each payload product is exhausted against every valid agenda anchor,
    // both flag values and matching/stale ids. Other axes above are independent.
    for phase in phases() {
        for pos in positions(phase).into_iter().chain([0]) {
            let mut w = anchor(phase, pos);
            // A live delivery makes Do entries settled event inputs.
            for kind in effects() {
                if at_effect(&w.nodes[0], kind) {
                    w.nodes[0].effect = Some(InFlight { seq: 7, kind, cancelling: false });
                    if kind == EffectKind::OpenPr { w.nodes[0].pr_open = true; w.nodes[0].had_pr = true; }
                    break;
                }
            }
            for fast in [false, true] {
                w.fail_fast = fast;
                for payload in 0..payload_count(h) {
                    for mode in 0..3 {
                        let e = event(h, &w, payload, mode);
                        if event_filter(&mut stats, e) {
                            let passed = stats.passed;
                            stats.case(index, A_SEED, Some(w.clone()), vec![], || {
                                let before = assert_apply(e);
                                if h == 29 { body_policy_refused(&before, e); }
                            });
                            if stats.passed > passed { extra_covers(&mut stats, h, &w, e); }
                        }
                        index += 1;
                    }
                }
            }
        }
    }
    assert!(stats.covers.contains("the event changes the world"), "no changing event witness: {name}");
    stats.finish(name, A_SEED, start);
}

pub fn run_advance(h: usize, name: &str) {
    assert_source_identity();
    let start = Instant::now();
    let mut stats = Stats::default();
    let targets = match h {
        44 => vec![Phase::Ingesting], 45 => vec![Phase::Auditing], 46 => vec![Phase::Unplanned],
        47 => vec![Phase::Planning { must_split: false }, Phase::Planning { must_split: true }],
        48 => resumes().into_iter().map(Phase::Ready).collect(),
        49 => vec![Phase::Making], 50 => vec![Phase::Checking], 51 => vec![Phase::Reviewing],
        52 => vec![Phase::Publishing], 53 => vec![Phase::AwaitingDelivery],
        54 => vec![Phase::Rebasing], 55 => outcomes().into_iter().map(Phase::Stopping).collect(),
        _ => unreachable!("advance harness {h}"),
    };
    let mut index = 0;
    for phase in targets {
        candidates(Some(phase), |w| {
            stats.case(index, A_SEED, Some(w), vec![], || assert_advance(phase, h != 53));
            index += 1;
        });
    }
    assert!(stats.covers.contains("the phase moves under settle, or waits"), "missing original advance cover: {name}");
    stats.finish(name, A_SEED, start);
}

const BEYOND_EVENT_SEED: u64 = A_SEED ^ 0xbe70_0001;
const BEYOND_ADVANCE_SEED: u64 = A_SEED ^ 0xbe70_0002;

fn beyond_world(index: usize, rng: &mut Prng) -> World {
    let phase = phases()[(rng.next() as usize) % phases().len()];
    let positions = positions(phase);
    let pos = if positions.is_empty() { 0 } else { positions[(rng.next() as usize) % positions.len()] };
    let mut w = anchor(phase, pos);
    w.gates.push(Gate { w: 3 });
    let count = if index % 4 == 0 { 72 } else { 6 };
    while w.nodes.len() < count {
        let mut n = Node::new(RepoId(2));
        n.phase = Phase::Closed(Outcome::Stopped);
        w.nodes.push(n);
    }
    let offset = rng.next() % 256;
    let a = w.intern_footprint((offset..offset + 80).map(FileId));
    let b = w.intern_footprint((offset + 48..offset + 128).map(FileId));
    for (i, n) in w.nodes.iter_mut().enumerate() {
        n.footprint = if i % 2 == 0 { a } else { b };
    }
    // All four dependency classes occur in every world. The linked Pending
    // edge 2 -> 3 is acyclic. Nodes above64 enter the heap graph traversal.
    for i in [2, 3] {
        let n = &mut w.nodes[i];
        n.phase = Phase::Unplanned;
        n.credit = Credit { ready: true, start: 4, withheld: true, ..Credit::default() };
        n.waiting_prerequisite = Some(100 + i as u64);
    }
    for (owner, target, status, rejected) in [
        (2, 3, DependencyStatus::Pending, false),
        (3, 4, DependencyStatus::Abandoned, false),
        (4, 5, DependencyStatus::Pending, true),
        (5, 2, DependencyStatus::Delivered, false),
    ] {
        push_dependency(&mut w, Dependency { node: NodeId(owner),
            target: DependencyTarget { kind: PrerequisiteKind::Issue, number: 100 + owner, request: Some(NodeId(target)) },
            status, rejected });
    }
    let (status, rejected) = [(DependencyStatus::Pending, false), (DependencyStatus::Abandoned, false),
        (DependencyStatus::Pending, true), (DependencyStatus::Delivered, false)][(rng.next() % 4) as usize];
    let blocked = rejected || status != DependencyStatus::Delivered;
    let n = &mut w.nodes[0];
    n.planning_started = false;
    if blocked && matches!(phase, Phase::Ingesting | Phase::Auditing | Phase::Unplanned) {
        n.credit.withheld = true;
        if phase == Phase::Ingesting && index % 2 == 0 {
            // Ingest completion must newly withhold the funded ledger.
            n.credit = Credit::default();
        }
    }
    if phase == Phase::AwaitingRob {
        n.question = Some(Question { id: 1, kind: QKind::EffectFailed,
            cont: Cont::Ingesting, cost: 0, clarified: index & 1 != 0 });
    }
    if matches!(phase, Phase::Stopping(_)) && index % 2 == 0 {
        n.question = Some(Question { id: 1, kind: QKind::CleanupFailed,
            cont: Cont::Nowhere, cost: 0, clarified: false });
    }
    if matches!(phase, Phase::Unplanned) && blocked {
        n.waiting_prerequisite = Some(9001);
    }
    w.fail_fast = rng.next() & 1 != 0;
    for kind in effects() {
        if at_effect(&w.nodes[0], kind) {
            w.nodes[0].effect = Some(InFlight { seq: 7, kind, cancelling: index % 7 == 0 });
            if kind == EffectKind::OpenPr && index % 7 != 0 {
                w.nodes[0].pr_open = true; w.nodes[0].had_pr = true;
            }
            break;
        }
    }
    push_dependency(&mut w, Dependency { node: NodeId(0),
        target: DependencyTarget { kind: PrerequisiteKind::Issue, number: 9001, request: None }, status, rejected });
    w
}

pub fn beyond_bounds_event() {
    assert_source_identity();
    let seed = BEYOND_EVENT_SEED;
    let start = Instant::now();
    let mut rng = Prng(seed);
    let mut stats = Stats::default();
    let mut case = 0;
    for sample in 0..512 {
        let w = beyond_world(sample, &mut rng);
        for h in (6..=43).chain([56]) {
            for mode in 0..2 {
                let mut e = event(h, &w, sample % payload_count(h), mode);
                // Interned payloads reach union_footprints, including overlap.
                if let Event::Reply { reply, .. } = &mut e {
                    match reply {
                        Reply::PlanLeaf { footprint, .. } => *footprint = w.nodes[5].footprint,
                        Reply::MakerNeedsFiles { files, .. } => *files = w.nodes[5].footprint,
                        _ => {},
                    }
                }
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    stats.case(case, seed, Some(w.clone()), vec![], || {
                        let before = assert_apply(e);
                        if h == 29 { body_policy_refused(&before, e); }
                    });
                }));
                if let Err(payload) = result {
                    eprintln!("BEYOND_FAILURE family=event sample={sample} case={case} seed={seed:#x} H={h} step={e:?}");
                    std::panic::resume_unwind(payload);
                }
                case += 1;
            }
        }
    }
    stats.finish("beyond_bounds_event", seed, start);
    println!("BEYOND_SCOPE family=event worlds=512 nodes=6,72 repos=3 file_sets=2 files_per_set=80 dependencies=5 statuses=Pending,Abandoned,rejected,Delivered");
}

pub fn beyond_bounds_advance() {
    assert_source_identity();
    let seed = BEYOND_ADVANCE_SEED;
    let start = Instant::now();
    let mut rng = Prng(seed);
    let mut stats = Stats::default();
    for sample in 0..512 {
        let w = beyond_world(sample, &mut rng);
        let phase = w.nodes[0].phase;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            stats.case(sample, seed, Some(w.clone()), vec![], || assert_advance(phase, phase != Phase::AwaitingDelivery));
        }));
        if let Err(payload) = result {
            eprintln!("BEYOND_FAILURE family=advance sample={sample} seed={seed:#x} node=0 phase={phase:?}");
            std::panic::resume_unwind(payload);
        }
    }
    stats.finish("beyond_bounds_advance", seed, start);
    println!("BEYOND_SCOPE family=advance worlds=512 nodes=6,72 repos=3 file_sets=2 files_per_set=80 dependencies=5 statuses=Pending,Abandoned,rejected,Delivered");
}
