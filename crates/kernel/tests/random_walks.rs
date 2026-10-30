//! Seeded random walks. Events are biased toward the ones a node can use
//! (its current generation, effect key and question), with stale, duplicate
//! and malformed events mixed in. After every step the world must be valid
//! and settled, and every witnessed event must lower the progress measure.

use std::collections::BTreeSet;
use tl3_kernel::*;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// File ids for footprints, one pool per seed. Ids below 64 make inline sets
/// and larger ids interned ones, so the walks lock and extend both kinds,
/// mixed, and across the 63/64 boundary.
const POOLS: [[u64; 4]; 4] = [
    [0, 1, 2, 3],
    [62, 63, 64, 65],
    [0, 63, 1 << 40, u64::MAX],
    [64, 66, 1 << 40, u64::MAX],
];

fn footprint(w: &mut World, pool: &[u64; 4], bits: u64) -> Footprint {
    w.intern_footprint(
        (0..4)
            .filter(|i| bits & (1 << i) != 0)
            .map(|i| FileId(pool[i])),
    )
}

fn random_event(w: &mut World, pool: &[u64; 4], rng: &mut Rng) -> Event {
    if w.nodes.is_empty() || (w.nodes.len() < 4 && rng.below(40) == 0) {
        return Event::NewRequest {
            repo: RepoId(rng.below(3)),
        };
    }
    let i = rng.below(w.nodes.len() as u64) as usize;
    let n = w.nodes[i];
    let node = NodeId(if rng.below(50) == 0 { 9 } else { i as u64 });
    let head = HeadId(rng.next());
    match rng.below(13) {
        12 => Event::NoticeFailed {
            node,
            question: n.question.map_or(rng.next(), |q| q.id),
        },
        0..=3 => match n.effect {
            Some(e) => {
                let ack = match rng.below(12) {
                    0 => Ack::Retryable,
                    1 => Ack::Permanent,
                    _ => Ack::Confirmed,
                };
                Event::Ack {
                    node,
                    seq: e.seq,
                    ack,
                }
            }
            None => Event::Ack {
                node,
                seq: rng.below(5),
                ack: Ack::Confirmed,
            },
        },
        4..=7 => {
            let generation = if rng.below(10) == 0 {
                n.generation.saturating_sub(1)
            } else {
                n.generation
            };
            let reply = match rng.below(17) {
                0 => Reply::IngestDone {
                    size: rng.below(4) as u8,
                },
                1 => Reply::IngestEarlyEnd(EarlyEnd::Duplicate),
                2 => Reply::AuditAgrees,
                3 => Reply::AuditDisagrees,
                4 => Reply::PlanLeaf {
                    footprint: footprint(w, pool, rng.below(16)),
                    fits: rng.below(5) != 0,
                },
                5 => Reply::PlanSplit,
                6 => Reply::PlanCantMeet,
                7 => Reply::MakerDone { head },
                8 => Reply::MakerNeedsFiles {
                    files: footprint(w, pool, rng.below(16)),
                    fits: rng.below(2) == 0,
                },
                9 => Reply::MakerCantMeet,
                10 => Reply::MakerOutOfRoom,
                11 => Reply::ChecksDone {
                    report: rng.below(8) != 0,
                    pass: rng.below(3) != 0,
                    in_footprint: rng.below(8) != 0,
                },
                12 => Reply::Approved {
                    head: if rng.below(2) == 0 {
                        n.head.unwrap_or(head)
                    } else {
                        head
                    },
                },
                13 => Reply::ChangesRequested,
                14 => Reply::RebaseClean { head },
                15 => Reply::RebaseNeedsChanges,
                _ => Reply::RunFailed,
            };
            Event::Reply {
                node,
                generation,
                reply,
            }
        }
        8..=9 => {
            let fact = match rng.below(8) {
                0 => Fact::PrMerged {
                    head: if rng.below(2) == 0 {
                        n.head.unwrap_or(head)
                    } else {
                        head
                    },
                    checks_passed: rng.below(4) != 0,
                },
                1 => Fact::PrClosedByPerson,
                2 => Fact::PrDequeuedByPerson,
                3 => Fact::PushByOther { head },
                4 => Fact::CiFailed {
                    infra: rng.below(2) == 0,
                },
                5 => Fact::Conflict,
                6 => Fact::UpstreamStalled,
                _ => Fact::IncidentCleared {
                    by_rob: rng.below(2) == 0,
                },
            };
            Event::Fact { node, fact }
        }
        _ => {
            let id = if rng.below(10) == 0 {
                n.last_cmd
            } else {
                n.last_cmd + 1
            };
            let command = match rng.below(10) {
                0 => Command::Stop,
                1 => Command::ChangeRequest,
                2 | 3 => Command::Pause,
                4 | 5 => Command::Resume,
                _ => {
                    let question = match n.question {
                        Some(q) if rng.below(8) != 0 => q.id,
                        Some(q) => q.id + 1,
                        None => rng.below(3),
                    };
                    let answer = match rng.below(8) {
                        0 => Answer::Continue,
                        1 => Answer::TryAgain,
                        2 => Answer::AddCredit(rng.below(4) as u8),
                        3 => Answer::ChangeRequest,
                        4 => Answer::Stop,
                        5 => Answer::Unclear,
                        6 => Answer::Retry,
                        _ => Answer::MarkHandled,
                    };
                    Command::Answer { question, answer }
                }
            };
            Event::Command { node, id, command }
        }
    }
}

#[test]
fn random_walks_keep_every_rule() {
    let mut phases = BTreeSet::new();
    let mut steps = 0u64;
    let mut merged = 0u64;
    // The most passes settle needed, by how many requests the world held.
    let mut passes_by_size = [0u32; 5];
    for seed in 1..=400u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let pool = &POOLS[seed as usize % POOLS.len()];
        let gates = [1 + rng.below(2) as u8, 1 + rng.below(2) as u8, 1];
        let mut w = World::new(&gates, 1 + rng.below(2) as u8);
        for _ in 0..500 {
            let event = if rng.below(4) == 0 {
                w.witness().unwrap_or(Event::NewRequest { repo: RepoId(0) })
            } else {
                random_event(&mut w, pool, &mut rng)
            };
            let witnessed = w.witness() == Some(event);
            let before = w.measure();
            let (next, out, passes) = step_with_passes(w.clone(), event);
            let size = next.nodes.len().min(4);
            passes_by_size[size] = passes_by_size[size].max(passes);
            // Every intermediate world is consistent, and each internal
            // transition lowers its node's settle rank.
            let (_, _, trace) = step_traced(w.clone(), event);
            for (k, mid) in trace.iter().enumerate() {
                assert!(
                    mid.consistent(),
                    "seed {seed}: {:?} after transition {k} of {event:?}\nfrom {w:#?}\nto {mid:#?}\ntrace before: {:#?}",
                    mid.check(),
                    if k > 0 { Some(&trace[k - 1]) } else { None }
                );
            }
            for pair in trace.windows(2) {
                let (a, b) = (&pair[0], &pair[1]);
                let lowered = a
                    .nodes
                    .iter()
                    .zip(&b.nodes)
                    .any(|(x, y)| y.settle_rank() < x.settle_rank());
                let raised = a
                    .nodes
                    .iter()
                    .zip(&b.nodes)
                    .any(|(x, y)| y.settle_rank() > x.settle_rank());
                assert!(
                    lowered && !raised,
                    "seed {seed}: settle rank didn't fall during {event:?}"
                );
            }
            assert!(
                next.valid(),
                "seed {seed}: invalid world after {event:?}\nfrom {w:#?}\nto {next:#?}"
            );
            assert!(next.is_settled(), "seed {seed}: unsettled after {event:?}");
            // The lock rule again, with plain sets instead of the kernel's set code.
            for (a, x) in next.nodes.iter().enumerate() {
                for y in &next.nodes[..a] {
                    if x.holds && y.holds && x.repo == y.repo {
                        let held: BTreeSet<FileId> = next.files(y.footprint).unwrap().collect();
                        assert!(
                            next.files(x.footprint).unwrap().all(|f| !held.contains(&f)),
                            "seed {seed}: two holders share a file after {event:?}"
                        );
                    }
                }
            }
            if witnessed {
                assert!(
                    next.measure() < before,
                    "seed {seed}: witness {event:?} didn't make progress"
                );
            }
            for o in &out {
                if let Out::Effect {
                    node,
                    kind: EffectKind::Enqueue,
                    head,
                    ..
                } = *o
                {
                    let n = &next.nodes[node.index().unwrap()];
                    let h = head.expect("an enqueue names its head");
                    assert!(
                        n.approved == Some(h) && n.checked == Some(h),
                        "seed {seed}: unapproved enqueue"
                    );
                }
            }
            for n in &next.nodes {
                phases.insert(n.phase.name());
            }
            w = next;
            steps += 1;
        }
        merged += w
            .nodes
            .iter()
            .filter(|n| n.phase == Phase::Closed(Outcome::Merged))
            .count() as u64;
    }
    println!("random walks: {steps} steps, {merged} requests merged, phases seen: {phases:?}");
    println!("most settle passes, by number of requests 0..=4: {passes_by_size:?}");
    assert_eq!(phases.len(), 14, "phases seen: {phases:?}");
}

/// From any state a random walk reaches, following the witness closes every request.
#[test]
fn the_witness_always_finishes_the_work() {
    for seed in 1..=200u64 {
        let mut rng = Rng(seed.wrapping_mul(0xD1B5_4A32_D192_ED03) | 1);
        let pool = &POOLS[seed as usize % POOLS.len()];
        let mut w = World::new(&[1, 2], 2);
        for _ in 0..rng.below(300) {
            let e = random_event(&mut w, pool, &mut rng);
            w = step(w, e).0;
        }
        for _ in 0..2_000 {
            let Some(e) = w.witness() else { break };
            let before = w.measure();
            w = step(w, e).0;
            assert!(w.measure() < before);
        }
        assert!(
            w.nodes.iter().all(|n| n.phase.is_closed()),
            "seed {seed}: {w:#?}"
        );
    }
}
