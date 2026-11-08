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
