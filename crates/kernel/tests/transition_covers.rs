//! K4c: executable covers retained when the Kani SPECIAL harnesses retire.
//! Each test is a concrete reachable scenario through the real `step`; the
//! arbitrary-world guarantees are Verus lemmas (verus/special_safety.rs and
//! the scheduler query contracts). Adoption covers live in tests/scenarios.rs.

use tl3_kernel::*;

const CHECKS_PASS: Reply = Reply::ChecksDone {
    report: true,
    pass: true,
    in_footprint: true,
};

struct Run {
    w: World,
    head: u64,
}

impl Run {
    fn new(fail_fast: bool) -> Run {
        let mut w = World::new(&[1], 1);
        w.fail_fast = fail_fast;
        let mut run = Run { w, head: 0 };
        run.send(Event::NewRequest {
            repo: RepoId(0),
            priority: Priority::Unranked,
        });
        run.confirm();
        run
    }

    fn send(&mut self, event: Event) -> Vec<Out> {
        let StepResult {
            world, outputs, ..
        } = step(self.w.clone(), event);
        assert!(world.valid(), "invalid world after {event:?}:\n{world:#?}");
        self.w = world;
        outputs
    }

    fn node(&self) -> Node {
        self.w.nodes[0]
    }

    fn confirm_one(&mut self) {
        let e = self.node().effect.expect("an effect in flight");
        self.send(Event::Ack {
            node: NodeId(0),
            seq: e.seq,
            ack: if e.kind == EffectKind::Enqueue {
                Ack::DeliveryAccepted
            } else {
                Ack::Confirmed
            },
        });
    }

    fn confirm(&mut self) {
        for _ in 0..64 {
            if self.node().effect.is_none() {
                return;
            }
            self.confirm_one();
        }
        panic!("effects never stopped");
    }

    fn reply_unconfirmed(&mut self, reply: Reply) {
        self.confirm();
        let generation = self.node().generation;
        self.send(Event::Reply {
            node: NodeId(0),
            generation,
            reply,
        });
    }

    fn reply(&mut self, reply: Reply) {
        self.reply_unconfirmed(reply);
        self.confirm();
    }

    /// Approved, with the first publication effect (Push) still in flight.
    fn publishing(fail_fast: bool) -> Run {
        let mut run = Run::new(fail_fast);
        run.reply(Reply::IngestDone { size: 1 });
        run.reply(Reply::AuditAgrees);
        let footprint = run.w.intern_footprint([FileId(0)]);
        run.reply(Reply::PlanLeaf {
            footprint,
            fits: true,
        });
        run.head += 1;
        run.reply(Reply::MakerDone {
            head: HeadId(run.head),
        });
        run.reply(CHECKS_PASS);
        run.reply_unconfirmed(Reply::Approved {
            head: HeadId(run.head),
        });
        assert_eq!(run.node().phase, Phase::Publishing);
        run
    }
}

fn ack(w: &World, seq: u64, ack: Ack) -> StepResult {
    step(
        w.clone(),
        Event::Ack {
            node: NodeId(0),
            seq,
            ack,
        },
    )
}

/// Kani `apply_ack_body_policy_refused`: outside a live OpenPr effect, a body
/// policy refusal is exactly a permanent failure, in world and outputs.
fn assert_same_as_permanent(w: &World, seq: u64) {
    let refused = ack(w, seq, Ack::BodyPolicyRefused);
    let permanent = ack(w, seq, Ack::Permanent);
    assert_eq!(refused.world, permanent.world);
    assert_eq!(refused.outputs, permanent.outputs);
}

#[test]
fn body_policy_refusal_of_another_effect_is_a_permanent_failure() {
    for fail_fast in [false, true] {
        let run = Run::publishing(fail_fast);
        let e = run.node().effect.expect("Push in flight");
        assert!(e.kind != EffectKind::OpenPr && !e.cancelling);
        assert_same_as_permanent(&run.w, e.seq);
    }
}

#[test]
fn a_stale_body_policy_refusal_is_ignored_like_a_permanent_one() {
    let run = Run::publishing(false);
    let e = run.node().effect.expect("Push in flight");
    let stale = e.seq + 1;
    assert_same_as_permanent(&run.w, stale);
    assert_eq!(ack(&run.w, stale, Ack::BodyPolicyRefused).world, run.w);
}

#[test]
fn a_cancelled_effect_refusal_only_clears_its_delivery() {
    let mut run = Run::publishing(false);
    let cmd = run.node().last_cmd + 1;
    run.send(Event::Command {
        node: NodeId(0),
        id: cmd,
        command: Command::Stop,
    });
    let e = run.node().effect.expect("the in-flight effect survives Stop");
    assert!(e.cancelling, "Stop cancels the in-flight effect");
    assert_same_as_permanent(&run.w, e.seq);
}

#[test]
fn an_open_pr_body_refusal_stops_for_review_without_asking() {
    for fail_fast in [false, true] {
        let mut run = Run::publishing(fail_fast);
        while run.node().effect.expect("publication effect").kind != EffectKind::OpenPr {
            run.confirm_one();
        }
        let e = run.node().effect.unwrap();
        assert!(!e.cancelling);
        let refused = ack(&run.w, e.seq, Ack::BodyPolicyRefused);
        let n = refused.world.nodes[0];
        assert_eq!(n.review_stop, Some(ReviewStop::PublicationBody));
        assert_eq!(n.credit, run.node().credit);
        assert!(n.question.is_none());
        assert!(!refused.outputs.iter().any(|o| matches!(
            o,
            Out::Notice {
                notice: Notice::Ask { .. } | Notice::Incident { .. },
                ..
            }
        )));
        assert_ne!(refused.world, ack(&run.w, e.seq, Ack::Permanent).world);
    }
}

/// Kani `pool_order_is_strict_priority`, exhaustively over every priority
/// pair and every ticket order: the pool line is priority first, then ticket.
#[test]
fn the_pool_key_is_strict_priority_then_ticket() {
    use Priority::*;
    let all = [P0, P1, P2, P3, Unranked];
    let level = |p: Priority| all.iter().position(|q| *q == p).unwrap();
    let (mut passes_older, mut tie) = (false, false);
    for pa in all {
        for pb in all {
            for (ta, tb) in [(1, 2), (2, 1), (5, 9), (9, 5)] {
                let mut a = Node::new(RepoId(0));
                let mut b = Node::new(RepoId(0));
                a.phase = Phase::Ingesting;
                b.phase = Phase::Ingesting;
                (a.ticket, b.ticket) = (Some(ta), Some(tb));
                (a.priority, b.priority) = (pa, pb);
                let (ka, kb) = (pool_key(&a).unwrap(), pool_key(&b).unwrap());
                let expected = level(pa) < level(pb) || (pa == pb && ta < tb);
                assert_eq!(ka < kb, expected, "{pa:?}/{ta} vs {pb:?}/{tb}");
                passes_older |= level(pa) < level(pb) && ta > tb;
                tie |= pa == pb;
            }
        }
    }
    assert!(passes_older && tie);
}

/// Several requests in one repository with gate width `width`, driven through
/// the real `step` like the Kani gate fixtures' leaves.
struct Gate {
    w: World,
}

impl Gate {
    fn new(width: u8) -> Gate {
        Gate {
            w: World::new(&[width], 8),
        }
    }

    fn send(&mut self, event: Event) {
        let StepResult { world, .. } = step(self.w.clone(), event);
        assert!(world.valid(), "invalid world after {event:?}:\n{world:#?}");
        self.w = world;
    }

    fn node(&self, i: usize) -> Node {
        self.w.nodes[i]
    }

    fn confirm(&mut self, i: usize) {
        for _ in 0..64 {
            let Some(e) = self.node(i).effect else { return };
            self.send(Event::Ack {
                node: NodeId(i as u64),
                seq: e.seq,
                ack: if e.kind == EffectKind::Enqueue {
                    Ack::DeliveryAccepted
                } else {
                    Ack::Confirmed
                },
            });
        }
        panic!("effects never stopped");
    }

    fn reply(&mut self, i: usize, reply: Reply) {
        self.confirm(i);
        let generation = self.node(i).generation;
        self.send(Event::Reply {
            node: NodeId(i as u64),
            generation,
            reply,
        });
        self.confirm(i);
    }

    /// A new request, ingested and audited, still unplanned.
    fn request(&mut self) -> usize {
        self.send(Event::NewRequest {
            repo: RepoId(0),
            priority: Priority::Unranked,
        });
        let i = self.w.nodes.len() - 1;
        self.confirm(i);
        self.reply(i, Reply::IngestDone { size: 1 });
        i
    }

    /// Audit and plan a leaf over `files`; it then holds a slot or waits.
    fn plan(&mut self, i: usize, files: &[u64]) {
        self.reply(i, Reply::AuditAgrees);
        assert!(matches!(self.node(i).phase, Phase::Planning { .. }));
        let footprint = self.w.intern_footprint(files.iter().map(|f| FileId(*f)));
        self.reply(
            i,
            Reply::PlanLeaf {
                footprint,
                fits: true,
            },
        );
    }

    fn holds(&self, i: usize) -> bool {
        self.node(i).holds
    }

    fn waiting(&self, i: usize) -> bool {
        matches!(self.node(i).phase, Phase::Ready(_)) && self.node(i).ticket.is_some()
    }

    /// Rob stops request i; its teardown runs to Closed through real acks.
    fn stop(&mut self, i: usize) {
        let id = self.node(i).last_cmd + 1;
        self.send(Event::Command {
            node: NodeId(i as u64),
            id,
            command: Command::Stop,
        });
        self.confirm(i);
        assert!(matches!(self.node(i).phase, Phase::Closed(_)));
    }
}

/// Kani `gate_leaf_progress_*` cover "an oldest waiter progresses from a full
/// gate when its last blocker completes". The younger overlapping waiter has
/// the lower index, so settle visits it first, and it still cannot take the
/// freed slot.
#[test]
fn an_oldest_waiter_takes_the_slot_its_last_blocker_frees() {
    let mut g = Gate::new(2);
    let a = g.request();
    g.plan(a, &[0]);
    let b = g.request();
    g.plan(b, &[1]);
    assert!(g.holds(a) && g.holds(b), "the W=2 gate is full");
    let younger = g.request();
    let oldest = g.request();
    g.plan(oldest, &[0]);
    g.plan(younger, &[0]);
    assert!(g.waiting(oldest) && g.waiting(younger));
    assert!(g.node(oldest).ticket < g.node(younger).ticket);
    let ticket = g.node(younger).ticket;
    g.stop(a);
    assert!(g.holds(oldest) && g.node(oldest).ticket.is_none());
    assert!(!g.holds(younger) && g.node(younger).ticket == ticket);
    assert!(g.holds(b));
}

/// Kani cover "an older leaf progresses while a disjoint overtaker remains
/// active": a truly disjoint younger leaf passes a file-blocked older waiter,
/// and the older one still gets the slot when its blocker completes.
#[test]
fn a_disjoint_overtaker_does_not_starve_the_older_waiter() {
    let mut g = Gate::new(2);
    let a = g.request();
    g.plan(a, &[0]);
    let older = g.request();
    let younger = g.request();
    g.plan(older, &[0]);
    assert!(g.waiting(older), "an overlapping holder blocks the older leaf");
    g.plan(younger, &[1]);
    assert!(g.holds(younger), "the disjoint younger leaf passes");
    assert!(g.waiting(older));
    g.stop(a);
    assert!(g.holds(older) && g.holds(younger));
}

/// Kani cover "passes a waiter blocked by an older overlapping waiter": a
/// younger leaf disjoint from both waiters passes one that is itself held back
/// by the oldest waiter, and the oldest still progresses afterwards.
#[test]
fn a_leaf_passes_a_waiter_held_back_by_an_older_overlapping_waiter() {
    let mut g = Gate::new(3);
    let a = g.request();
    g.plan(a, &[0]);
    let oldest = g.request();
    let middle = g.request();
    let youngest = g.request();
    g.plan(oldest, &[0, 1]);
    g.plan(middle, &[1]);
    assert!(g.waiting(oldest), "the holder's file blocks the oldest");
    assert!(g.waiting(middle), "the older overlapping waiter holds back the middle leaf");
    g.plan(youngest, &[2]);
    assert!(g.holds(youngest), "a leaf disjoint from both waiters passes");
    g.stop(a);
    assert!(g.holds(oldest));
    assert!(g.waiting(middle), "the middle leaf still overlaps the new holder");
    assert!(g.holds(youngest));
}

/// A Ready slot waiter is not in the pool line.
#[test]
fn a_slot_waiter_has_no_pool_key() {
    let mut n = Node::new(RepoId(0));
    n.phase = Phase::Ready(Resume::Making);
    n.ticket = Some(3);
    assert_eq!(pool_key(&n), None);
}

// ---- K5 G17 first cut: Kani covers as concrete executable witnesses ----
// Each test reaches the covered state through the real `step` (worlds are
// checked with `World::valid()`, as Kani's `any_settled_world` domain is any
// consistent settled world), asserts the cover predicate exactly as
// crates/kernel/src/proofs.rs states it at aa20aecc, and asserts the harness's
// outcome assertion for that case.

/// `Credit::can_spend(1)`: not withheld and one unit of balance left.
fn can_spend1(n: &Node) -> bool {
    !n.credit.withheld
        && u128::from(n.credit.start) + u128::from(n.credit.earned) + u128::from(n.credit.added)
            >= u128::from(n.credit.spent) + 1
}

/// proofs.rs:357 rebase fuel: credit balance plus the free recheck allowance left.
fn rebase_fuel(n: &Node) -> i128 {
    let balance = if n.credit.withheld {
        0
    } else {
        i128::from(n.credit.start) + i128::from(n.credit.earned) + i128::from(n.credit.added)
            - i128::from(n.credit.spent)
    };
    balance + i128::from(FREE_RECHECKS.saturating_sub(n.rechecks))
}

impl Run {
    /// Reviewing, awaiting the reviewer's reply.
    fn reviewing(fail_fast: bool) -> Run {
        let mut run = Run::new(fail_fast);
        run.reply(Reply::IngestDone { size: 1 });
        run.reply(Reply::AuditAgrees);
        let footprint = run.w.intern_footprint([FileId(0)]);
        run.reply(Reply::PlanLeaf {
            footprint,
            fits: true,
        });
        run.made_to_reviewing();
        run
    }

    fn made_to_reviewing(&mut self) {
        self.head += 1;
        self.reply(Reply::MakerDone {
            head: HeadId(self.head),
        });
        self.reply(CHECKS_PASS);
        assert_eq!(self.node().phase, Phase::Reviewing);
        assert!(self.awaiting());
    }

    /// The node waits for the reply of its current run (Item::Await).
    fn awaiting(&self) -> bool {
        let n = self.node();
        n.effect.is_none() && n.run == Some(n.generation)
    }

    /// Adjust the node inside the consistent settled domain.
    fn tune(&mut self, f: impl FnOnce(&mut Node)) {
        f(&mut self.w.nodes[0]);
        assert!(self.w.valid(), "tuned world must stay consistent and settled");
    }

    fn reply_now(&self, reply: Reply) -> Event {
        Event::Reply {
            node: NodeId(0),
            generation: self.node().generation,
            reply,
        }
    }

    /// Approved, delivered and waiting (AwaitingDelivery).
    fn delivered(fail_fast: bool) -> Run {
        let mut run = Run::reviewing(fail_fast);
        run.reply(Reply::Approved {
            head: HeadId(run.head),
        });
        assert_eq!(run.node().phase, Phase::AwaitingDelivery);
        run
    }

    /// Rebasing, awaiting the rebase reply, reached through a real Conflict.
    fn rebasing() -> Run {
        let mut run = Run::delivered(false);
        run.send(Event::Fact {
            node: NodeId(0),
            fact: Fact::Conflict,
        });
        run.confirm();
        assert_eq!(run.node().phase, Phase::Rebasing);
        assert!(run.awaiting());
        run
    }

    /// Publishing with the delivery handoff (Do(Enqueue)) in flight, in round 0
    /// or, after a round-0 CI failure in waiting, in round 1.
    fn at_enqueue(round_one: bool) -> Run {
        let mut run = if round_one {
            let mut run = Run::delivered(false);
            run.send(Event::Fact {
                node: NodeId(0),
                fact: Fact::CiFailed { infra: false },
            });
            run.confirm();
            assert_eq!((run.node().phase, run.node().round), (Phase::Making, 1));
            run.made_to_reviewing();
            run
        } else {
            Run::reviewing(false)
        };
        run.reply_unconfirmed(Reply::Approved {
            head: HeadId(run.head),
        });
        for _ in 0..8 {
            let e = run.node().effect.expect("a publication effect");
            if e.kind == EffectKind::Enqueue {
                assert_eq!(run.node().phase, Phase::Publishing);
                assert!(!run.node().waiting_incident);
                return run;
            }
            run.confirm_one();
        }
        panic!("never reached the delivery handoff");
    }
}

fn broke(n: &mut Node) {
    n.credit.spent = n.credit.start + n.credit.earned + n.credit.added;
}

// apply_reviewer_out_of_room covers, proofs.rs:937-953. `accepted` is Reviewing
// at Await for this run (proofs.rs:933-935); `paid` is rechecks+1 > FREE_RECHECKS.

#[test]
fn reviewer_out_of_room_first_exhaustion_free_review() {
    // proofs.rs:937 cover!(accepted && !b.review_restarted && !paid, "first exhaustion: free review")
    let mut run = Run::reviewing(false);
    let b = run.node();
    assert!(!b.review_restarted && b.rechecks.saturating_add(1) <= FREE_RECHECKS);
    run.send(run.reply_now(Reply::ReviewerOutOfRoom));
    let n = run.node();
    assert!(n.review_restarted && n.rechecks == b.rechecks + 1);
    assert_eq!(n.phase, Phase::Reviewing);
    assert!(n.generation > b.generation);
    assert_eq!(n.credit.spent, b.credit.spent);
}

#[test]
fn reviewer_out_of_room_first_exhaustion_paid_review() {
    // proofs.rs:941 cover!(accepted && !b.review_restarted && paid && b.credit.can_spend(1), "first exhaustion: paid review")
    let mut run = Run::reviewing(false);
    run.tune(|n| n.rechecks = FREE_RECHECKS);
    let b = run.node();
    assert!(!b.review_restarted && b.rechecks.saturating_add(1) > FREE_RECHECKS && can_spend1(&b));
    run.send(run.reply_now(Reply::ReviewerOutOfRoom));
    let n = run.node();
    assert_eq!(n.phase, Phase::Reviewing);
    assert_eq!(n.credit.spent, b.credit.spent + 1);
}

#[test]
fn reviewer_out_of_room_first_exhaustion_needs_credit() {
    // proofs.rs:945 cover!(accepted && !b.review_restarted && paid && !b.credit.can_spend(1), "first exhaustion: needs credit")
    let mut run = Run::reviewing(false);
    run.tune(|n| {
        n.rechecks = FREE_RECHECKS;
        broke(n);
    });
    let b = run.node();
    assert!(!b.review_restarted && b.rechecks.saturating_add(1) > FREE_RECHECKS && !can_spend1(&b));
    run.send(run.reply_now(Reply::ReviewerOutOfRoom));
    let n = run.node();
    assert_eq!(n.phase, Phase::AwaitingRob);
    assert_eq!(n.question.unwrap().cont, Cont::Ready(Resume::Reviewing));
}

#[test]
fn reviewer_out_of_room_second_exhaustion_paid_split() {
    // proofs.rs:949 cover!(accepted && b.review_restarted && b.credit.can_spend(1), "second exhaustion: paid split")
    let mut run = Run::reviewing(false);
    run.reply(Reply::ReviewerOutOfRoom);
    assert_eq!(run.node().phase, Phase::Reviewing);
    assert!(run.awaiting());
    let b = run.node();
    assert!(b.review_restarted && can_spend1(&b));
    run.send(run.reply_now(Reply::ReviewerOutOfRoom));
    let n = run.node();
    assert_eq!(n.phase, Phase::Planning { must_split: true });
    assert_eq!(n.credit.spent, b.credit.spent + 1);
    assert!(!n.holds);
}

#[test]
fn reviewer_out_of_room_second_exhaustion_split_needs_credit() {
    // proofs.rs:953 cover!(accepted && b.review_restarted && !b.credit.can_spend(1), "second exhaustion: split needs credit")
    let mut run = Run::reviewing(false);
    run.reply(Reply::ReviewerOutOfRoom);
    assert!(run.awaiting());
    run.tune(broke);
    let b = run.node();
    assert!(b.review_restarted && !can_spend1(&b));
    run.send(run.reply_now(Reply::ReviewerOutOfRoom));
    let n = run.node();
    assert_eq!(n.phase, Phase::AwaitingRob);
    assert_eq!(n.question.unwrap().cont, Cont::Planning { must_split: true });
}

// apply_rebase_clean / apply_rebase_needs_changes covers, proofs.rs:972-994.
// `accepted` is rebase_progress_applies (Rebasing at Await for this run);
// `free` is rechecks+1 <= FREE_RECHECKS; `funded` is can_spend(1). Every case
// keeps proofs.rs:360: AwaitingRob or strictly less rebase fuel.

fn rebase_case(reply: Reply, rechecks: u8, funded: bool) -> (Node, Node) {
    let mut run = Run::rebasing();
    run.tune(|n| {
        n.rechecks = rechecks;
        if !funded {
            broke(n);
        }
    });
    let b = run.node();
    assert!(run.awaiting() && b.phase == Phase::Rebasing);
    assert_eq!(can_spend1(&b), funded);
    run.send(run.reply_now(reply));
    let n = run.node();
    assert!(n.phase == Phase::AwaitingRob || rebase_fuel(&n) < rebase_fuel(&b));
    (b, n)
}

#[test]
fn clean_rebase_uses_the_free_allowance() {
    // proofs.rs:972 cover!(accepted && free, "clean rebase uses the free allowance")
    let (b, _) = rebase_case(Reply::RebaseClean { head: HeadId(9) }, 0, true);
    assert!(b.rechecks.saturating_add(1) <= FREE_RECHECKS);
}

#[test]
fn clean_rebase_spends_credit() {
    // proofs.rs:973 cover!(accepted && !free && funded, "clean rebase spends credit")
    let (b, _) = rebase_case(Reply::RebaseClean { head: HeadId(9) }, FREE_RECHECKS, true);
    assert!(b.rechecks.saturating_add(1) > FREE_RECHECKS && can_spend1(&b));
}

#[test]
fn clean_rebase_needs_credit() {
    // proofs.rs:974 cover!(accepted && !free && !funded, "clean rebase needs credit")
    let (b, _) = rebase_case(Reply::RebaseClean { head: HeadId(9) }, FREE_RECHECKS, false);
    assert!(b.rechecks.saturating_add(1) > FREE_RECHECKS && !can_spend1(&b));
}

#[test]
fn conflict_fix_pays_despite_free_allowance() {
    // proofs.rs:986 cover!(accepted && free && funded, "conflict fix pays despite free allowance")
    let (b, _) = rebase_case(Reply::RebaseNeedsChanges, 0, true);
    assert!(b.rechecks.saturating_add(1) <= FREE_RECHECKS && can_spend1(&b));
}

#[test]
fn conflict_fix_spends_credit_after_allowance() {
    // proofs.rs:990 cover!(accepted && !free && funded, "conflict fix spends credit after allowance")
    let (b, _) = rebase_case(Reply::RebaseNeedsChanges, FREE_RECHECKS, true);
    assert!(b.rechecks.saturating_add(1) > FREE_RECHECKS && can_spend1(&b));
}

#[test]
fn conflict_fix_needs_credit() {
    // proofs.rs:994 cover!(accepted && !funded, "conflict fix needs credit")
    let (b, _) = rebase_case(Reply::RebaseNeedsChanges, 0, false);
    assert!(!can_spend1(&b));
}

// check_publishing_ci_tests / check_publishing_ci_earlier_step covers,
// proofs.rs:1459-1468 and :1545. The domain is Publishing at Do(Enqueue), not
// waiting on an incident, with a non-infrastructure CI failure (proofs.rs:1391-1399).

fn ci_failed() -> Event {
    Event::Fact {
        node: NodeId(0),
        fact: Fact::CiFailed { infra: false },
    }
}

#[test]
fn ci_failure_takes_a_free_fix_round() {
    // proofs.rs:1459 cover!(b.round == 0 && n.round == 1, "a free fix round")
    let mut run = Run::at_enqueue(false);
    let b = run.node();
    run.send(ci_failed());
    let n = run.node();
    assert!(b.round == 0 && n.round == 1);
    assert_eq!((n.attempt, n.credit), (b.attempt, b.credit));
    assert_ne!(n.phase, Phase::Publishing);
}

#[test]
fn ci_failure_in_round_one_pays_the_next_attempt() {
    // proofs.rs:1460 cover!(b.round == 1 && n.credit.spent > b.credit.spent, "a paid next attempt")
    let mut run = Run::at_enqueue(true);
    let b = run.node();
    assert!(can_spend1(&b));
    run.send(ci_failed());
    let n = run.node();
    assert!(b.round == 1 && n.credit.spent > b.credit.spent);
}

#[test]
fn ci_failure_with_exhausted_credit_asks_rob() {
    // proofs.rs:1464 cover!(b.round == 1 && !b.credit.can_spend(1) && n.phase == Phase::AwaitingRob, "exhausted credit asks Rob")
    let mut run = Run::at_enqueue(true);
    run.tune(broke);
    let b = run.node();
    run.send(ci_failed());
    let n = run.node();
    assert!(b.round == 1 && !can_spend1(&b) && n.phase == Phase::AwaitingRob);
}

#[test]
fn ci_failure_cancels_the_enqueue_in_flight() {
    // proofs.rs:1468 cover!(b.effect.is_some(), "an enqueue is cancelled"), with
    // proofs.rs:1408-1412: the in-flight effect keeps its key and is cancelling.
    let mut run = Run::at_enqueue(false);
    let b = run.node();
    let e = b.effect.expect("the enqueue in flight");
    run.send(ci_failed());
    let n = run.node();
    assert!(n.effect.is_some_and(|c| c.seq == e.seq && c.cancelling));
}

#[test]
fn ci_failure_before_the_push_is_ignored() {
    // proofs.rs:1545 cover!(before.nodes[0].pos == 0, "a failure before push is ignored"),
    // with proofs.rs:1544: the whole world is unchanged.
    let run = Run::publishing(false);
    assert_eq!(run.node().pos, 0);
    for infra in [false, true] {
        let after = step(
            run.w.clone(),
            Event::Fact {
                node: NodeId(0),
                fact: Fact::CiFailed { infra },
            },
        );
        assert_eq!(after.world, run.w);
    }
}

// check_pool_grant_order covers, proofs.rs:1680-1687: pool capacity 1, waiters
// at their first run in arbitrary priority and ticket order.

fn pool_request(g: &mut Gate, priority: Priority) -> usize {
    g.send(Event::NewRequest {
        repo: RepoId(0),
        priority,
    });
    let i = g.w.nodes.len() - 1;
    g.confirm(i);
    i
}

#[test]
fn the_pool_goes_to_the_better_priority_and_the_older_ticket_waits() {
    let mut g = Gate {
        w: World::new(&[3], 1),
    };
    let running = pool_request(&mut g, Priority::Unranked);
    assert!(g.node(running).pool, "the free pool starts the first ingest");
    let older = pool_request(&mut g, Priority::Unranked);
    let better = pool_request(&mut g, Priority::P0);
    let (older_ticket, better_ticket) = (g.node(older).ticket, g.node(better).ticket);
    assert!(older_ticket.is_some() && better_ticket.is_some() && older_ticket < better_ticket);
    assert!(!g.node(older).pool && !g.node(better).pool);
    g.reply(running, Reply::IngestDone { size: 1 });
    // proofs.rs:1680 cover!(after.nodes[0].pool && before.nodes[1..].iter().any(|m| m.ticket.is_some_and(|t| t < mine.1)),
    //     "a better priority passes an older ticket")
    assert!(g.node(better).pool && g.node(better).ticket.is_none());
    // proofs.rs:1687 cover!(!after.nodes[0].pool && before.pool_capacity == 1, "a better waiter keeps its place")
    assert!(!g.node(older).pool && g.w.pool_capacity == 1);
    assert_eq!(g.node(older).ticket, older_ticket);
}

// ---- K5 G17 second cut, part A ----

/// proofs.rs:233 cover!(after != before, "the event changes the world"). From a
/// consistent settled world (settle is then the identity), a changed `step`
/// result means the event's `apply` changed the world.
fn changes(family: &str, w: &World, event: Event) {
    assert!(w.valid(), "{family}: the pre-state is consistent and settled");
    assert_eq!(settle_world(w.clone()).0, *w, "{family}: settle is the identity here");
    let after = step(w.clone(), event).world;
    assert!(after.valid(), "{family}: the result is consistent and settled");
    assert_ne!(after, *w, "{family}: the event changes the world");
}

impl Run {
    fn command(&self, command: Command) -> Event {
        Event::Command {
            node: NodeId(0),
            id: self.node().last_cmd + 1,
            command,
        }
    }

    fn answer(&self, answer: Answer) -> Event {
        let q = self.node().question.expect("an open question");
        self.command(Command::Answer {
            question: q.id,
            answer,
        })
    }

    fn ack_now(&self, ack: Ack) -> Event {
        Event::Ack {
            node: NodeId(0),
            seq: self.node().effect.expect("an effect in flight").seq,
            ack,
        }
    }

    fn planning() -> Run {
        let mut run = Run::new(false);
        run.reply(Reply::IngestDone { size: 1 });
        run.reply(Reply::AuditAgrees);
        assert!(matches!(run.node().phase, Phase::Planning { .. }) && run.awaiting());
        run
    }

    fn making() -> Run {
        let mut run = Run::planning();
        let footprint = run.w.intern_footprint([FileId(0)]);
        run.reply(Reply::PlanLeaf {
            footprint,
            fits: true,
        });
        assert!(run.node().phase == Phase::Making && run.awaiting());
        run
    }
}

#[test]
fn every_event_family_can_change_the_world() {
    let fresh = World::new(&[1], 1);
    changes("apply_new_request", &fresh, Event::NewRequest {
        repo: RepoId(0),
        priority: Priority::Unranked,
    });
    changes("apply_new_adoption", &fresh, Event::NewAdoption {
        repo: RepoId(0),
        head: HeadId(7),
        priority: Priority::Unranked,
    });

    let ingesting = Run::new(false);
    assert!(ingesting.awaiting());
    changes("apply_ingest_done", &ingesting.w, ingesting.reply_now(Reply::IngestDone { size: 1 }));
    changes(
        "apply_ingest_early_end",
        &ingesting.w,
        ingesting.reply_now(Reply::IngestEarlyEnd(EarlyEnd::Duplicate)),
    );
    changes("apply_protected_paths", &ingesting.w, ingesting.reply_now(Reply::ProtectedPaths));

    let mut auditing = Run::new(false);
    auditing.reply(Reply::IngestDone { size: 1 });
    changes("apply_audit_agrees", &auditing.w, auditing.reply_now(Reply::AuditAgrees));

    let mut planning = Run::planning();
    let footprint = planning.w.intern_footprint([FileId(0)]);
    changes("apply_plan_leaf", &planning.w, planning.reply_now(Reply::PlanLeaf { footprint, fits: true }));
    changes("apply_plan_split", &planning.w, planning.reply_now(Reply::PlanSplit));
    changes("apply_plan_cant_meet", &planning.w, planning.reply_now(Reply::PlanCantMeet));

    let mut making = Run::making();
    changes("apply_maker_done", &making.w, making.reply_now(Reply::MakerDone { head: HeadId(5) }));
    let files = making.w.intern_footprint([FileId(1)]);
    changes("apply_maker_needs_files", &making.w, making.reply_now(Reply::MakerNeedsFiles { files, fits: true }));
    changes("apply_maker_cant_meet", &making.w, making.reply_now(Reply::MakerCantMeet));
    changes("apply_maker_out_of_room", &making.w, making.reply_now(Reply::MakerOutOfRoom));
    changes("apply_run_failed", &making.w, making.reply_now(Reply::RunFailed));

    let mut checking = Run::making();
    checking.head += 1;
    checking.reply(Reply::MakerDone {
        head: HeadId(checking.head),
    });
    // Checking waits on its checks job, not a run (Item::Await on the job generation).
    let c = checking.node();
    assert!(c.phase == Phase::Checking && c.effect.is_none() && c.job == Some(c.generation));
    changes("apply_checks_done", &checking.w, checking.reply_now(CHECKS_PASS));

    let reviewing = Run::reviewing(false);
    changes("apply_approved", &reviewing.w, reviewing.reply_now(Reply::Approved { head: HeadId(reviewing.head) }));
    changes("apply_reviewer_out_of_room", &reviewing.w, reviewing.reply_now(Reply::ReviewerOutOfRoom));
    changes("apply_changes_requested", &reviewing.w, reviewing.reply_now(Reply::ChangesRequested));
    changes("apply_stop", &reviewing.w, reviewing.command(Command::Stop));
    changes("apply_pause", &reviewing.w, reviewing.command(Command::Pause));
    changes("apply_change_request", &reviewing.w, reviewing.command(Command::ChangeRequest));

    let mut paused = Run::reviewing(false);
    paused.send(paused.command(Command::Pause));
    assert!(paused.node().paused);
    changes("apply_resume", &paused.w, paused.command(Command::Resume));

    let rebasing = Run::rebasing();
    changes("apply_rebase_clean", &rebasing.w, rebasing.reply_now(Reply::RebaseClean { head: HeadId(9) }));
    changes("apply_rebase_needs_changes", &rebasing.w, rebasing.reply_now(Reply::RebaseNeedsChanges));

    let publishing = Run::publishing(false);
    changes("apply_ack_confirmed", &publishing.w, publishing.ack_now(Ack::Confirmed));
    changes("apply_ack_retryable", &publishing.w, publishing.ack_now(Ack::Retryable));
    changes("apply_ack_permanent", &publishing.w, publishing.ack_now(Ack::Permanent));
    changes("apply_ack_body_policy_refused", &publishing.w, publishing.ack_now(Ack::BodyPolicyRefused));

    let enqueue = Run::at_enqueue(false);
    changes("apply_ack_delivery_accepted", &enqueue.w, enqueue.ack_now(Ack::DeliveryAccepted));
    changes("apply_ci_failed", &enqueue.w, ci_failed());

    let delivered = Run::delivered(false);
    assert!(delivered.node().had_pr);
    changes("apply_outside_push", &delivered.w, Event::Fact {
        node: NodeId(0),
        fact: Fact::PushByOther { head: HeadId(delivered.head + 3) },
    });

    // Rob's answers, each to a question kind that admits it (command_helpers.rs `allowed`).
    let mut out_of_credit = Run::reviewing(false);
    out_of_credit.tune(|n| {
        n.rechecks = FREE_RECHECKS;
        broke(n);
    });
    out_of_credit.send(out_of_credit.reply_now(Reply::ReviewerOutOfRoom));
    assert_eq!(out_of_credit.node().question.unwrap().kind, QKind::OutOfCredit);
    changes("apply_answer_add_credit", &out_of_credit.w, out_of_credit.answer(Answer::AddCredit(1)));
    changes("apply_answer_change_request", &out_of_credit.w, out_of_credit.answer(Answer::ChangeRequest));
    changes("apply_answer_stop", &out_of_credit.w, out_of_credit.answer(Answer::Stop));
    changes("apply_answer_unclear", &out_of_credit.w, out_of_credit.answer(Answer::Unclear));

    let mut effect_failed = Run::publishing(false);
    effect_failed.send(effect_failed.ack_now(Ack::Permanent));
    assert_eq!(effect_failed.node().question.unwrap().kind, QKind::EffectFailed);
    changes("apply_answer_continue", &effect_failed.w, effect_failed.answer(Answer::Continue));

    let mut cant_meet = Run::planning();
    let footprint = cant_meet.w.intern_footprint([FileId(0)]);
    cant_meet.reply(Reply::PlanLeaf { footprint, fits: false });
    cant_meet.reply(Reply::PlanLeaf { footprint, fits: false });
    assert_eq!(cant_meet.node().question.unwrap().kind, QKind::CantMeet);
    changes("apply_answer_try_again", &cant_meet.w, cant_meet.answer(Answer::TryAgain));

    let mut cleanup = Run::reviewing(false);
    cleanup.send(cleanup.command(Command::Stop));
    assert!(matches!(cleanup.node().phase, Phase::Stopping(_)));
    cleanup.send(cleanup.ack_now(Ack::Permanent));
    assert_eq!(cleanup.node().question.unwrap().kind, QKind::CleanupFailed);
    changes("apply_answer_retry", &cleanup.w, cleanup.answer(Answer::Retry));
    changes("apply_answer_mark_handled", &cleanup.w, cleanup.answer(Answer::MarkHandled));
}

// check_gate_grant / check_gate_planning covers at both original widths.

impl Gate {
    /// `count` holders on disjoint files 10.., each holding a slot.
    fn holders(&mut self, count: u8) -> Vec<usize> {
        let mut held = Vec::new();
        for k in 0..count {
            let h = self.request();
            self.plan(h, &[10 + u64::from(k)]);
            assert!(self.holds(h));
            held.push(h);
        }
        held
    }

    fn holder_count(&self) -> usize {
        self.w.nodes.iter().filter(|n| n.holds).count()
    }
}

#[test]
fn gate_grants_the_last_available_slot() {
    for width in [2u8, 3] {
        // proofs.rs:1270 cover!(after.nodes[0].holds && holders == usize::from(width - 1), "last available slot granted")
        let mut g = Gate::new(width);
        g.holders(width - 1);
        assert_eq!(g.holder_count(), usize::from(width - 1));
        let leaf = g.request();
        g.plan(leaf, &[30]);
        assert!(g.holds(leaf) && g.node(leaf).ticket.is_none());
    }
}

#[test]
fn a_file_blocked_older_waiter_holds_back_a_younger_overlapping_leaf() {
    for width in [2u8, 3] {
        let mut g = Gate::new(width);
        let held = g.holders(width - 1);
        let blocked_older = g.request();
        let younger = g.request();
        g.plan(blocked_older, &[10, 20]);
        // proofs.rs:1274 cover!(n.ticket.is_some() && !n.paused && !slot_free(&before, 0), "full gate or overlapping footprint blocks")
        assert!(g.waiting(blocked_older) && !g.node(blocked_older).paused);
        assert!(g.node(held[0]).holds, "the overlapping holder keeps file 10");
        g.plan(younger, &[20]);
        // proofs.rs:1308 cover!(n.ticket.is_some() && !n.paused && slot_free(&before, 0)
        //     && before.nodes.iter().enumerate().any(|(j, older)| { j != 0
        //         && matches!(older.phase, Phase::Ready(_))
        //         && older.ticket.is_some_and(|theirs| theirs < n.ticket.unwrap())
        //         && !slot_free(&before, j)
        //         && !before.footprints_disjoint(older.footprint, n.footprint) })
        //     && !after.nodes[0].holds,
        //     "a file-blocked older waiter prevents a younger leaf taking another needed file")
        assert!(g.holder_count() < usize::from(width), "a slot is free for the younger leaf");
        assert!(g.waiting(younger) && !g.holds(younger));
        assert!(g.node(blocked_older).ticket < g.node(younger).ticket);
    }
}

#[test]
fn an_older_runnable_ticket_keeps_its_place() {
    for width in [2u8, 3] {
        // proofs.rs:1278 cover!(n.ticket.is_some() && !n.paused && slot_free(&before, 0) && !first_in_line(&before, 0), "older ticket keeps its place")
        let mut g = Gate::new(width);
        let held = g.holders(width);
        let younger = g.request();
        let older = g.request();
        g.plan(older, &[30]);
        g.plan(younger, &[31]);
        assert!(g.waiting(older) && g.waiting(younger));
        assert!(g.node(older).ticket < g.node(younger).ticket);
        let ticket = g.node(younger).ticket;
        // Releasing one slot: settle visits the younger (lower index) first; its
        // slot is free, but the older ticket is runnable, so it keeps its place.
        g.stop(held[0]);
        assert!(g.holds(older));
        assert!(!g.holds(younger) && g.node(younger).ticket == ticket);
    }
}

#[test]
fn w_slot_waiters_block_planning_and_fewer_admit_it() {
    for width in [2u8, 3] {
        let mut g = Gate::new(width);
        g.holders(width);
        let mut waiters = Vec::new();
        for k in 0..width {
            let r = g.request();
            g.plan(r, &[40 + u64::from(k)]);
            assert!(g.waiting(r));
            waiters.push(r);
        }
        // proofs.rs:1356 cover!(waiting == usize::from(width) && !changed, "W waiters block planning")
        let blocked = g.request();
        g.reply(blocked, Reply::AuditAgrees);
        assert_eq!(g.node(blocked).phase, Phase::Unplanned);

        let mut g = Gate::new(width);
        g.holders(width);
        for k in 0..width - 1 {
            let r = g.request();
            g.plan(r, &[40 + u64::from(k)]);
            assert!(g.waiting(r));
        }
        // proofs.rs:1360 cover!(waiting == usize::from(width - 1) && changed, "fewer than W waiters admit planning")
        let admitted = g.request();
        g.reply(admitted, Reply::AuditAgrees);
        assert!(matches!(g.node(admitted).phase, Phase::Planning { .. }));
    }
}

// ---- K5 G17 second cut, part B ----

/// proofs.rs:568 cover!(changed == moves, "the phase moves under settle, or waits").
/// `step_traced` records the world after apply and after every moving advance,
/// so in a one-request world each consecutive pair is one moving `advance` of
/// node 0 from the earlier world, which Kani's `any_world` domain admits when
/// it is consistent (settled or not).
fn moves_from(trace: &[World], phase: impl Fn(Phase) -> bool) -> bool {
    trace.windows(2).any(|pair| {
        phase(pair[0].nodes[0].phase) && pair[0].consistent() && pair[0].nodes[0] != pair[1].nodes[0]
    })
}

#[test]
fn every_phase_moves_under_settle_or_waits() {
    let (_, _, trace) = step_traced(World::new(&[1], 1), Event::NewRequest {
        repo: RepoId(0),
        priority: Priority::Unranked,
    });
    assert!(moves_from(&trace, |p| p == Phase::Ingesting), "advance_ingesting");

    let ingesting = Run::new(false);
    let (_, _, trace) = step_traced(ingesting.w.clone(), ingesting.reply_now(Reply::IngestDone { size: 1 }));
    assert!(moves_from(&trace, |p| p == Phase::Auditing), "advance_auditing");

    let mut auditing = Run::new(false);
    auditing.reply(Reply::IngestDone { size: 1 });
    let (_, _, trace) = step_traced(auditing.w.clone(), auditing.reply_now(Reply::AuditAgrees));
    assert!(moves_from(&trace, |p| p == Phase::Unplanned), "advance_unplanned");
    assert!(moves_from(&trace, |p| matches!(p, Phase::Planning { .. })), "advance_planning");

    let mut planning = Run::planning();
    let footprint = planning.w.intern_footprint([FileId(0)]);
    let (_, _, trace) = step_traced(planning.w.clone(), planning.reply_now(Reply::PlanLeaf { footprint, fits: true }));
    assert!(moves_from(&trace, |p| matches!(p, Phase::Ready(_))), "advance_ready");
    assert!(moves_from(&trace, |p| p == Phase::Making), "advance_making");

    let making = Run::making();
    let (_, _, trace) = step_traced(making.w.clone(), making.reply_now(Reply::MakerDone { head: HeadId(5) }));
    assert!(moves_from(&trace, |p| p == Phase::Checking), "advance_checking");

    let mut checking = Run::making();
    checking.head += 1;
    checking.reply(Reply::MakerDone {
        head: HeadId(checking.head),
    });
    let (_, _, trace) = step_traced(checking.w.clone(), checking.reply_now(CHECKS_PASS));
    assert!(moves_from(&trace, |p| p == Phase::Reviewing), "advance_reviewing");

    let reviewing = Run::reviewing(false);
    let (_, _, trace) = step_traced(
        reviewing.w.clone(),
        reviewing.reply_now(Reply::Approved { head: HeadId(reviewing.head) }),
    );
    assert!(moves_from(&trace, |p| p == Phase::Publishing), "advance_publishing");
    let (_, _, trace) = step_traced(reviewing.w.clone(), reviewing.command(Command::Stop));
    assert!(moves_from(&trace, |p| matches!(p, Phase::Stopping(_))), "advance_stopping");

    let delivered = Run::delivered(false);
    let (_, _, trace) = step_traced(delivered.w.clone(), Event::Fact {
        node: NodeId(0),
        fact: Fact::Conflict,
    });
    assert!(moves_from(&trace, |p| p == Phase::Rebasing), "advance_rebasing");
    // advance_awaiting_delivery: check_advance(Phase::AwaitingDelivery, false);
    // the waiting phase does not move, so settle leaves the world unchanged.
    assert_eq!(delivered.node().phase, Phase::AwaitingDelivery);
    assert!(delivered.w.consistent());
    assert_eq!(settle_world(delivered.w.clone()).0, delivered.w);
}

#[test]
fn ci_failure_in_round_one_under_fail_fast_stops_for_review() {
    // proofs.rs:1452 cover!(before.fail_fast && b.round == 1 && b.review_stop.is_none()
    //     && n.review_stop == Some(ReviewStop::CiFailed), "fail_fast stops for TL3 review")
    let mut run = Run::at_enqueue(true);
    run.w.fail_fast = true;
    assert!(run.w.valid());
    let b = run.node();
    run.send(ci_failed());
    let n = run.node();
    assert!(run.w.fail_fast && b.round == 1 && b.review_stop.is_none());
    assert_eq!(n.review_stop, Some(ReviewStop::CiFailed));
    assert_eq!(n.credit, b.credit);
}
