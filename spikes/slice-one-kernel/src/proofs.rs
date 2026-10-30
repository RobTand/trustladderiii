//! Kani proofs of the state table's rules, one transition at a time.
//!
//! A step is one event applied by `apply`, then internal transitions taken by
//! `advance` until settle finds none. The proofs cover the two kinds of
//! transition separately, each from an arbitrary consistent world within the
//! bounds, so the rules hold for every step by induction:
//!
//! - `apply` and `advance` keep `consistent(world)` and the per-transition rules.
//! - Every `advance` that changes something lowers its node's `settle_rank`
//!   and leaves the other nodes alone, so settle ends for any number of requests.
//! - Since `settle_rank` is 4 × distance + at most 3, no `advance` raises the
//!   progress measure. The witness event lowers it, so from every settled
//!   world some outside event moves the work closer to done: no dead states.
//!
//! The proofs are as small as they usefully get, so they run fast and can run
//! on every change. Each covers one event variant, or one phase, and acts on
//! request 0. Requests are interchangeable, so request 0 stands for any of
//! them. A concrete event or phase lets Kani skip every path that case can't
//! reach.
//!
//! Worlds have exactly two requests and two repos of four files each, with W
//! and the agent pool at most 2. A closed request is inert, so this covers a
//! world with one open request as well. A new request makes three.

use super::*;

/// Counters stay far below overflow. Reaching this would take 2^40 events.
const LIMIT: u64 = 1 << 40;
/// Credit stays far below overflow too.
const CREDIT_LIMIT: u32 = 1 << 30;

fn small(n: &Node) -> bool {
    n.generation < LIMIT
        && n.next_seq < LIMIT
        && n.last_cmd < LIMIT
        && n.footprint < 16
        && n.credit.start < CREDIT_LIMIT
        && n.credit.earned < CREDIT_LIMIT
        && n.credit.added < CREDIT_LIMIT
        && n.credit.spent < CREDIT_LIMIT
        && n.ticket.map_or(true, |t| t < LIMIT)
        && n.run.map_or(true, |g| g < LIMIT)
        && n.job.map_or(true, |g| g < LIMIT)
        && n.effect.map_or(true, |e| e.seq < LIMIT)
        && n.question.map_or(true, |q| q.id < LIMIT)
}

/// Any consistent world, which may be partway through settling. With a phase
/// given, request 0 is in it.
fn any_world(phase: Option<Phase>) -> World {
    let mut nodes: Vec<Node> = kani::vec::exact_vec::<Node, 2>();
    if let Some(phase) = phase {
        nodes[0].phase = phase;
    }
    let mut gates = Vec::new();
    gates.push(Gate { w: kani::any() });
    gates.push(Gate { w: kani::any() });
    let world = World {
        nodes,
        gates,
        pool_capacity: kani::any(),
        next_ticket: kani::any(),
        next_question: kani::any(),
    };
    kani::assume(world.pool_capacity <= 2);
    kani::assume(world.gates[0].w <= 2 && world.gates[1].w <= 2);
    kani::assume(world.next_ticket < LIMIT && world.next_question < LIMIT);
    kani::assume(small(&world.nodes[0]) && small(&world.nodes[1]));
    kani::assume(world.consistent());
    world
}

/// Any world at rest, where events arrive.
fn any_settled_world(phase: Option<Phase>) -> World {
    let world = any_world(phase);
    kani::assume(world.is_settled());
    world
}

/// Asserts the world is consistent. Each rule has its own panic, so Kani
/// names the one that breaks.
fn require_consistent(w: &World) {
    match w.check() {
        Ok(()) => {}
        Err(Broken::Config) => panic!("broken: config"),
        Err(Broken::Agenda) => panic!("broken: agenda"),
        Err(Broken::Head) => panic!("broken: head"),
        Err(Broken::Effect) => panic!("broken: effect"),
        Err(Broken::Holds) => panic!("broken: holds"),
        Err(Broken::Run) => panic!("broken: run"),
        Err(Broken::Pool) => panic!("broken: pool"),
        Err(Broken::Ticket) => panic!("broken: ticket"),
        Err(Broken::Question) => panic!("broken: question"),
        Err(Broken::Incident) => panic!("broken: incident"),
        Err(Broken::Credit) => panic!("broken: credit"),
        Err(Broken::Leaf) => panic!("broken: leaf"),
        Err(Broken::Pr) => panic!("broken: pr"),
        Err(Broken::Teardown) => panic!("broken: teardown"),
        Err(Broken::Merge) => panic!("broken: merge"),
        Err(Broken::Closed) => panic!("broken: closed"),
        Err(Broken::Gate) => panic!("broken: gate"),
    }
}

fn teardown(kind: EffectKind) -> bool {
    matches!(
        kind,
        EffectKind::StopRun | EffectKind::WithdrawJob | EffectKind::ClosePr | EffectKind::RemoveWorktree
    )
}

/// Rules about each node, comparing it before and after a transition.
fn check_nodes(before: &World, after: &World) {
    let mut i = 0;
    while i < before.nodes.len() {
        let b = &before.nodes[i];
        let n = &after.nodes[i];
        // Effect keys only move forward.
        assert!(n.next_seq >= b.next_seq);
        // Closed is final. Only a late merge changes it, from Stopped or Replaced to Merged.
        if let Phase::Closed(o) = b.phase {
            let corrected = n.phase == Phase::Closed(Outcome::Merged)
                && matches!(o, Outcome::Stopped | Outcome::Replaced);
            assert!(n.phase == b.phase || corrected);
        }
        // A merged leaf earns its credit once.
        assert!(n.credit.earned <= b.credit.earned + 1);
        if n.credit.earned > b.credit.earned {
            assert!(!b.credit.merge_credited && n.credit.merge_credited);
        }
        i += 1;
    }
}

/// Rules about the effects a transition emits. `exempt` is a node allowed to
/// emit while paused: one finishing an effect already in flight, or resuming.
fn check_outputs(before: &World, after: &World, out: &[Out], exempt: Option<usize>) {
    for o in out {
        if let Out::Effect { node, seq, kind, head } = *o {
            let n = &after.nodes[node];
            assert!(seq < n.next_seq);
            if let Some(b) = before.nodes.get(node) {
                // Every effect gets a key no earlier effect of its node had.
                assert!(seq >= b.next_seq);
                // A paused node starts nothing new, though teardown still runs.
                if b.paused && !b.phase.is_stopping() && exempt != Some(node) {
                    assert!(teardown(kind));
                }
            }
            // TL3 enqueues a head only after that exact head passed its checks and review.
            if kind == EffectKind::Enqueue {
                assert!(matches!(head, Some(h) if has(n.approved, h) && n.checked == Some(h)));
            }
        }
    }
}

/// Applying one event keeps the world consistent and keeps every rule.
fn check_apply(event: Event) {
    let before = any_settled_world(None);
    let mut after = before.clone();
    let mut out = Vec::new();
    apply(&mut after, event, &mut out);
    // Not vacuous: some world in the bounds is one this event changes.
    kani::cover!(after != before, "the event changes the world");
    require_consistent(&after);
    check_nodes(&before, &after);
    let exempt = match event {
        Event::Ack { node, .. } => Some(node as usize),
        Event::Command { node, command: Command::Resume, .. } => Some(node as usize),
        _ => None,
    };
    check_outputs(&before, &after, &out, exempt);
    // Each of Rob's answers applies only to the question it names, and only
    // once: anything it changes closes that question.
    if let Event::Command { command: Command::Answer { question, .. }, .. } = event {
        let mut unchanged = before.nodes[0];
        unchanged.last_cmd = after.nodes[0].last_cmd;
        if after.nodes[0] != unchanged {
            assert!(before.nodes[0].question.is_some_and(|q| q.id == question));
            assert!(after.nodes[0].question.map_or(true, |q| q.id != question));
        }
    }
}

/// One internal transition of request 0 keeps the world consistent, changes
/// only that request, and keeps every rule. It also lowers the request's
/// settle rank, so settle ends, and it never raises the progress measure.
fn check_advance(phase: Phase, moves: bool) {
    let before = any_world(Some(phase));
    let rank = before.nodes[0].settle_rank();
    let mut after = before.clone();
    let mut out = Vec::new();
    let changed = advance(&mut after, 0, &mut out);
    // Not vacuous: settle takes a transition from this phase, or, for a phase
    // that only waits on outside events, a world in it exists. One cover per
    // harness, so every cover a proof contains is one it can reach.
    kani::cover!(changed == moves, "the phase moves under settle, or waits");
    require_consistent(&after);
    if changed {
        assert!(after.nodes[0].settle_rank() < rank);
        assert!(after.nodes[1] == before.nodes[1]);
        check_nodes(&before, &after);
        check_outputs(&before, &after, &out, None);
    } else {
        assert!(after == before && out.is_empty());
    }
}

/// No dead states: while any request is open, the witness names an outside
/// event, and applying it lowers the progress measure. Settle can't raise it
/// again, so the whole step lowers it.
fn check_witness(phase: Phase) {
    let world = any_settled_world(Some(phase));
    kani::assume(!phase.is_closed() || !world.nodes[1].phase.is_closed());
    let before = world.measure();
    let event = world.witness();
    kani::cover!(event.is_some(), "an open settled world exists here");
    assert!(event.is_some());
    if let Some(event) = event {
        let mut after = world;
        let mut out = Vec::new();
        apply(&mut after, event, &mut out);
        assert!(after.measure() < before);
    }
}

fn reply(reply: Reply) -> Event {
    Event::Reply { node: 0, generation: kani::any(), reply }
}

fn ack(ack: Ack) -> Event {
    Event::Ack { node: 0, seq: kani::any(), ack }
}

fn fact(fact: Fact) -> Event {
    Event::Fact { node: 0, fact }
}

fn command(command: Command) -> Event {
    Event::Command { node: 0, id: kani::any(), command }
}

fn answer(answer: Answer) -> Event {
    command(Command::Answer { question: kani::any(), answer })
}

macro_rules! proof {
    ($name:ident, $body:expr) => {
        #[kani::proof]
        #[kani::unwind(6)]
        fn $name() {
            $body;
        }
    };
}

// Applying each event variant.
proof!(apply_new_request, check_apply(Event::NewRequest { repo: kani::any() }));
proof!(apply_ingest_done, check_apply(reply(Reply::IngestDone { size: kani::any() })));
proof!(apply_ingest_early_end, check_apply(reply(Reply::IngestEarlyEnd(kani::any()))));
proof!(apply_audit_agrees, check_apply(reply(Reply::AuditAgrees)));
proof!(apply_audit_disagrees, check_apply(reply(Reply::AuditDisagrees)));
proof!(apply_plan_leaf, check_apply(reply(Reply::PlanLeaf { footprint: kani::any(), fits: kani::any() })));
proof!(apply_plan_split, check_apply(reply(Reply::PlanSplit)));
proof!(apply_plan_cant_meet, check_apply(reply(Reply::PlanCantMeet)));
proof!(apply_maker_done, check_apply(reply(Reply::MakerDone { head: kani::any() })));
proof!(apply_maker_needs_files, check_apply(reply(Reply::MakerNeedsFiles { files: kani::any(), fits: kani::any() })));
proof!(apply_maker_cant_meet, check_apply(reply(Reply::MakerCantMeet)));
proof!(apply_maker_out_of_room, check_apply(reply(Reply::MakerOutOfRoom)));
proof!(apply_checks_done, check_apply(reply(Reply::ChecksDone { report: kani::any(), pass: kani::any(), in_footprint: kani::any() })));
proof!(apply_approved, check_apply(reply(Reply::Approved { head: kani::any() })));
proof!(apply_changes_requested, check_apply(reply(Reply::ChangesRequested)));
proof!(apply_rebase_clean, check_apply(reply(Reply::RebaseClean { head: kani::any() })));
proof!(apply_rebase_needs_changes, check_apply(reply(Reply::RebaseNeedsChanges)));
proof!(apply_run_failed, check_apply(reply(Reply::RunFailed)));
proof!(apply_ack_confirmed, check_apply(ack(Ack::Confirmed)));
proof!(apply_ack_retryable, check_apply(ack(Ack::Retryable)));
proof!(apply_ack_permanent, check_apply(ack(Ack::Permanent)));
proof!(apply_pr_closed, check_apply(fact(Fact::PrClosedByPerson)));
proof!(apply_pr_dequeued, check_apply(fact(Fact::PrDequeuedByPerson)));
proof!(apply_outside_push, check_apply(fact(Fact::PushByOther { head: kani::any() })));
proof!(apply_ci_failed, check_apply(fact(Fact::CiFailed { infra: kani::any() })));
proof!(apply_conflict, check_apply(fact(Fact::Conflict)));
proof!(apply_incident_cleared, check_apply(fact(Fact::IncidentCleared { by_rob: kani::any() })));
proof!(apply_stop, check_apply(command(Command::Stop)));
proof!(apply_pause, check_apply(command(Command::Pause)));
proof!(apply_resume, check_apply(command(Command::Resume)));
proof!(apply_change_request, check_apply(command(Command::ChangeRequest)));
proof!(apply_answer_continue, check_apply(answer(Answer::Continue)));
proof!(apply_answer_try_again, check_apply(answer(Answer::TryAgain)));
proof!(apply_answer_add_credit, check_apply(answer(Answer::AddCredit(kani::any()))));
proof!(apply_answer_change_request, check_apply(answer(Answer::ChangeRequest)));
proof!(apply_answer_stop, check_apply(answer(Answer::Stop)));
proof!(apply_answer_unclear, check_apply(answer(Answer::Unclear)));
proof!(apply_answer_retry, check_apply(answer(Answer::Retry)));
proof!(apply_answer_mark_handled, check_apply(answer(Answer::MarkHandled)));

// Settle's internal transitions, one phase at a time.
proof!(advance_ingesting, check_advance(Phase::Ingesting, true));
proof!(advance_auditing, check_advance(Phase::Auditing, true));
proof!(advance_unplanned, check_advance(Phase::Unplanned, true));
proof!(advance_planning, check_advance(Phase::Planning { must_split: kani::any() }, true));
proof!(advance_ready, check_advance(Phase::Ready(kani::any()), true));
proof!(advance_making, check_advance(Phase::Making, true));
proof!(advance_checking, check_advance(Phase::Checking, true));
proof!(advance_reviewing, check_advance(Phase::Reviewing, true));
proof!(advance_publishing, check_advance(Phase::Publishing, true));
proof!(advance_queued, check_advance(Phase::Queued, false));
proof!(advance_rebasing, check_advance(Phase::Rebasing, true));
proof!(advance_awaiting_rob, check_advance(Phase::AwaitingRob, false));
proof!(advance_stopping, check_advance(Phase::Stopping(kani::any()), true));
proof!(advance_closed, check_advance(Phase::Closed(kani::any()), false));

// No dead states, one phase at a time.
proof!(progress_from_ingesting, check_witness(Phase::Ingesting));
proof!(progress_from_auditing, check_witness(Phase::Auditing));
proof!(progress_from_unplanned, check_witness(Phase::Unplanned));
proof!(progress_from_planning, check_witness(Phase::Planning { must_split: kani::any() }));
proof!(progress_from_ready, check_witness(Phase::Ready(kani::any())));
proof!(progress_from_making, check_witness(Phase::Making));
proof!(progress_from_checking, check_witness(Phase::Checking));
proof!(progress_from_reviewing, check_witness(Phase::Reviewing));
proof!(progress_from_publishing, check_witness(Phase::Publishing));
proof!(progress_from_queued, check_witness(Phase::Queued));
proof!(progress_from_rebasing, check_witness(Phase::Rebasing));
proof!(progress_from_awaiting_rob, check_witness(Phase::AwaitingRob));
proof!(progress_from_stopping, check_witness(Phase::Stopping(kani::any())));
proof!(progress_from_closed, check_witness(Phase::Closed(kani::any())));

/// A merge TL3 observes keeps every rule, is recorded truthfully, and a merge
/// of a head TL3 didn't approve raises an incident and a note to Rob. Merges
/// are the heaviest event to prove, so they get one proof per phase.
fn check_pr_merged(phase: Phase) {
    let before = any_settled_world(Some(phase));
    let n = before.nodes[0];
    let head: u8 = kani::any();
    let checks_passed: bool = kani::any();
    let event = fact(Fact::PrMerged { head, checks_passed });
    let mut after = before.clone();
    let mut out = Vec::new();
    apply(&mut after, event, &mut out);
    kani::cover!(after != before, "the merge changes the world");
    require_consistent(&after);
    check_nodes(&before, &after);
    check_outputs(&before, &after, &out, None);
    if n.had_pr && n.merged.is_none() && !n.phase.is_closed() && head < MAX_HEADS {
        let m = after.nodes[0];
        assert!(m.merged == Some(head));
        assert!(m.phase == Phase::Stopping(Outcome::Merged));
        if !(has(n.approved, head) && checks_passed) {
            let noted = out.iter().any(|o| {
                matches!(o, Out::Notice { node: 0, notice: Notice::Note(NoteKind::UnapprovedMerge) })
            });
            assert!(noted);
        }
    }
}

proof!(pr_merged_ingesting, check_pr_merged(Phase::Ingesting));
proof!(pr_merged_auditing, check_pr_merged(Phase::Auditing));
proof!(pr_merged_unplanned, check_pr_merged(Phase::Unplanned));
proof!(pr_merged_planning, check_pr_merged(Phase::Planning { must_split: kani::any() }));
proof!(pr_merged_ready, check_pr_merged(Phase::Ready(kani::any())));
proof!(pr_merged_making, check_pr_merged(Phase::Making));
proof!(pr_merged_checking, check_pr_merged(Phase::Checking));
proof!(pr_merged_reviewing, check_pr_merged(Phase::Reviewing));
proof!(pr_merged_publishing, check_pr_merged(Phase::Publishing));
proof!(pr_merged_queued, check_pr_merged(Phase::Queued));
proof!(pr_merged_rebasing, check_pr_merged(Phase::Rebasing));
proof!(pr_merged_awaiting_rob, check_pr_merged(Phase::AwaitingRob));
proof!(pr_merged_stopping, check_pr_merged(Phase::Stopping(kani::any())));
proof!(pr_merged_closed, check_pr_merged(Phase::Closed(kani::any())));
