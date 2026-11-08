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

/// A Ready slot waiter is not in the pool line.
#[test]
fn a_slot_waiter_has_no_pool_key() {
    let mut n = Node::new(RepoId(0));
    n.phase = Phase::Ready(Resume::Making);
    n.ticket = Some(3);
    assert_eq!(pool_key(&n), None);
}
