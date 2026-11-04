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

fn small_head(head: Option<HeadId>) -> bool {
    head.is_none_or(|head| head.0 < 8)
}

fn small(n: &Node) -> bool {
    n.generation < LIMIT
        && n.next_seq < LIMIT
        && n.last_cmd < LIMIT
        && n.repo.0 < 2
        && small_head(n.proposed)
        && small_head(n.head)
        && small_head(n.checked)
        && small_head(n.approved)
        && small_head(n.merged)
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
    any_world_with_dependencies(phase, Vec::new())
}

fn any_world_with_dependencies(phase: Option<Phase>, dependencies: Vec<Dependency>) -> World {
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
        dependencies,
        fail_fast: kani::any(),
    };
    kani::assume(world.pool_capacity <= 2);
    kani::assume(world.gates[0].w <= 3 && world.gates[1].w <= 3);
    kani::assume(world.next_ticket < LIMIT && world.next_question < LIMIT);
    kani::assume(small(&world.nodes[0]) && small(&world.nodes[1]));
    // With an empty dependency table consistency already requires this new
    // flag to be false. State it early so the solver can prune the credit path.
    if world.dependencies.is_empty() {
        kani::assume(!world.nodes[0].credit.withheld && !world.nodes[1].credit.withheld);
    }
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
        Err(Broken::Dependencies) => panic!("broken: dependencies"),
        Err(Broken::ReviewStop) => panic!("broken: review stop"),
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
    if let Event::Reply {
        generation,
        reply: Reply::ProtectedPaths,
        ..
    } = event
    {
        let b = &before.nodes[0];
        let n = &after.nodes[0];
        if matches!(
            b.phase,
            Phase::Ingesting | Phase::Auditing | Phase::Planning { .. }
        ) && b.current() == Some(Item::Await)
            && b.run == Some(generation)
        {
            assert_eq!(n.phase, Phase::Stopping(Outcome::NotActionable));
            assert_eq!(n.credit, b.credit);
            assert!(n.question.is_none());
            assert_eq!(n.plan_retries, b.plan_retries);
        }
    }
    if let Event::NewAdoption { repo, head, .. } = &event {
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
    check_fail_fast(&before, &after, event);
    check_fail_fast_funding(&before, &after, event);
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
            if b.review_restarted && before.fail_fast {
                assert_eq!(n.phase, Phase::Stopping(Outcome::Stopped));
                assert_eq!(n.review_stop, Some(ReviewStop::ReviewerOutOfRoom));
                assert_eq!(n.credit, b.credit);
            } else if b.review_restarted {
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

/// Q54: under `fail_fast` no transition retries the work: none raises
/// `attempt` past the first leaf plan's 1, re-plans, or enters Planning other
/// than through Rob's answer, and the planner never re-runs on `cant_meet`.
/// A stop for TL3 review spends nothing and asks nothing. Without the flag,
/// only a TL3-created OpenPr body refusal stops for TL3 review (#629).
fn check_fail_fast(before: &World, after: &World, event: Event) {
    let mut i = 0;
    while i < before.nodes.len() {
        let b = &before.nodes[i];
        let n = &after.nodes[i];
        let stopped = n.review_stop != b.review_stop;
        let publication_body = matches!(
            event,
            Event::Ack {
                node,
                seq,
                ack: Ack::BodyPolicyRefused,
            } if node.index() == Some(i)
                && b.effect.is_some_and(|e| {
                    e.seq == seq && !e.cancelling && e.kind == EffectKind::OpenPr
                })
        );
        if publication_body {
            assert_eq!(n.review_stop, Some(ReviewStop::PublicationBody));
        }
        if !before.fail_fast {
            assert!(
                !stopped || publication_body,
                "without fail_fast only an OpenPr body refusal stops for TL3 review"
            );
        } else {
            assert!(
                n.attempt <= b.attempt.max(1),
                "fail_fast starts no new attempt"
            );
            assert!(b.replanned || !n.replanned, "fail_fast never re-plans");
            if matches!(n.phase, Phase::Planning { .. }) {
                assert!(matches!(
                    b.phase,
                    Phase::Planning { .. } | Phase::AwaitingRob
                ));
            }
            // A work failure never asks Rob whether to go on. Only the kernel's
            // rule-break path, which the planner worker can't reach, still does.
            // Rob's unclear answer re-asks his open question under a new id; that
            // clarification is the same question, not a new one.
            let clarifies = |p: Question, q: Question| {
                b.phase == Phase::AwaitingRob
                    && p.kind == q.kind
                    && p.cont == q.cont
                    && p.cost == q.cost
                    && !p.clarified
                    && q.clarified
            };
            let asked = n.question.is_some_and(|q| {
                q.kind == QKind::CantMeet
                    && b.question
                        .map_or(true, |p| p.id != q.id && !clarifies(p, q))
            });
            if asked {
                assert!(
                    matches!(
                        event,
                        Event::Reply {
                            reply: Reply::PlanLeaf { .. },
                            ..
                        }
                    ),
                    "fail_fast asks can't-meet only after a broken rule"
                );
            }
        }
        if stopped {
            assert!(b.review_stop.is_none());
            assert_eq!(n.phase, Phase::Stopping(Outcome::Stopped));
            assert_eq!(n.credit, b.credit);
            assert!(n.question.is_none());
        }
        i += 1;
    }
    if let Event::Reply {
        generation,
        reply: Reply::PlanCantMeet,
        ..
    } = event
    {
        let b = &before.nodes[0];
        let n = &after.nodes[0];
        if before.fail_fast
            && matches!(b.phase, Phase::Planning { .. })
            && b.current() == Some(Item::Await)
            && b.run == Some(generation)
        {
            assert_eq!(n.phase, Phase::Stopping(Outcome::Stopped));
            assert_eq!(n.review_stop, Some(ReviewStop::PlanCantMeet));
        }
    }
}

/// fail_fast stops retries, not credit decisions: a footprint extension that
/// fits but has no credit asks Rob for credit (#482).
fn check_fail_fast_funding(before: &World, after: &World, event: Event) {
    if let Event::Reply {
        generation,
        reply: Reply::MakerNeedsFiles { files, fits },
        ..
    } = event
    {
        let b = &before.nodes[0];
        let n = &after.nodes[0];
        if before.fail_fast
            && b.phase == Phase::Making
            && b.current() == Some(Item::Await)
            && b.run == Some(generation)
            && before.has_footprint(files)
            && fits
            && !files.is_empty()
            && !b.credit.can_spend(1)
        {
            assert_eq!(n.phase, Phase::AwaitingRob);
            assert!(n.question.is_some_and(
                |q| q.kind == QKind::OutOfCredit && q.cont == Cont::Ready(Resume::Making)
            ));
            assert!(n.review_stop.is_none());
        }
    }
}

/// Q54: the flag changes only a step that would retry the work, and that step
/// stops for TL3 review instead. So with the flag off, the event does exactly
/// what it did before the flag existed. The phase is the one that accepts the
/// event; elsewhere the event never reaches a step that reads the flag.
fn check_fail_fast_diverts(phase: Phase, event: Event) {
    let mut off = any_settled_world(Some(phase));
    off.fail_fast = false;
    let mut on = off.clone();
    on.fail_fast = true;
    let b = off.nodes[0];
    let mut out_off = Outbox::new();
    apply(&mut off, event, &mut out_off);
    let mut out_on = Outbox::new();
    apply(&mut on, event, &mut out_on);
    on.fail_fast = false;
    let diverted = on != off;
    kani::cover!(diverted, "fail_fast stops a retry for TL3 review");
    kani::cover!(!diverted, "with the flag on, the event does what it did");
    if diverted {
        let n = &on.nodes[0];
        let r = &off.nodes[0];
        assert_eq!(on.nodes[1], off.nodes[1]);
        assert_eq!(n.credit, b.credit);
        let funding = n
            .question
            .is_some_and(|q| q.kind == QKind::OutOfCredit && q.cont == Cont::Ready(Resume::Making));
        if funding {
            // An unfunded extension that fits waits for credit instead of a split.
            assert!(
                r.question.is_some_and(|q| q.kind == QKind::OutOfCredit
                    && q.cont == Cont::Planning { must_split: true })
            );
            assert!(n.review_stop.is_none());
        } else {
            assert_eq!(n.phase, Phase::Stopping(Outcome::Stopped));
            assert!(n.review_stop.is_some());
            assert!(out_on.is_empty());
        }
        let retried = r.attempt > b.attempt
            || matches!(r.phase, Phase::Planning { .. })
            || r.question
                .is_some_and(|q| matches!(q.kind, QKind::CantMeet | QKind::OutOfCredit));
        assert!(retried, "fail_fast diverts only a retry");
    }
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

// The original Unplanned implication, proved by exhaustive cases. A direct
// outside witness proves strict progress even before the other root settles.
// Otherwise a delivered root or its gate occupant must still have an internal
// transition, so the premise "the world is settled" is false.
fn check_unplanned_progress(world: World) {
    let n = &world.nodes[0];
    if n.paused || n.effect.is_some() || n.waiting_incident {
        kani::cover!(
            world.nodes[1].phase.is_closed(),
            "a settled Unplanned world exists"
        );
        check_unplanned_witness(world);
    } else {
        let mut after = world.clone();
        let mut out = Outbox::new();
        let admitted = advance(&mut after, 0, &mut out);
        if admitted {
            assert_eq!(after.nodes[0].phase, Phase::Planning { must_split: false });
            assert_unplanned_measure_decreases(&world, &after);
        }
        if !admitted {
            let gate_occupant =
                matches!(world.nodes[1].phase, Phase::Ready(_)) && world.nodes[1].ticket.is_some();
            assert!(gate_occupant);
            if gate_occupant {
                assert!(advance(&mut after, 1, &mut out), "a gate occupant can move");
            }
        }
        kani::cover!(admitted, "an unblocked root enters planning");
        kani::cover!(!admitted, "a gate occupant can move instead");
    }
}

// Construct one concrete event per branch before applying it. Equality with
// the actual witness proves that this is the original witness property, while
// avoiding mutation paths for every phase of the other request.
fn check_unplanned_witness(world: World) {
    let n = world.nodes[0];
    let node = NodeId(0);
    if n.paused {
        check_unplanned_event(
            world,
            Event::Command {
                node,
                id: n.last_cmd + 1,
                command: Command::Resume,
            },
        );
    } else if let Some(effect) = n.effect {
        check_unplanned_event(
            world,
            Event::Ack {
                node,
                seq: effect.seq,
                ack: Ack::Confirmed,
            },
        );
    } else if n.waiting_incident {
        check_unplanned_event(
            world,
            Event::Fact {
                node,
                fact: Fact::IncidentCleared { by_rob: true },
            },
        );
    } else {
        let dependency = *world
            .dependencies
            .iter()
            .find(|d| d.node == node && (d.rejected || d.status != DependencyStatus::Delivered))
            .expect("a blocked Unplanned root has an unresolved prerequisite");
        if dependency.rejected || dependency.status == DependencyStatus::Abandoned {
            check_unplanned_event(
                world,
                Event::Command {
                    node,
                    id: n.last_cmd + 1,
                    command: Command::ChangeRequest,
                },
            );
        } else {
            check_unplanned_event(
                world,
                Event::Fact {
                    node,
                    fact: Fact::Dependency {
                        target: dependency.target,
                        status: DependencyStatus::Delivered,
                    },
                },
            );
        }
    }
}

fn unplanned_measure(world: &World) -> u64 {
    assert!(world.dependencies.iter().all(|d| d.node == NodeId(0)));
    world.nodes[0].distance()
        + world
            .dependencies
            .iter()
            .filter(|d| {
                d.status != DependencyStatus::Delivered && !world.nodes[0].phase.is_closed()
            })
            .count() as u64
}

// The other root is an identical common summand in the whole-world measure.
// Comparing only the changed contribution avoids expanding its arbitrary phase
// through distance twice. This is the same strict whole-world inequality.
fn assert_unplanned_measure_decreases(before: &World, after: &World) {
    assert_eq!(before.nodes.len(), 2);
    assert_eq!(after.nodes.len(), 2);
    assert_eq!(after.nodes[1], before.nodes[1]);
    assert!(unplanned_measure(after) < unplanned_measure(before));
}

fn check_unplanned_event(world: World, event: Event) {
    let witness = world.witness();
    assert_eq!(witness, Some(event.clone()));
    kani::cover!(
        witness.is_some(),
        "an Unplanned root has an outside progress witness"
    );
    let before = unplanned_measure(&world);
    let other = world.nodes[1];
    let mut after = world;
    let mut out = Outbox::new();
    apply(&mut after, event, &mut out);
    assert_eq!(after.nodes[1], other);
    assert!(unplanned_measure(&after) < before);
}

// One or two prerequisites owned by root 0. Root identity is symmetric, as in
// the event proofs. Other node fields, counters, repos, and credit retain the
// original any_world bounds. Statuses, rejection, and external/root targets
// vary before consistency is assumed; no prerequisite facts are assumed away.
fn any_unplanned_dependencies<const N: usize>(delivered: bool) -> World {
    let mut dependencies = Vec::with_capacity(N);
    for index in 0..N {
        let request = if kani::any() {
            let target: u8 = kani::any();
            kani::assume(target < 2);
            Some(NodeId(u64::from(target)))
        } else {
            None
        };
        dependencies.push(Dependency {
            node: NodeId(0),
            target: DependencyTarget {
                kind: kani::any(),
                number: index as u64 + 1,
                request,
            },
            status: if delivered {
                DependencyStatus::Delivered
            } else {
                kani::any()
            },
            rejected: !delivered && kani::any(),
        });
    }
    if !delivered {
        kani::assume(
            dependencies
                .iter()
                .any(|d| d.rejected || d.status != DependencyStatus::Delivered),
        );
    }
    any_world_with_dependencies(Some(Phase::Unplanned), dependencies)
}

fn check_unplanned_waiting<const N: usize>() {
    let world = any_unplanned_dependencies::<N>(false);
    assert!(!world.dependencies_delivered(NodeId(0)));
    let mut after = world.clone();
    let mut out = Outbox::new();
    assert!(!advance(&mut after, 0, &mut out));
    assert!(after == world && out.is_empty());
    let n = &after.nodes[0];
    assert!(!n.holds && !n.pool && n.ticket.is_none() && n.run.is_none());
    assert_eq!(n.credit.balance(), 0);
    kani::cover!(
        world
            .dependencies
            .iter()
            .any(|d| d.status == DependencyStatus::Pending && !d.rejected),
        "pending prerequisites block admission"
    );
    kani::cover!(
        world
            .dependencies
            .iter()
            .any(|d| d.status == DependencyStatus::Abandoned),
        "abandoned prerequisites block admission"
    );
    kani::cover!(
        world.dependencies.iter().any(|d| d.rejected),
        "rejected cycles block admission"
    );
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

// Root prerequisites are the only slice-two feature admitted by #693. These
// fixtures add dependency worlds; the existing fixtures retain their original
// slice-one domains and assertions.
fn waiting_dependency_world() -> World {
    let mut world = World::new(&[1], 1);
    let mut node = Node::new(RepoId(0));
    node.phase = Phase::Unplanned;
    node.credit.ready = true;
    node.credit.start = 4;
    node.credit.withheld = true;
    node.waiting_prerequisite = Some(2761);
    world.nodes.push(node);
    world.dependencies.push(Dependency {
        node: NodeId(0),
        target: DependencyTarget {
            kind: PrerequisiteKind::Issue,
            number: 2761,
            request: None,
        },
        status: DependencyStatus::Pending,
        rejected: false,
    });
    world
}

fn check_dependency_delivery() {
    let mut world = waiting_dependency_world();
    let before = world.measure();
    let target = world.dependencies[0].target;
    let mut out = Outbox::new();
    apply(
        &mut world,
        fact(Fact::Dependency {
            target,
            status: DependencyStatus::Delivered,
        }),
        &mut out,
    );
    assert!(world.measure() < before);
    assert!(advance(&mut world, 0, &mut out));
    assert_eq!(world.nodes[0].phase, Phase::Planning { must_split: false });
    assert_eq!(world.nodes[0].credit.balance(), 4);
    require_consistent(&world);
    kani::cover!(world.nodes[0].planning_started, "delivery admits planning");
}

fn check_dependency_abandoned() {
    let mut world = waiting_dependency_world();
    let target = world.dependencies[0].target;
    let mut out = Outbox::new();
    apply(
        &mut world,
        fact(Fact::Dependency {
            target,
            status: DependencyStatus::Abandoned,
        }),
        &mut out,
    );
    assert!(matches!(
        out.get(0),
        Some(Out::Notice {
            notice: Notice::Note(NoteKind::DependencyAbandoned { .. }),
            ..
        })
    ));
    out = Outbox::new();
    apply(
        &mut world,
        fact(Fact::Dependency {
            target,
            status: kani::any(),
        }),
        &mut out,
    );
    assert!(out.is_empty());
    assert!(!advance(&mut world, 0, &mut out));
    assert_eq!(world.nodes[0].phase, Phase::Unplanned);
    assert_eq!(world.dependencies[0].status, DependencyStatus::Abandoned);
    require_consistent(&world);
    kani::cover!(
        world.nodes[0].waiting_prerequisite.is_some(),
        "abandonment keeps the root blocked"
    );
}

fn check_dependency_waiting() {
    let mut world = waiting_dependency_world();
    let mut out = Outbox::new();
    let blocked = !advance(&mut world, 0, &mut out);
    assert!(blocked);
    let node = &world.nodes[0];
    assert!(!node.holds && !node.pool && node.ticket.is_none() && node.run.is_none());
    assert!(node.footprint.is_empty());
    assert_eq!(node.credit.balance(), 0);
    assert!(out.is_empty());
    require_consistent(&world);
    kani::cover!(blocked, "an open prerequisite blocks planning");
}

fn check_dependency_cycles() {
    let mut world = World::new(&[3], 3);
    for _ in 0..3 {
        world.nodes.push(Node::new(RepoId(0)));
    }
    for index in 0..2 {
        let from: u8 = kani::any();
        let to: u8 = kani::any();
        kani::assume(from < 3 && to < 3);
        world.dependencies.push(Dependency {
            node: NodeId(u64::from(from)),
            target: DependencyTarget {
                kind: PrerequisiteKind::Issue,
                number: index + 1,
                request: Some(NodeId(u64::from(to))),
            },
            status: DependencyStatus::Pending,
            rejected: false,
        });
    }
    // The concrete roots and unique external numbers satisfy every other
    // invariant. Only the prior graph is symbolic in this graph-safety proof.
    kani::assume(world.dependency_graph_acyclic());
    // Roots are interchangeable. Fix the owner of the new edge, as the
    // existing event proofs do, while both prior edges remain arbitrary.
    let from: u8 = 0;
    let to: u8 = kani::any();
    kani::assume(to < 3);
    let mut out = Outbox::new();
    apply(
        &mut world,
        Event::Fact {
            node: NodeId(u64::from(from)),
            fact: Fact::Dependency {
                target: DependencyTarget {
                    kind: PrerequisiteKind::Issue,
                    number: 3,
                    request: Some(NodeId(u64::from(to))),
                },
                status: DependencyStatus::Pending,
            },
        },
        &mut out,
    );
    assert!(world.dependency_graph_acyclic());
    kani::cover!(
        world.dependencies.last().unwrap().rejected,
        "a cyclic declaration is rejected"
    );
    kani::cover!(
        !world.dependencies.last().unwrap().rejected,
        "an acyclic declaration is accepted"
    );
}

fn check_dependency_fact() {
    // Dependency facts do no head or counter arithmetic, so this domain needs
    // no small-counter assumptions. The root may be in any valid phase.
    let mut world = World::new(&[3], 1);
    world.nodes = kani::vec::exact_vec::<Node, 1>();
    kani::assume(world.consistent());
    let target = DependencyTarget {
        kind: PrerequisiteKind::Issue,
        number: 2761,
        request: None,
    };
    let previous_status: DependencyStatus = kani::any();
    world.dependencies.push(Dependency {
        node: NodeId(0),
        target,
        status: previous_status,
        rejected: false,
    });
    kani::assume(world.consistent());
    let phase = world.nodes[0].phase;
    let status: DependencyStatus = kani::any();
    let mut out = Outbox::new();
    apply(
        &mut world,
        fact(Fact::Dependency { target, status }),
        &mut out,
    );
    let expected = if previous_status == DependencyStatus::Pending {
        status
    } else {
        previous_status
    };
    assert_eq!(world.dependencies[0].status, expected);
    assert_eq!(world.nodes[0].phase, phase);
    require_consistent(&world);
    kani::cover!(
        phase.is_closed() && status == DependencyStatus::Delivered,
        "late facts update closed roots"
    );
    kani::cover!(
        phase == Phase::Ingesting && status == DependencyStatus::Abandoned,
        "facts update ingesting roots"
    );
    kani::cover!(
        phase == Phase::Making,
        "facts are accepted during maker work"
    );
    kani::cover!(
        phase == Phase::Queued,
        "facts are accepted in the merge queue"
    );
}

fn check_dependency_registration() {
    let mut world = World::new(&[3], 1);
    world.nodes = kani::vec::exact_vec::<Node, 1>();
    kani::assume(world.consistent());
    let phase = world.nodes[0].phase;
    let started = world.nodes[0].planning_started;
    apply(
        &mut world,
        fact(Fact::Dependency {
            target: DependencyTarget {
                kind: PrerequisiteKind::Issue,
                number: 2762,
                request: None,
            },
            status: kani::any(),
        }),
        &mut Outbox::new(),
    );
    if started || !matches!(phase, Phase::Ingesting | Phase::Auditing | Phase::Unplanned) {
        assert!(
            world.dependencies.is_empty(),
            "a started request never gains a prerequisite"
        );
    } else {
        assert_eq!(world.dependencies.len(), 1);
    }
    require_consistent(&world);
    kani::cover!(started, "a started request rejects a declaration");
    kani::cover!(
        !world.dependencies.is_empty(),
        "an unstarted root accepts a declaration"
    );
}

fn check_dependency_remove() {
    let mut world = waiting_dependency_world();
    world.nodes[0].phase = kani::any();
    let retained = Dependency {
        target: DependencyTarget {
            number: 2762,
            ..world.dependencies[0].target
        },
        status: if kani::any() {
            DependencyStatus::Abandoned
        } else {
            DependencyStatus::Delivered
        },
        ..world.dependencies[0]
    };
    world.dependencies.push(retained);
    kani::assume(world.consistent());
    let phase = world.nodes[0].phase;
    let target = world.dependencies[0].target;
    apply(
        &mut world,
        fact(Fact::DependencyRemove {
            kind: target.kind,
            number: target.number,
        }),
        &mut Outbox::new(),
    );
    assert_eq!(
        world.dependencies.len(),
        if phase == Phase::Ingesting { 1 } else { 2 }
    );
    assert_eq!(
        *world.dependencies.last().unwrap(),
        retained,
        "retained prerequisites keep their terminal outcome and identity"
    );
    require_consistent(&world);
    kani::cover!(
        world.dependencies.len() == 1,
        "re-ingest removes a changed declaration"
    );
    kani::cover!(
        world.dependencies.len() == 2,
        "other phases preserve declarations"
    );
    kani::cover!(
        retained.status == DependencyStatus::Abandoned,
        "abandoned never reverts during re-ingest"
    );
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
        proof!($name, 6, $body);
    };
    ($name:ident, $bound:literal, $body:expr) => {
        #[kani::proof]
        #[kani::unwind($bound)]
        fn $name() {
            $body;
        }
    };
}

proof!(apply_dependency, 3, check_dependency_fact());
proof!(
    apply_dependency_registration,
    3,
    check_dependency_registration()
);
proof!(apply_dependency_remove, 3, check_dependency_remove());
proof!(dependency_no_cycle, 4, check_dependency_cycles());
proof!(
    dependency_waiting_holds_nothing,
    2,
    check_dependency_waiting()
);
proof!(
    dependency_delivered_releases,
    2,
    check_dependency_delivery()
);
proof!(
    dependency_abandoned_never_releases,
    2,
    check_dependency_abandoned()
);

// Applying each event variant.
proof!(
    apply_new_request,
    check_apply(Event::NewRequest {
        repo: kani::any(),
        priority: kani::any()
    })
);
proof!(
    apply_ingest_done,
    check_apply(reply(Reply::IngestDone { size: kani::any() }))
);
proof!(
    apply_ingest_early_end,
    check_apply(reply(Reply::IngestEarlyEnd(kani::any())))
);
proof!(
    apply_protected_paths,
    check_apply(reply(Reply::ProtectedPaths))
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
proof!(apply_ack_body_policy_refused, {
    let event = ack(Ack::BodyPolicyRefused);
    let Event::Ack { seq, .. } = event else {
        unreachable!()
    };
    let before = check_apply(event);
    let b = &before.nodes[0];
    let open_pr = b
        .effect
        .is_some_and(|e| e.seq == seq && !e.cancelling && e.kind == EffectKind::OpenPr);
    kani::cover!(open_pr && before.fail_fast, "OpenPr refusal with fail_fast");
    kani::cover!(
        open_pr && !before.fail_fast,
        "OpenPr refusal without fail_fast"
    );
    kani::cover!(
        b.effect.is_some_and(|e| e.seq != seq),
        "a stale refusal is ignored"
    );
    kani::cover!(
        b.effect.is_some_and(|e| e.seq == seq && e.cancelling),
        "a cancelled effect only clears its delivery"
    );
    let mut refused = before.clone();
    let mut refused_out = Outbox::new();
    apply(&mut refused, event, &mut refused_out);
    if open_pr {
        assert!(
            refused_out.is_empty(),
            "publication refusal never asks Rob or raises an incident"
        );
    } else {
        let mut permanent = before.clone();
        let mut permanent_out = Outbox::new();
        apply(
            &mut permanent,
            Event::Ack {
                node: NodeId(0),
                seq,
                ack: Ack::Permanent,
            },
            &mut permanent_out,
        );
        assert_eq!(refused, permanent);
        assert_eq!(refused_out, permanent_out);
        kani::cover!(
            b.effect.is_some_and(|e| e.seq == seq && !e.cancelling),
            "other effect kinds retain permanent failure handling"
        );
    }
});
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
proof!(apply_priority, check_priority());
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
proof!(
    progress_from_unplanned,
    3,
    check_unplanned_progress(any_world(Some(Phase::Unplanned)))
);
proof!(
    progress_from_unplanned_delivered,
    3,
    check_unplanned_progress(any_unplanned_dependencies::<2>(true))
);
proof!(
    progress_from_unplanned_delivered_one,
    3,
    check_unplanned_progress(any_unplanned_dependencies::<1>(true))
);
proof!(
    progress_from_unplanned_waiting,
    3,
    check_unplanned_waiting::<2>()
);
proof!(
    progress_from_unplanned_waiting_one,
    3,
    check_unplanned_waiting::<1>()
);
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
        priority: kani::any(),
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
                    let runnable = slot_free(&before, j) && first_in_line(&before, j);
                    if runnable || !before.footprints_disjoint(other.footprint, n.footprint) {
                        assert!(
                            !after.nodes[0].holds,
                            "a runnable or overlapping older ticket prevents this grant"
                        );
                    }
                    if after.nodes[0].holds {
                        assert!(!runnable);
                        assert!(before.footprints_disjoint(other.footprint, n.footprint));
                    }
                }
            }
        }
    }
    kani::cover!(
        after.nodes[0].holds && older_blocked,
        "a truly disjoint younger leaf passes a file-blocked older waiter"
    );
    kani::cover!(
        n.ticket.is_some()
            && !n.paused
            && slot_free(&before, 0)
            && before.nodes.iter().enumerate().any(|(j, older)| {
                j != 0
                    && matches!(older.phase, Phase::Ready(_))
                    && older
                        .ticket
                        .is_some_and(|theirs| theirs < n.ticket.unwrap())
                    && !slot_free(&before, j)
                    && !before.footprints_disjoint(older.footprint, n.footprint)
            })
            && !after.nodes[0].holds,
        "a file-blocked older waiter prevents a younger leaf taking another needed file"
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

fn publishing_ci_transition(infra: bool) -> (World, World) {
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
    (before, after)
}

fn check_publishing_ci_tests() {
    let (before, after) = publishing_ci_transition(false);
    let b = &before.nodes[0];
    let n = &after.nodes[0];
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
    assert!(
        n.round != b.round
            || n.credit.spent > b.credit.spent
            || n.question.is_some()
            || n.review_stop.is_some()
    );
    kani::cover!(
        before.fail_fast
            && b.round == 1
            && b.review_stop.is_none()
            && n.review_stop == Some(ReviewStop::CiFailed),
        "fail_fast stops for TL3 review"
    );
    kani::cover!(b.round == 0 && n.round == 1, "a free fix round");
    kani::cover!(
        b.round == 1 && n.credit.spent > b.credit.spent,
        "a paid next attempt"
    );
    kani::cover!(
        b.round == 1 && !b.credit.can_spend(1) && n.phase == Phase::AwaitingRob,
        "exhausted credit asks Rob"
    );
    kani::cover!(b.effect.is_some(), "an enqueue is cancelled");
}
proof!(apply_publishing_ci_tests, check_publishing_ci_tests());

// Q54: one proof per event that can reach a retry, in the phase that accepts it.
proof!(
    fail_fast_plan_cant_meet,
    check_fail_fast_diverts(
        Phase::Planning {
            must_split: kani::any()
        },
        reply(Reply::PlanCantMeet)
    )
);
proof!(
    fail_fast_maker_needs_files,
    check_fail_fast_diverts(
        Phase::Making,
        reply(Reply::MakerNeedsFiles {
            files: kani::any(),
            fits: kani::any()
        })
    )
);
proof!(
    fail_fast_maker_cant_meet,
    check_fail_fast_diverts(Phase::Making, reply(Reply::MakerCantMeet))
);
proof!(
    fail_fast_maker_out_of_room,
    check_fail_fast_diverts(Phase::Making, reply(Reply::MakerOutOfRoom))
);
proof!(
    fail_fast_checks_done,
    check_fail_fast_diverts(
        Phase::Checking,
        reply(Reply::ChecksDone {
            report: kani::any(),
            pass: kani::any(),
            in_footprint: kani::any()
        })
    )
);
proof!(
    fail_fast_changes_requested,
    check_fail_fast_diverts(Phase::Reviewing, reply(Reply::ChangesRequested))
);
proof!(
    fail_fast_reviewer_out_of_room,
    check_fail_fast_diverts(Phase::Reviewing, reply(Reply::ReviewerOutOfRoom))
);
proof!(
    fail_fast_ci_failed_queued,
    check_fail_fast_diverts(Phase::Queued, fact(Fact::CiFailed { infra: kani::any() }))
);
proof!(
    fail_fast_ci_failed_publishing,
    check_fail_fast_diverts(
        Phase::Publishing,
        fact(Fact::CiFailed { infra: kani::any() })
    )
);

fn check_publishing_ci_infrastructure() {
    let (before, after) = publishing_ci_transition(true);
    let b = &before.nodes[0];
    let n = &after.nodes[0];
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
    kani::cover!(b.effect.is_some(), "an enqueue is cancelled");
}
proof!(
    apply_publishing_ci_infrastructure,
    check_publishing_ci_infrastructure()
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

// Four leaves, an unpaused oldest waiter, and its last conflicting holder.
// Younger disjoint holders may remain active when that blocker completes.
fn check_gate_leaf_progress(width: u8) {
    let mut w = gate_world_in_phase(width, Some(Phase::Ready(Resume::Making)));
    kani::assume(w.nodes.iter().all(|n| !n.paused));
    kani::assume(w.nodes[1].phase == Phase::Queued);
    kani::assume(!w.footprints_disjoint(w.nodes[0].footprint, w.nodes[1].footprint));
    kani::assume(w.nodes[0].ticket.is_some());
    let ticket = w.nodes[0].ticket.unwrap();
    for i in 2..4 {
        kani::assume(matches!(w.nodes[i].phase, Phase::Queued | Phase::Ready(_)));
        if w.nodes[i].holds {
            kani::assume(w.footprints_disjoint(w.nodes[0].footprint, w.nodes[i].footprint));
        } else {
            kani::assume(w.nodes[i].ticket.is_some_and(|theirs| theirs > ticket));
        }
    }
    let full = w.nodes.iter().filter(|n| n.holds).count() == usize::from(width);
    let chained_waiter = matches!(w.nodes[2].phase, Phase::Ready(_))
        && matches!(w.nodes[3].phase, Phase::Ready(_))
        && slot_free(&w, 2)
        && w.nodes[2].ticket < w.nodes[3].ticket
        && !w.footprints_disjoint(w.nodes[0].footprint, w.nodes[2].footprint)
        && w.footprints_disjoint(w.nodes[0].footprint, w.nodes[3].footprint)
        && w.footprints_disjoint(w.nodes[2].footprint, w.nodes[3].footprint);
    let mut passed = false;
    let mut passed_chain = false;
    for i in 2..4 {
        let held = w.nodes[i].holds;
        advance(&mut w, i, &mut Outbox::new());
        if !held && w.nodes[i].holds {
            passed = true;
            passed_chain |= i == 3 && chained_waiter;
            assert!(w.footprints_disjoint(w.nodes[0].footprint, w.nodes[i].footprint));
        }
    }
    assert_eq!(w.nodes[0].ticket, Some(ticket));
    // Successful teardown of the last conflicting holder, through real paths.
    on_command(&mut w, 1, 1, Command::Stop, &mut Outbox::new());
    for _ in 0..5 {
        advance(&mut w, 1, &mut Outbox::new());
        if let Some(effect) = w.nodes[1].effect {
            on_ack(&mut w, 1, effect.seq, Ack::Confirmed, &mut Outbox::new());
        }
    }
    advance(&mut w, 1, &mut Outbox::new());
    assert!(w.nodes[1].phase.is_closed());
    assert!(slot_free(&w, 0));
    // Visit younger leaves first: none can take the newly free slot or files.
    for i in 2..4 {
        let held = w.nodes[i].holds;
        advance(&mut w, i, &mut Outbox::new());
        assert_eq!(w.nodes[i].holds, held);
    }
    advance(&mut w, 0, &mut Outbox::new());
    assert!(w.nodes[0].holds && w.nodes[0].ticket.is_none());
    require_consistent(&w);
    kani::cover!(
        full && w.nodes[0].holds,
        "an oldest waiter progresses from a full gate when its last blocker completes"
    );
    kani::cover!(
        passed && w.nodes[0].holds,
        "an older leaf progresses while a disjoint overtaker remains active"
    );
    kani::cover!(
        passed_chain && w.nodes[0].holds,
        "passes a waiter blocked by an older overlapping waiter"
    );
}

proof!(gate_leaf_progress_w2, check_gate_leaf_progress(2));
proof!(gate_leaf_progress_w3, check_gate_leaf_progress(3));

// Q39: agent-pool grants go strictly by priority, then ticket.

fn check_priority() {
    let before = any_settled_world(None);
    let priority: Priority = kani::any();
    let mut after = before.clone();
    let mut out = Outbox::new();
    apply(&mut after, fact(Fact::Priority(priority)), &mut out);
    require_consistent(&after);
    check_nodes(&before, &after);
    check_outputs(&before, &after, &out, None);
    let n = before.nodes[0];
    if n.phase.is_closed() {
        assert_eq!(after.nodes[0], n, "Closed is final");
    } else {
        assert_eq!(
            after.nodes[0],
            Node { priority, ..n },
            "only the priority changes"
        );
    }
    assert_eq!(&after.nodes[1..], &before.nodes[1..]);
    assert!(out.is_empty());
    kani::cover!(
        !n.phase.is_closed() && n.priority != priority,
        "a priority changes"
    );
}

/// The state table's place in the pool line, written independently of the
/// kernel: the priority's level, then the ticket.
fn spec_key(priority: Priority, ticket: u64) -> (u64, u64) {
    let level = match priority {
        Priority::P0 => 0,
        Priority::P1 => 1,
        Priority::P2 => 2,
        Priority::P3 => 3,
        Priority::Unranked => 4,
    };
    (level, ticket)
}

/// Three leaves waiting for the agent pool at their first run, in arbitrary
/// priority and ticket order, with the pool free or full.
fn pool_world() -> World {
    let mut w = World::new(&[3], kani::any());
    kani::assume(w.pool_capacity <= 1);
    w.next_ticket = kani::any();
    kani::assume(w.next_ticket <= 64);
    for _ in 0..3 {
        let mut n = Node::new(RepoId(0));
        n.ticket = kani::any();
        n.priority = kani::any();
        w.nodes.push(n);
    }
    kani::assume(w.consistent());
    w
}

fn check_pool_grant_order() {
    let before = pool_world();
    let n = before.nodes[0];
    kani::assume(n.ticket.is_some());
    let mut after = before.clone();
    let mut out = Outbox::new();
    advance(&mut after, 0, &mut out);
    require_consistent(&after);
    let mine = spec_key(n.priority, n.ticket.unwrap());
    let first = before.nodes[1..]
        .iter()
        .all(|m| m.ticket.is_none_or(|t| mine < spec_key(m.priority, t)));
    if after.nodes[0].pool {
        assert!(
            first,
            "a grant goes to the best priority, then the older ticket"
        );
        assert!(after.nodes[0].ticket.is_none());
    } else if before.pool_capacity == 1 {
        assert!(!first, "the free pool goes to the first waiter in line");
    }
    kani::cover!(
        after.nodes[0].pool
            && before.nodes[1..]
                .iter()
                .any(|m| m.ticket.is_some_and(|t| t < mine.1)),
        "a better priority passes an older ticket"
    );
    kani::cover!(
        !after.nodes[0].pool && before.pool_capacity == 1,
        "a better waiter keeps its place"
    );
    assert_eq!(&after.nodes[1..], &before.nodes[1..]);
}

fn check_pool_order_is_strict_priority() {
    let mut a = Node::new(RepoId(0));
    let mut b = Node::new(RepoId(0));
    a.phase = Phase::Ingesting;
    b.phase = Phase::Ingesting;
    a.ticket = Some(kani::any());
    b.ticket = Some(kani::any());
    a.priority = kani::any();
    b.priority = kani::any();
    kani::assume(a.ticket != b.ticket);
    let (ka, kb) = (pool_key(&a).unwrap(), pool_key(&b).unwrap());
    // Priority first, whatever the tickets; the older ticket breaks a tie.
    let expected = a.priority < b.priority || (a.priority == b.priority && a.ticket < b.ticket);
    assert_eq!(
        ka < kb,
        expected,
        "the pool line is strict priority, then ticket"
    );
    kani::cover!(
        a.priority < b.priority && a.ticket > b.ticket,
        "a better priority passes an older ticket"
    );
    kani::cover!(a.priority == b.priority, "a tie goes by ticket");
}

proof!(pool_grant_is_priority_ordered, check_pool_grant_order());
proof!(
    pool_order_is_strict_priority,
    check_pool_order_is_strict_priority()
);
