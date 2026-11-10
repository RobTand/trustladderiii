//! The same assertions on enumerated and sampled inputs.
use crate::common::*;
use std::time::Instant;

pub const A_SEED: u64 = 0x4b35_1110_5eed_a001;
const LIMIT: u64 = 1 << 40;
const CREDIT_LIMIT: u64 = 1 << 30;

fn counter(rng: &mut Prng, index: usize) -> u64 {
    [0, 1, 7, LIMIT - 1024, rng.next() % (LIMIT - 1024)][index % 5]
}

fn node(phase: Phase, pos: u8, bits: usize, rng: &mut Prng) -> Node {
    let mut n = Node::new(RepoId(((bits >> 4) & 1) as u64));
    n.phase = phase;
    n.pos = pos;
    n.generation = counter(rng, bits);
    n.next_seq = counter(rng, bits / 2) + 1;
    n.last_cmd = counter(rng, bits / 3);
    n.paused = bits & 1 != 0;
    n.credit.ready = phase != Phase::Ingesting || bits & 2 != 0;
    if n.credit.ready {
        n.credit.start = [0, 1, 4, CREDIT_LIMIT - 1][(bits >> 1) % 4];
        n.credit.spent = if bits & 4 != 0 { n.credit.start } else { 0 };
    }
    n.footprint = Footprint::inline(1);
    n.attempt = 1 + (bits % 3) as u8;
    n.round = ((bits >> 1) & 1) as u8;
    n.replanned = bits & 4 != 0;
    n.plan_retries = (bits % 3) as u8;
    n.rechecks = (bits % 5) as u8;
    n.review_restarted = bits & 8 != 0;
    n.audit_rounds = (bits & 1) as u8;
    n.run_failures = (bits & 1) as u8;
    n.run_incidents = (bits % 3) as u8;
    n.fx_failures = ((bits >> 1) & 1) as u8;
    n.fx_incidents = ((bits / 3) % 3) as u8;
    n.priority = priorities()[bits % 5];
    n.planning_started = bits & 2 != 0;
    n.head = Some(HeadId((bits % 8) as u64));
    n.checked = n.head;
    n.approved = n.head;
    n.proposed = if bits & 2 != 0 { n.head } else { None };
    n.adopted = n.proposed.is_some() && bits & 4 != 0;
    n.holds = phase.holding();
    n.worktree = phase.holding() || matches!(phase, Phase::Ready(_) | Phase::AwaitingDelivery);
    if phase == Phase::AwaitingDelivery || (phase == Phase::Publishing && pos >= 3) {
        n.pr_open = true;
        n.had_pr = true;
    }
    if phase == Phase::AwaitingRob {
        n.question = Some(Question { id: 1, kind: QKind::CantMeet,
            cont: Cont::Planning { must_split: false }, cost: 1, clarified: bits & 4 != 0 });
        n.credit.ready = true;
    }
    if at_await(&n) {
        if phase == Phase::Checking { n.job = Some(n.generation); }
        else { n.run = Some(n.generation); n.pool = phase != Phase::Rebasing && bits & 2 != 0; }
    }
    let effect = match phase {
        Phase::Making if pos == 0 => Some(EffectKind::PrepareWorktree),
        Phase::Publishing => [EffectKind::Push, EffectKind::PostApproval, EffectKind::OpenPr, EffectKind::Enqueue].get(pos as usize).copied(),
        Phase::Stopping(_) => [Some(EffectKind::StopRun), Some(EffectKind::WithdrawJob), Some(EffectKind::ClosePr), None, Some(EffectKind::RemoveWorktree)].get(pos as usize).copied().flatten(),
        _ => None,
    };
    if let Some(kind) = effect {
        if bits & 8 != 0 {
            n.effect = Some(InFlight { seq: n.next_seq - 1, kind, cancelling: bits & 4 != 0 });
            if kind == EffectKind::OpenPr && bits & 4 == 0 { n.pr_open = true; n.had_pr = true; }
        }
    }
    if let Phase::Stopping(o) | Phase::Closed(o) = phase {
        n.holds = false;
        n.worktree = false;
        n.pr_open = false;
        if o == Outcome::Merged {
            n.merged = Some(HeadId(1)); n.had_pr = true; n.credit.ready = true;
            if matches!(phase, Phase::Closed(_)) { n.credit.merge_credited = true; }
        }
        if matches!(phase, Phase::Closed(_)) { n.effect = None; }
    }
    n
}

fn world(n: Node, index: usize) -> World {
    let mut w = World::new(&[1 + (index % 3) as u8, 1 + ((index / 3) % 3) as u8], 1 + (index % 2) as u8);
    w.fail_fast = index & 16 != 0;
    w.next_ticket = LIMIT - 1;
    w.next_question = LIMIT - 1;
    let mut other = Node::new(RepoId(1));
    other.phase = Phase::Closed(Outcome::Stopped);
    w.nodes = vec![n, other];
    w
}

fn all_conts() -> Vec<Cont> {
    let mut c = vec![Cont::Ingesting, Cont::Auditing, Cont::Unplanned,
        Cont::Planning { must_split: false }, Cont::Planning { must_split: true }, Cont::Nowhere];
    for r in [Resume::Making, Resume::Checking, Resume::Reviewing, Resume::Publishing, Resume::Rebasing] { c.push(Cont::Ready(r)); }
    for o in [Outcome::Merged, Outcome::Stopped, Outcome::Replaced, Outcome::Duplicate, Outcome::AlreadyDone, Outcome::NotActionable] { c.push(Cont::Stop(o)); }
    c
}

fn corpus(fixed: Option<Phase>, focused: Phase) -> Vec<World> {
    let mut rng = Prng(A_SEED);
    let mut worlds = Vec::new();
    let ps = fixed.map_or_else(phases, |p| vec![p]);
    for phase in ps {
        let positions = positions(phase);
        let end = positions.len() as u8;
        for pos in 0..=end {
            for bits in 0..32 {
                let n = node(phase, pos, bits, &mut rng);
                worlds.push(world(n, bits));
            }
        }
    }
    // Full finite axes around a fixed shape; interactions outside the core are sampled.
    let pos = positions(focused).last().copied().unwrap_or(0);
    for value in 0..=u8::MAX {
        for field in 0..10 {
            let mut n = node(focused, pos, 10, &mut rng);
            match field {
                0 => n.attempt = value, 1 => n.round = value, 2 => n.plan_retries = value,
                3 => n.rechecks = value, 4 => n.audit_rounds = value, 5 => n.run_failures = value,
                6 => n.run_incidents = value, 7 => n.fx_failures = value,
                8 => n.fx_incidents = value, _ => n.pos = value,
            }
            worlds.push(world(n, value as usize));
        }
    }
    for mask in 0..16 {
        let mut n = node(focused, pos, mask, &mut rng);
        n.footprint = Footprint::inline(mask as u64);
        worlds.push(world(n, mask));
    }
    for h in 0..9 {
        for field in 0..5 {
            let mut n = node(focused, pos, 10, &mut rng);
            let head = if h == 8 { None } else { Some(HeadId(h)) };
            match field { 0 => n.head = head, 1 => n.checked = head, 2 => n.approved = head,
                3 => { n.proposed = head; n.adopted = false; }, _ => n.merged = head }
            worlds.push(world(n, h as usize));
        }
    }
    if fixed.is_none() {
        for kind in [QKind::AuditDisagreement, QKind::NeedsSplitting, QKind::CantMeet, QKind::OutOfCredit, QKind::EffectFailed, QKind::CleanupFailed] {
            for cont in all_conts() {
                for bits in 0..4 {
                    let mut n = node(Phase::AwaitingRob, 0, 10 + bits, &mut rng);
                    n.question = Some(Question { id: 1, kind, cont,
                        cost: u8::from(matches!(kind, QKind::CantMeet | QKind::OutOfCredit)), clarified: bits & 1 != 0 });
                    worlds.push(world(n, bits));
                }
            }
        }
        // Every companion phase is considered with every acting phase.
        for p in phases() {
            for q in phases() {
                let n = node(p, positions(p).last().copied().unwrap_or(0), 10, &mut rng);
                let mut w = world(n, 10);
                let mut other = node(q, positions(q).last().copied().unwrap_or(0), 11, &mut rng);
                other.repo = RepoId(1); other.footprint = Footprint::inline(2);
                w.nodes[1] = other;
                worlds.push(w);
            }
        }
    }
    worlds
}

fn focused(kind: &str) -> Phase {
    match kind {
        "ingest_done" | "ingest_early_end" | "protected_paths" => Phase::Ingesting,
        "audit_agrees" => Phase::Auditing,
        "plan_leaf" | "plan_split" | "plan_cant_meet" => Phase::Planning { must_split: false },
        "maker_done" | "maker_needs_files" | "maker_cant_meet" | "maker_out_of_room" => Phase::Making,
        "checks_done" => Phase::Checking,
        "approved" | "reviewer_out_of_room" | "changes_requested" => Phase::Reviewing,
        "rebase_clean" | "rebase_needs_changes" => Phase::Rebasing,
        "outside_push" | "ci_failed" | "ack_body_policy_refused" => Phase::Publishing,
        k if k.starts_with("answer_") => Phase::AwaitingRob,
        _ => Phase::Ingesting,
    }
}

fn payloads(kind: &str) -> usize {
    match kind {
        "new_request" | "new_adoption" => 20,
        "ingest_done" => 256, "ingest_early_end" => 3,
        "plan_leaf" | "maker_needs_files" => 32,
        "maker_done" | "approved" | "rebase_clean" | "outside_push" => 8,
        "checks_done" => 8, "ci_failed" => 2, "answer_add_credit" => 12,
        _ => 1,
    }
}

fn event(kind: &str, p: usize, w: &World, index: usize, rng: &mut Prng) -> Event {
    let b = &w.nodes[0];
    let generation = [b.generation, b.generation.wrapping_add(1), 0, u64::MAX, rng.next()][index % 5];
    let seq = [b.effect.map_or(0, |e| e.seq), b.next_seq, u64::MAX, rng.next()][index % 4];
    let id = [b.last_cmd.saturating_add(1), b.last_cmd, u64::MAX, rng.next()][index % 4];
    let r = |reply| Event::Reply { node: NodeId(0), generation, reply };
    let c = |command| Event::Command { node: NodeId(0), id, command };
    let a = |answer| c(Command::Answer { question: [1, 0, u64::MAX][index % 3], answer });
    let ack = |ack| Event::Ack { node: NodeId(0), seq, ack };
    match kind {
        "new_request" => Event::NewRequest { repo: RepoId([0, 1, 2, u64::MAX][p / 5]), priority: priorities()[p % 5] },
        "new_adoption" => Event::NewAdoption { repo: RepoId([0, 1, 2, u64::MAX][p / 5]), head: HeadId(7), priority: priorities()[p % 5] },
        "ingest_done" => r(Reply::IngestDone { size: p as u8 }),
        "ingest_early_end" => r(Reply::IngestEarlyEnd([EarlyEnd::Duplicate, EarlyEnd::AlreadyDone, EarlyEnd::NotActionable][p])),
        "protected_paths" => r(Reply::ProtectedPaths), "audit_agrees" => r(Reply::AuditAgrees),
        "plan_leaf" => r(Reply::PlanLeaf { footprint: Footprint::inline((p / 2) as u64), fits: p % 2 != 0 }),
        "plan_split" => r(Reply::PlanSplit), "plan_cant_meet" => r(Reply::PlanCantMeet),
        "maker_done" => r(Reply::MakerDone { head: HeadId(p as u64) }),
        "maker_needs_files" => r(Reply::MakerNeedsFiles { files: Footprint::inline((p / 2) as u64), fits: p % 2 != 0 }),
        "maker_cant_meet" => r(Reply::MakerCantMeet), "maker_out_of_room" => r(Reply::MakerOutOfRoom),
        "checks_done" => r(Reply::ChecksDone { report: p & 1 != 0, pass: p & 2 != 0, in_footprint: p & 4 != 0 }),
        "approved" => r(Reply::Approved { head: HeadId(p as u64) }),
        "reviewer_out_of_room" => r(Reply::ReviewerOutOfRoom), "changes_requested" => r(Reply::ChangesRequested),
        "rebase_clean" => r(Reply::RebaseClean { head: HeadId(p as u64) }),
        "rebase_needs_changes" => r(Reply::RebaseNeedsChanges), "run_failed" => r(Reply::RunFailed),
        "ack_confirmed" => ack(Ack::Confirmed), "ack_delivery_accepted" => ack(Ack::DeliveryAccepted),
        "ack_retryable" => ack(Ack::Retryable), "ack_permanent" => ack(Ack::Permanent),
        "ack_body_policy_refused" => ack(Ack::BodyPolicyRefused),
        "outside_push" => Event::Fact { node: NodeId(0), fact: Fact::PushByOther { head: HeadId(p as u64) } },
        "ci_failed" => Event::Fact { node: NodeId(0), fact: Fact::CiFailed { infra: p != 0 } },
        "stop" => c(Command::Stop), "pause" => c(Command::Pause), "resume" => c(Command::Resume),
        "change_request" => c(Command::ChangeRequest), "answer_continue" => a(Answer::Continue),
        "answer_try_again" => a(Answer::TryAgain), "answer_change_request" => a(Answer::ChangeRequest),
        "answer_stop" => a(Answer::Stop), "answer_unclear" => a(Answer::Unclear),
        "answer_retry" => a(Answer::Retry), "answer_mark_handled" => a(Answer::MarkHandled),
        "answer_add_credit" => a(Answer::AddCredit([0, 1, 2, 3, 4, 7, CREDIT_LIMIT - 1, LIMIT - 1, u64::MAX, u64::MAX - 1, rng.next(), rng.next()][p])),
        _ => panic!("unknown event family {kind}"),
    }
}

// The extra assertions in the original BodyPolicyRefused harness, after the
// generic check_apply. Only the private apply call is adapted to B's API.
fn body_refusal(before: &World, event: Event) {
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

pub fn run_event(name: &str, kind: &str) {
    assert_source_identity();
    let start = Instant::now();
    let mut stats = Stats::default();
    let mut rng = Prng(A_SEED);
    let mut index = 0;
    for w in corpus(None, focused(kind)) {
        let accepted = is_consistent(&w) && is_settled(&w);
        let count = if accepted { payloads(kind) } else { 1 };
        for p in 0..count {
            let e = event(kind, p, &w, index, &mut rng);
            stats.case(index, A_SEED, Some(w.clone()), vec![], || {
                let before = assert_apply(e);
                if kind == "ack_body_policy_refused" { body_refusal(&before, e); }
            });
            index += 1;
        }
    }
    stats.finish(name, A_SEED, start);
}

pub fn run_advance(name: &str, ps: &[Phase], moves: bool) {
    assert_source_identity();
    let start = Instant::now();
    let mut stats = Stats::default();
    let mut index = 0;
    for &phase in ps {
        for w in corpus(Some(phase), phase) {
            stats.case(index, A_SEED, Some(w), vec![], || assert_advance(phase, moves));
            index += 1;
        }
    }
    stats.finish(name, A_SEED, start);
}
