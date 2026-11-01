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
//! at most 3 and the agent pool at most 2. A closed request is inert, so this covers a
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
        && n.repo.0 < 2
        && [n.proposed, n.head, n.checked, n.approved, n.merged]
            .into_iter()
            .flatten()
            .all(|h| h.0 < 8)
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
        file_sets: Vec::new(),
    };
    kani::assume(world.pool_capacity <= 2);
    kani::assume(world.gates[0].w <= 3 && world.gates[1].w <= 3);
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
        Err(Broken::Footprint) => panic!("broken: footprint handle"),
        Err(Broken::Config) => panic!("broken: config"),
        Err(Broken::Agenda) => panic!("broken: agenda"),
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
        EffectKind::StopRun
            | EffectKind::WithdrawJob
            | EffectKind::ClosePr
            | EffectKind::RemoveWorktree
    )
}

/// Rules about each node, comparing it before and after a transition.
fn check_nodes(before: &World, after: &World) {
    let mut i = 0;
    while i < before.nodes.len() {
        let b = &before.nodes[i];
        let n = &after.nodes[i];
        assert!(n.proposed == b.proposed);
        assert!(!b.adopted || n.adopted);
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
fn check_outputs(before: &World, after: &World, out: &Outbox, exempt: Option<usize>) {
    let mut slot = 0;
    while slot < OUTBOX {
        if let Some(Out::Effect {
            node,
            seq,
            kind,
            head,
        }) = out.get(slot)
        {
            assert!(node.index().is_some_and(|i| i < after.nodes.len()));
            // Look the node up by a concrete index. A read at a symbolic index
            // into nodes the event just rewrote made the merge proofs need
            // more than 48 GB; this way they need a few.
            let mut i = 0;
            while i < after.nodes.len() {
                if NodeId(i as u64) == node {
                    check_effect(
                        before.nodes.get(i),
                        &after.nodes[i],
                        seq,
                        kind,
                        head,
                        exempt == Some(i),
                    );
                }
                i += 1;
            }
        }
        slot += 1;
    }
}
fn check_effect(
    before: Option<&Node>,
    n: &Node,
    seq: u64,
    kind: EffectKind,
    head: Option<HeadId>,
    exempt: bool,
) {
    assert!(seq < n.next_seq);
    if let Some(b) = before {
        // Every effect gets a key no earlier effect of its node had.
        assert!(seq >= b.next_seq);
        // A paused node starts nothing new, though teardown still runs.
        if b.paused && !b.phase.is_stopping() && !exempt {
            assert!(teardown(kind));
        }
    }
    // TL3 enqueues a head only after that exact head passed its checks and review.
    if kind == EffectKind::Enqueue {
        assert!(matches!(head, Some(h) if n.approved == Some(h) && n.checked == Some(h)));
    }
}

fn rebase_progress_applies(node: &Node, generation: u64) -> bool {
    node.phase == Phase::Rebasing
        && node.current() == Some(Item::Await)
        && node.run == Some(generation)
}

/// Applying one event keeps the world consistent and keeps every rule.
fn check_apply(event: Event) -> World {
    let before = any_settled_world(None);
    let mut after = before.clone();
    let mut out = Outbox::new();
    apply(&mut after, event.clone(), &mut out);
    // Not vacuous: some world in the bounds is one this event changes.
    kani::cover!(after != before, "the event changes the world");
    if let Event::NewAdoption { repo, head } = &event {
        if repo.index().is_some_and(|r| r < before.gates.len()) {
            assert_eq!(after.nodes.len(), before.nodes.len() + 1);
            assert_eq!(after.nodes.last().unwrap().proposed, Some(*head));
            assert!(!after.nodes.last().unwrap().adopted);
        }
    }
    require_consistent(&after);
    check_nodes(&before, &after);
    let exempt = match event {
        Event::Ack { node, .. } => node.index(),
        Event::Command {
            node,
            command: Command::Resume,
            ..
        } => node.index(),
        _ => None,
    };
    check_outputs(&before, &after, &out, exempt);
    if let Event::Reply {
        generation,
        reply: Reply::ReviewerOutOfRoom,
        ..
    } = event
    {
        let b = &before.nodes[0];
        let n = &after.nodes[0];
        if b.phase == Phase::Reviewing
            && b.current() == Some(Item::Await)
            && b.run == Some(generation)
        {
            assert_eq!(n.run_failures, 0);
            assert_eq!(n.attempt, b.attempt);
            assert_eq!(n.round, b.round);
            if b.review_restarted {
                assert!(!n.holds);
                if b.credit.can_spend(1) {
                    assert_eq!(n.phase, Phase::Planning { must_split: true });
                    assert_eq!(n.credit.spent, b.credit.spent + 1);
                } else {
                    assert_eq!(n.phase, Phase::AwaitingRob);
                    assert_eq!(
                        n.question.unwrap().cont,
                        Cont::Planning { must_split: true }
                    );
                }
            } else {
                assert!(n.review_restarted);
                assert_eq!(n.rechecks, b.rechecks.saturating_add(1));
                assert_eq!(n.checked, b.checked);
                if n.rechecks <= FREE_RECHECKS || b.credit.can_spend(1) {
                    assert_eq!(n.phase, Phase::Reviewing);
                    assert!(n.generation > b.generation);
                    assert_eq!(
                        n.credit.spent,
                        b.credit.spent + u32::from(n.rechecks > FREE_RECHECKS)
                    );
                } else {
                    assert_eq!(n.phase, Phase::AwaitingRob);
                    assert_eq!(n.question.unwrap().cont, Cont::Ready(Resume::Reviewing));
                }
            }
        }
    }
    if let Event::Fact {
        fact: Fact::Conflict,
        ..
    } = event
    {
        let b = &before.nodes[0];
        let n = &after.nodes[0];
        if matches!(b.phase, Phase::Publishing | Phase::Queued) {
            assert_eq!(n.phase, Phase::Rebasing);
            assert_eq!(n.generation, b.generation + 1);
            assert_eq!(n.credit, b.credit);
            assert_eq!(n.holds, b.holds);
        } else {
            assert_eq!(n, b, "a repeated Conflict cannot restart the rebase");
        }
    }
    if let Event::Reply {
        generation,
        reply: Reply::RebaseClean { .. } | Reply::RebaseNeedsChanges,
        ..
    } = event
    {
        let b = &before.nodes[0];
        let n = &after.nodes[0];
        if rebase_progress_applies(b, generation) {
            // Each completed catch-up consumes finite free allowance or credit, or asks Rob.
            let fuel =
                |n: &Node| n.credit.balance() + i64::from(FREE_RECHECKS.saturating_sub(n.rechecks));
            assert!(n.phase == Phase::AwaitingRob || fuel(n) < fuel(b));
        }
    }
    // Each of Rob's answers applies only to the question it names, and only
    // once: anything it changes closes that question.
    if let Event::Command {
        command: Command::Answer { question, .. },
        ..
    } = event
    {
        let mut unchanged = before.nodes[0].clone();
        unchanged.last_cmd = after.nodes[0].last_cmd;
        if after.nodes[0] != unchanged {
            assert!(before.nodes[0].question.is_some_and(|q| q.id == question));
            assert!(after.nodes[0].question.map_or(true, |q| q.id != question));
        }
    }
    before
}

/// One internal transition of request 0 keeps the world consistent, changes
/// only that request, and keeps every rule. It also lowers the request's
/// settle rank, so settle ends, and it never raises the progress measure.
fn check_advance(phase: Phase, moves: bool) {
    let before = any_world(Some(phase));
    let rank = before.nodes[0].settle_rank();
    let mut after = before.clone();
    let mut out = Outbox::new();
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
        let mut out = Outbox::new();
        apply(&mut after, event.clone(), &mut out);
        assert!(after.measure() < before);
    }
}

fn reply(reply: Reply) -> Event {
    match &reply {
        Reply::MakerDone { head } | Reply::Approved { head } | Reply::RebaseClean { head } => {
            kani::assume(head.0 < 8)
        }
        _ => {}
    }
    Event::Reply {
        node: NodeId(0),
        generation: kani::any(),
        reply,
    }
}

fn ack(ack: Ack) -> Event {
    Event::Ack {
        node: NodeId(0),
        seq: kani::any(),
        ack,
    }
}

fn fact(fact: Fact) -> Event {
    if let Fact::PushByOther { head } = fact {
        kani::assume(head.0 < 8);
    }
    Event::Fact {
        node: NodeId(0),
        fact,
    }
}

fn command(command: Command) -> Event {
    Event::Command {
        node: NodeId(0),
        id: kani::any(),
        command,
    }
}

fn answer(answer: Answer) -> Event {
    command(Command::Answer {
        question: kani::any(),
        answer,
    })
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
proof!(
    apply_new_request,
    check_apply(Event::NewRequest { repo: kani::any() })
);
proof!(
    apply_ingest_done,
    check_apply(reply(Reply::IngestDone { size: kani::any() }))
);
proof!(
    apply_ingest_early_end,
    check_apply(reply(Reply::IngestEarlyEnd(kani::any())))
);
proof!(apply_audit_agrees, check_apply(reply(Reply::AuditAgrees)));
proof!(
    apply_audit_disagrees,
    check_apply(reply(Reply::AuditDisagrees))
);
proof!(
    apply_plan_leaf,
    check_apply(reply(Reply::PlanLeaf {
        footprint: kani::any(),
        fits: kani::any()
    }))
);
proof!(apply_plan_split, check_apply(reply(Reply::PlanSplit)));
proof!(
    apply_plan_cant_meet,
    check_apply(reply(Reply::PlanCantMeet))
);
proof!(
    apply_maker_done,
    check_apply(reply(Reply::MakerDone { head: kani::any() }))
);
proof!(
    apply_maker_needs_files,
    check_apply(reply(Reply::MakerNeedsFiles {
        files: kani::any(),
        fits: kani::any()
    }))
);
proof!(
    apply_maker_cant_meet,
    check_apply(reply(Reply::MakerCantMeet))
);
proof!(
    apply_maker_out_of_room,
    check_apply(reply(Reply::MakerOutOfRoom))
);
proof!(
    apply_checks_done,
    check_apply(reply(Reply::ChecksDone {
        report: kani::any(),
        pass: kani::any(),
        in_footprint: kani::any()
    }))
);
proof!(
    apply_approved,
    check_apply(reply(Reply::Approved { head: kani::any() }))
);
proof!(apply_reviewer_out_of_room, {
    let event = reply(Reply::ReviewerOutOfRoom);
    let Event::Reply { generation, .. } = event else {
        unreachable!()
    };
    let before = check_apply(event);
    let b = &before.nodes[0];
    let accepted = b.phase == Phase::Reviewing
        && b.current() == Some(Item::Await)
        && b.run == Some(generation);
    let paid = b.rechecks.saturating_add(1) > FREE_RECHECKS;
    kani::cover!(
        accepted && !b.review_restarted && !paid,
        "first exhaustion: free review"
    );
    kani::cover!(
        accepted && !b.review_restarted && paid && b.credit.can_spend(1),
        "first exhaustion: paid review"
    );
    kani::cover!(
        accepted && !b.review_restarted && paid && !b.credit.can_spend(1),
        "first exhaustion: needs credit"
    );
    kani::cover!(
        accepted && b.review_restarted && b.credit.can_spend(1),
        "second exhaustion: paid split"
    );
    kani::cover!(
        accepted && b.review_restarted && !b.credit.can_spend(1),
        "second exhaustion: split needs credit"
    );
});
proof!(
    apply_changes_requested,
    check_apply(reply(Reply::ChangesRequested))
);
proof!(apply_rebase_clean, {
    let event = reply(Reply::RebaseClean { head: kani::any() });
    let Event::Reply { generation, .. } = event else {
        unreachable!()
    };
    let before = check_apply(event);
    let b = &before.nodes[0];
    let accepted = rebase_progress_applies(b, generation);
    let free = b.rechecks.saturating_add(1) <= FREE_RECHECKS;
    let funded = b.credit.can_spend(1);
    kani::cover!(accepted && free, "clean rebase uses the free allowance");
    kani::cover!(accepted && !free && funded, "clean rebase spends credit");
    kani::cover!(accepted && !free && !funded, "clean rebase needs credit");
});
proof!(apply_rebase_needs_changes, {
    let event = reply(Reply::RebaseNeedsChanges);
    let Event::Reply { generation, .. } = event else {
        unreachable!()
    };
    let before = check_apply(event);
    let b = &before.nodes[0];
    let accepted = rebase_progress_applies(b, generation);
    let free = b.rechecks.saturating_add(1) <= FREE_RECHECKS;
    let funded = b.credit.can_spend(1);
    kani::cover!(
        accepted && free && funded,
        "conflict fix pays despite free allowance"
    );
    kani::cover!(
        accepted && !free && funded,
        "conflict fix spends credit after allowance"
    );
    kani::cover!(accepted && !funded, "conflict fix needs credit");
});
proof!(apply_run_failed, check_apply(reply(Reply::RunFailed)));
proof!(apply_ack_confirmed, check_apply(ack(Ack::Confirmed)));
proof!(apply_ack_retryable, check_apply(ack(Ack::Retryable)));
proof!(apply_ack_permanent, check_apply(ack(Ack::Permanent)));
proof!(apply_pr_closed, check_apply(fact(Fact::PrClosedByPerson)));
proof!(
    apply_pr_dequeued,
    check_apply(fact(Fact::PrDequeuedByPerson))
);
proof!(
    apply_outside_push,
    check_apply(fact(Fact::PushByOther { head: kani::any() }))
);
proof!(
    apply_ci_failed,
    check_apply(fact(Fact::CiFailed { infra: kani::any() }))
);
fn check_conflict() {
    let before = check_apply(fact(Fact::Conflict));
    kani::cover!(
        before.nodes[0].phase == Phase::Publishing,
        "Q46 conflict before the queue"
    );
    kani::cover!(
        before.nodes[0].phase == Phase::Queued,
        "conflict after enqueue"
    );
}
proof!(apply_conflict, check_conflict());
proof!(
    apply_upstream_stalled,
    check_apply(fact(Fact::UpstreamStalled))
);
proof!(
    apply_incident_cleared,
    check_apply(fact(Fact::IncidentCleared {
        by_rob: kani::any()
    }))
);
proof!(apply_stop, check_apply(command(Command::Stop)));
proof!(apply_pause, check_apply(command(Command::Pause)));
proof!(apply_resume, check_apply(command(Command::Resume)));
proof!(
    apply_change_request,
    check_apply(command(Command::ChangeRequest))
);
proof!(apply_answer_continue, check_apply(answer(Answer::Continue)));
proof!(
    apply_answer_try_again,
    check_apply(answer(Answer::TryAgain))
);
proof!(
    apply_answer_add_credit,
    check_apply(answer(Answer::AddCredit(kani::any())))
);
proof!(
    apply_answer_change_request,
    check_apply(answer(Answer::ChangeRequest))
);
proof!(apply_answer_stop, check_apply(answer(Answer::Stop)));
proof!(apply_answer_unclear, check_apply(answer(Answer::Unclear)));
proof!(apply_answer_retry, check_apply(answer(Answer::Retry)));
proof!(
    apply_answer_mark_handled,
    check_apply(answer(Answer::MarkHandled))
);

// Settle's internal transitions, one phase at a time.
proof!(advance_ingesting, check_advance(Phase::Ingesting, true));
proof!(advance_auditing, check_advance(Phase::Auditing, true));
proof!(advance_unplanned, check_advance(Phase::Unplanned, true));
proof!(
    advance_planning,
    check_advance(
        Phase::Planning {
            must_split: kani::any()
        },
        true
    )
);
proof!(
    advance_ready,
    check_advance(Phase::Ready(kani::any()), true)
);
proof!(advance_making, check_advance(Phase::Making, true));
proof!(advance_checking, check_advance(Phase::Checking, true));
proof!(advance_reviewing, check_advance(Phase::Reviewing, true));
proof!(advance_publishing, check_advance(Phase::Publishing, true));
proof!(advance_queued, check_advance(Phase::Queued, false));
proof!(advance_rebasing, check_advance(Phase::Rebasing, true));
proof!(
    advance_awaiting_rob,
    check_advance(Phase::AwaitingRob, false)
);
proof!(
    advance_stopping,
    check_advance(Phase::Stopping(kani::any()), true)
);
proof!(
    advance_closed,
    check_advance(Phase::Closed(kani::any()), false)
);

// No dead states, one phase at a time.
proof!(progress_from_ingesting, check_witness(Phase::Ingesting));
proof!(progress_from_auditing, check_witness(Phase::Auditing));
proof!(progress_from_unplanned, check_witness(Phase::Unplanned));
proof!(
    progress_from_planning,
    check_witness(Phase::Planning {
        must_split: kani::any()
    })
);
proof!(
    progress_from_ready,
    check_witness(Phase::Ready(kani::any()))
);
proof!(progress_from_making, check_witness(Phase::Making));
proof!(progress_from_checking, check_witness(Phase::Checking));
proof!(progress_from_reviewing, check_witness(Phase::Reviewing));
proof!(progress_from_publishing, check_witness(Phase::Publishing));
proof!(progress_from_queued, check_witness(Phase::Queued));
proof!(progress_from_rebasing, check_witness(Phase::Rebasing));
proof!(
    progress_from_awaiting_rob,
    check_witness(Phase::AwaitingRob)
);
proof!(
    progress_from_stopping,
    check_witness(Phase::Stopping(kani::any()))
);
proof!(
    progress_from_closed,
    check_witness(Phase::Closed(kani::any()))
);

/// A merge TL3 observes keeps every rule, is recorded truthfully, and a merge
/// of a head TL3 didn't approve raises an incident and a note to Rob. Merges
/// are the heaviest event to prove, so they get one proof per phase.
fn check_pr_merged(phase: Phase) {
    let before = any_settled_world(Some(phase));
    let n = &before.nodes[0];
    let head: HeadId = kani::any();
    kani::assume(head.0 < 8);
    let checks_passed: bool = kani::any();
    let event = fact(Fact::PrMerged {
        head,
        checks_passed,
    });
    let mut after = before.clone();
    let mut out = Outbox::new();
    apply(&mut after, event.clone(), &mut out);
    kani::cover!(after != before, "the merge changes the world");
    require_consistent(&after);
    check_nodes(&before, &after);
    check_outputs(&before, &after, &out, None);
    if n.had_pr && n.merged.is_none() && !n.phase.is_closed() {
        let m = &after.nodes[0];
        assert!(m.merged == Some(head));
        assert!(m.phase == Phase::Stopping(Outcome::Merged));
        if !(n.approved == Some(head) && checks_passed) {
            assert!(out.contains(Out::Notice {
                node: NodeId(0),
                notice: Notice::Note(NoteKind::UnapprovedMerge)
            }));
        }
    }
}

proof!(pr_merged_ingesting, check_pr_merged(Phase::Ingesting));
proof!(pr_merged_auditing, check_pr_merged(Phase::Auditing));
proof!(pr_merged_unplanned, check_pr_merged(Phase::Unplanned));
proof!(
    pr_merged_planning,
    check_pr_merged(Phase::Planning {
        must_split: kani::any()
    })
);
proof!(pr_merged_ready, check_pr_merged(Phase::Ready(kani::any())));
proof!(pr_merged_making, check_pr_merged(Phase::Making));
proof!(pr_merged_checking, check_pr_merged(Phase::Checking));
proof!(pr_merged_reviewing, check_pr_merged(Phase::Reviewing));
proof!(pr_merged_publishing, check_pr_merged(Phase::Publishing));
proof!(pr_merged_queued, check_pr_merged(Phase::Queued));
proof!(pr_merged_rebasing, check_pr_merged(Phase::Rebasing));
proof!(pr_merged_awaiting_rob, check_pr_merged(Phase::AwaitingRob));
proof!(
    pr_merged_stopping,
    check_pr_merged(Phase::Stopping(kani::any()))
);
proof!(
    pr_merged_closed,
    check_pr_merged(Phase::Closed(kani::any()))
);

proof!(apply_new_adoption, {
    let repo = kani::any();
    let after = check_apply(Event::NewAdoption {
        repo,
        head: HeadId(7),
    });
    kani::cover!(
        repo.index().is_some_and(|r| r < after.gates.len()),
        "new adoption retains the requested head"
    );
});

fn adoption_preparation() -> World {
    let w = any_world(Some(Phase::Making));
    let n = &w.nodes[0];
    kani::assume(n.pos == 0 && n.proposed.is_some());
    kani::assume(
        n.effect
            .is_some_and(|e| e.kind == EffectKind::PrepareWorktree && !e.cancelling),
    );
    w
}

fn check_adoption_at_most_once() {
    let mut w = adoption_preparation();
    let before = w.nodes[0];
    let mut out = Outbox::new();
    on_ack(
        &mut w,
        0,
        before.effect.unwrap().seq,
        Ack::Confirmed,
        &mut out,
    );
    let after = w.nodes[0];
    if before.adopted {
        kani::cover!(true, "a later prepare preserves the existing head");
        assert!(after.adopted && after.head == before.head);
        assert!(after.phase == Phase::Making);
    } else {
        kani::cover!(true, "the first prepare adopts the proposed head");
        assert!(after.adopted && after.head == before.proposed);
        assert!(after.phase == Phase::Checking);
    }
    require_consistent(&w);
}

fn check_adoption_has_no_maker_credit() {
    let mut w = adoption_preparation();
    kani::assume(!w.nodes[0].adopted);
    let before = w.nodes[0];
    let mut out = Outbox::new();
    on_ack(
        &mut w,
        0,
        before.effect.unwrap().seq,
        Ack::Confirmed,
        &mut out,
    );
    let after = w.nodes[0];
    kani::cover!(after.phase == Phase::Checking, "adoption reaches checking");
    assert!(after.phase == Phase::Checking);
    assert!(after.credit == before.credit);
    assert!(after.attempt == before.attempt && after.round == before.round);
    assert!(after.run.is_none() && !after.pool && out.is_empty());
    require_consistent(&w);
}

proof!(apply_adoption_at_most_once, check_adoption_at_most_once());
proof!(
    apply_adoption_has_no_maker_credit,
    check_adoption_has_no_maker_credit()
);

// Four-node gate worlds cover a full W=3 gate and a waiter. The general
// event/phase proofs retain their two-node bounds.
fn gate_world(width: u8) -> World {
    gate_world_in_phase(width, None)
}

fn gate_world_in_phase(width: u8, first: Option<Phase>) -> World {
    gate_world_with_waiters(width, first, false)
}

fn gate_world_with_waiters(width: u8, first: Option<Phase>, waiting_only: bool) -> World {
    let mut w = World::new(&[width], 4);
    w.next_ticket = 8;
    for i in 0..4 {
        let mut n = Node::new(RepoId(0));
        n.credit.ready = true;
        n.credit.start = 4;
        n.attempt = 1;
        let files: u64 = kani::any();
        kani::assume(files > 0 && files < 16);
        n.footprint = Footprint::inline(files);
        n.paused = !waiting_only && kani::any();
        let phase: u8 = kani::any();
        kani::assume(phase < if waiting_only { 2 } else { 4 });
        n.phase = match phase {
            0 => Phase::Ready(Resume::Making),
            1 if waiting_only => Phase::Unplanned,
            1 => Phase::Queued,
            2 => Phase::Making,
            _ => Phase::Unplanned,
        };
        if i == 0
            && let Some(first) = first
        {
            n.phase = first;
        }
        if n.phase == Phase::Queued {
            n.holds = true;
            n.head = Some(HeadId(1));
            n.checked = n.head;
            n.approved = n.head;
            n.had_pr = true;
            n.pr_open = true;
            n.worktree = true;
        } else if n.phase == Phase::Making {
            n.holds = true;
            n.worktree = true;
            n.pos = 2;
            n.run = Some(n.generation);
            n.pool = true;
        } else if matches!(n.phase, Phase::Ready(_)) {
            n.ticket = kani::any();
        }
        w.nodes.push(n);
    }
    kani::assume(w.consistent());
    w
}

fn check_gate_grant(width: u8) {
    let before = gate_world(width);
    kani::assume(matches!(before.nodes[0].phase, Phase::Ready(_)));
    let n = before.nodes[0];
    let mut after = before.clone();
    let mut out = Outbox::new();
    let changed = advance(&mut after, 0, &mut out);
    require_consistent(&after);
    let holders = before.nodes.iter().filter(|n| n.holds).count();
    kani::cover!(
        after.nodes[0].holds && holders == usize::from(width - 1),
        "last available slot granted"
    );
    kani::cover!(
        n.ticket.is_some() && !n.paused && !slot_free(&before, 0),
        "full gate or overlapping footprint blocks"
    );
    kani::cover!(
        n.ticket.is_some() && !n.paused && slot_free(&before, 0) && !first_in_line(&before, 0),
        "older ticket keeps its place"
    );
    let mut older_blocked = false;
    for j in 1..before.nodes.len() {
        let other = before.nodes[j];
        if matches!(other.phase, Phase::Ready(_)) && !other.paused {
            if let (Some(theirs), Some(mine)) = (other.ticket, n.ticket) {
                if theirs < mine {
                    older_blocked |= !slot_free(&before, j);
                    assert!(
                        !after.nodes[0].holds,
                        "an older slot ticket prevents this grant"
                    );
                }
            }
        }
    }
    kani::cover!(
        !after.nodes[0].holds && older_blocked && slot_free(&before, 0) && !n.paused,
        "a file-blocked older waiter keeps its place despite spare capacity"
    );
    assert_eq!(&after.nodes[1..], &before.nodes[1..]);
    if after.nodes[0].holds {
        assert!(slot_free(&before, 0) && first_in_line(&before, 0) && !n.paused);
        assert!(after.nodes[0].ticket.is_none());
    } else {
        assert!(!after.nodes[0].pool);
        if n.ticket.is_some() && !n.paused {
            assert_eq!(after.nodes[0].ticket, n.ticket);
        }
    }
    if changed {
        assert!(after.nodes[0].settle_rank() < n.settle_rank());
    }
    check_nodes(&before, &after);
    check_outputs(&before, &after, &out, None);
}

fn check_gate_planning(width: u8) {
    let before = gate_world(width);
    kani::assume(before.nodes[0].phase == Phase::Unplanned);
    kani::assume(!before.nodes[0].paused);
    let waiting = before
        .nodes
        .iter()
        .filter(|n| matches!(n.phase, Phase::Ready(_)) && n.ticket.is_some())
        .count();
    let mut after = before.clone();
    let mut out = Outbox::new();
    let changed = advance(&mut after, 0, &mut out);
    require_consistent(&after);
    assert_eq!(changed, waiting < usize::from(width));
    assert!(!after.nodes[0].holds && !after.nodes[0].pool);
    kani::cover!(
        waiting == usize::from(width) && !changed,
        "W waiters block planning"
    );
    kani::cover!(
        waiting == usize::from(width - 1) && changed,
        "fewer than W waiters admit planning"
    );
    if changed {
        assert!(after.nodes[0].settle_rank() < before.nodes[0].settle_rank());
    }
}

fn check_gate_progress(width: u8) {
    let mut before = gate_world(width);
    // Every mixed gate world either has a witness or is waiting-only. The
    // separate waiting proof establishes that the latter cannot be settled.
    let Some(event) = before.witness() else {
        for n in &before.nodes {
            assert!(!n.paused && !n.holds && !n.pool && n.run.is_none());
            assert!(matches!(n.phase, Phase::Ready(_) | Phase::Unplanned));
        }
        kani::cover!(true, "without a witness every leaf is an unpaused waiter");
        return;
    };
    kani::cover!(
        before.nodes.iter().filter(|n| n.holds).count() == usize::from(width),
        "a full gate has a progress event"
    );
    kani::cover!(
        before.nodes.iter().filter(|n| n.pool).count() == usize::from(width),
        "all W leaves have admitted maker runs"
    );
    // A symbolic actor in apply duplicates large Node reads and exhausted
    // 64 GiB. These independent roots are interchangeable; keep the actor at 0.
    let (actor, event) = match event {
        Event::Reply {
            node,
            generation,
            reply,
        } => (
            node,
            Event::Reply {
                node: NodeId(0),
                generation,
                reply,
            },
        ),
        Event::Ack { node, seq, ack } => (
            node,
            Event::Ack {
                node: NodeId(0),
                seq,
                ack,
            },
        ),
        Event::Fact { node, fact } => (
            node,
            Event::Fact {
                node: NodeId(0),
                fact,
            },
        ),
        Event::Command { node, id, command } => (
            node,
            Event::Command {
                node: NodeId(0),
                id,
                command,
            },
        ),
        Event::NewRequest { .. } | Event::NewAdoption { .. } => unreachable!(),
    };
    let measure = before.measure();
    for i in 1..4 {
        if actor == NodeId(i as u64) {
            before.nodes.swap(0, i);
        }
    }
    require_consistent(&before);
    assert_eq!(before.measure(), measure);
    match event {
        Event::Reply {
            generation,
            reply: Reply::MakerDone { head },
            ..
        } => {
            let n = before.nodes[0];
            assert!(n.phase == Phase::Making && !n.paused);
            assert_eq!(n.run, Some(generation));
            assert_eq!(head, n.head.unwrap_or(HeadId(0)));
        }
        Event::Fact {
            fact:
                Fact::PrMerged {
                    head,
                    checks_passed,
                },
            ..
        } => {
            let n = before.nodes[0];
            assert!(n.phase == Phase::Queued && !n.paused && checks_passed);
            assert_eq!(head, n.head.unwrap_or(HeadId(0)));
        }
        Event::Command {
            id,
            command: Command::Resume,
            ..
        } => {
            assert!(before.nodes[0].paused);
            assert_eq!(id, before.nodes[0].last_cmd + 1);
        }
        _ => unreachable!("gate worlds have only maker, merge or resume witnesses"),
    }
}

fn check_gate_waiting_progress(width: u8) {
    let before = gate_world_with_waiters(width, None, true);
    assert!(before.witness().is_none());
    assert!(!before.is_settled());
    kani::cover!(
        before
            .nodes
            .iter()
            .all(|n| matches!(n.phase, Phase::Ready(_))),
        "a world with only ready leaves can advance"
    );
    kani::cover!(
        before.nodes.iter().all(|n| n.phase == Phase::Unplanned),
        "a world with only unplanned leaves can advance"
    );
}

// Together the witness and waiting proofs establish these preconditions for
// every settled gate world. Prove each application separately to keep the event
// payload and actor concrete, as in the general per-event proofs.
fn check_gate_maker_progress(width: u8) {
    let mut world = gate_world_in_phase(width, Some(Phase::Making));
    kani::assume(!world.nodes[0].paused);
    let generation = world.nodes[0].generation;
    let head = world.nodes[0].head.unwrap_or(HeadId(0));
    let before_measure = world.measure();
    let mut out = Outbox::new();
    // apply dispatches this fixed, valid node directly to on_reply.
    on_reply(
        &mut world,
        0,
        generation,
        Reply::MakerDone { head },
        &mut out,
    );
    require_consistent(&world);
    assert!(world.measure() < before_measure);
    kani::cover!(true, "maker completion lowers the gate world's measure");
}

fn check_gate_merge_progress(width: u8) {
    let mut world = gate_world_in_phase(width, Some(Phase::Queued));
    kani::assume(!world.nodes[0].paused);
    let before_measure = world.measure();
    let head = world.nodes[0].head.unwrap_or(HeadId(0));
    let mut out = Outbox::new();
    apply(
        &mut world,
        Event::Fact {
            node: NodeId(0),
            fact: Fact::PrMerged {
                head,
                checks_passed: true,
            },
        },
        &mut out,
    );
    require_consistent(&world);
    assert!(world.measure() < before_measure);
    kani::cover!(true, "a merge lowers the gate world's measure");
}

fn check_gate_resume_progress(width: u8) {
    let mut world = gate_world(width);
    kani::assume(world.nodes[0].paused);
    let before_measure = world.measure();
    let id = world.nodes[0].last_cmd + 1;
    let mut out = Outbox::new();
    apply(
        &mut world,
        Event::Command {
            node: NodeId(0),
            id,
            command: Command::Resume,
        },
        &mut out,
    );
    require_consistent(&world);
    assert!(world.measure() < before_measure);
    kani::cover!(true, "resuming lowers the gate world's measure");
}

proof!(gate_grant_w2, check_gate_grant(2));
proof!(gate_grant_w3, check_gate_grant(3));
proof!(gate_planning_w2, check_gate_planning(2));
proof!(gate_planning_w3, check_gate_planning(3));
proof!(gate_progress_w2, check_gate_progress(2));
proof!(gate_progress_w3, check_gate_progress(3));
proof!(gate_waiting_progress_w2, check_gate_waiting_progress(2));
proof!(gate_waiting_progress_w3, check_gate_waiting_progress(3));

proof!(gate_maker_progress_w2, check_gate_maker_progress(2));
proof!(gate_maker_progress_w3, check_gate_maker_progress(3));
proof!(gate_merge_progress_w2, check_gate_merge_progress(2));
proof!(gate_merge_progress_w3, check_gate_merge_progress(3));
proof!(gate_resume_progress_w2, check_gate_resume_progress(2));
proof!(gate_resume_progress_w3, check_gate_resume_progress(3));

proof!(apply_rebase_progress_gate, {
    let mut node = Node::new(RepoId(0));
    node.phase = Phase::Rebasing;
    node.run = Some(node.generation);
    node.pos = kani::any();
    kani::assume(node.pos < 2);
    let awaiting = node.current() == Some(Item::Await);
    kani::cover!(!awaiting, "the rebase has not reached Await");
    kani::cover!(awaiting, "the rebase is awaiting its reply");
    assert_eq!(rebase_progress_applies(&node, node.generation), awaiting);
});

fn check_publishing_ci(infra: bool) {
    let before = any_settled_world(Some(Phase::Publishing));
    let b = &before.nodes[0];
    kani::assume(b.current() == Some(Item::Do(EffectKind::Enqueue)));
    kani::assume(!b.waiting_incident);
    let mut after = before.clone();
    let mut out = Outbox::new();
    apply(&mut after, fact(Fact::CiFailed { infra }), &mut out);
    let n = &after.nodes[0];
    require_consistent(&after);
    check_nodes(&before, &after);
    check_outputs(&before, &after, &out, None);
    assert_eq!(after.nodes[1], before.nodes[1]);
    if let Some(effect) = b.effect {
        assert!(
            n.effect
                .is_some_and(|current| current.seq == effect.seq && current.cancelling)
        );
    }
    if infra {
        assert_eq!(n.credit, b.credit);
        assert!(n.waiting_incident || n.run_failures > b.run_failures);
        kani::cover!(
            b.run_failures == 0 && !n.waiting_incident,
            "first infrastructure failure retries"
        );
        kani::cover!(
            b.run_failures == 1 && n.waiting_incident,
            "second infrastructure failure raises an incident"
        );
    } else {
        assert_ne!(n.phase, Phase::Publishing);
        // Compare the credit ladder with the existing Queued rule.
        let mut queued = before.clone();
        queued.nodes[0].phase = Phase::Queued;
        queued.nodes[0].pos = 0;
        queued.nodes[0].effect = None;
        apply(
            &mut queued,
            fact(Fact::CiFailed { infra: false }),
            &mut Outbox::new(),
        );
        assert_eq!(n.credit, queued.nodes[0].credit);
        assert_eq!(n.round, queued.nodes[0].round);
        assert_eq!(n.attempt, queued.nodes[0].attempt);
        assert!(n.round != b.round || n.credit.spent > b.credit.spent || n.question.is_some());
        kani::cover!(b.round == 0 && n.round == 1, "a free fix round");
        kani::cover!(
            b.round == 1 && n.credit.spent > b.credit.spent,
            "a paid next attempt"
        );
        kani::cover!(
            b.round == 1 && !b.credit.can_spend(1) && n.phase == Phase::AwaitingRob,
            "exhausted credit asks Rob"
        );
    }
    kani::cover!(b.effect.is_some(), "an enqueue is cancelled");
}
proof!(apply_publishing_ci_tests, check_publishing_ci(false));
proof!(
    apply_publishing_ci_infrastructure,
    check_publishing_ci(true)
);

fn check_publishing_ci_earlier_step() {
    let before = any_settled_world(Some(Phase::Publishing));
    kani::assume(before.nodes[0].current() != Some(Item::Do(EffectKind::Enqueue)));
    let mut after = before.clone();
    apply(
        &mut after,
        fact(Fact::CiFailed { infra: kani::any() }),
        &mut Outbox::new(),
    );
    assert_eq!(after, before);
    kani::cover!(before.nodes[0].pos == 0, "a failure before push is ignored");
}
proof!(
    apply_publishing_ci_earlier_step,
    check_publishing_ci_earlier_step()
);
