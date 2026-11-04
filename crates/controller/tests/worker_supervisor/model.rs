use stateright::{Checker, Model, Property};
use std::time::Instant;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Claim {
    Free,
    Cancelled,
    Owner(u8),
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Supervisor {
    Fresh,
    Claimed,
    Spawned,
    Reaped,
    Finished,
    Lost,
    Rejected,
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Worker {
    Cold,
    Running,
    Zombie,
    Reaped,
    Cancelled,
}
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct Attempt {
    claim: Claim,
    process: Claim,
    supervisors: [Supervisor; 2],
    worker: Worker,
    writes: u8,
    recorded: bool,
    inspected: bool,
}
impl Default for Attempt {
    fn default() -> Self {
        Self {
            claim: Claim::Free,
            process: Claim::Free,
            supervisors: [Supervisor::Fresh; 2],
            worker: Worker::Cold,
            writes: 0,
            recorded: false,
            inspected: false,
        }
    }
}
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct State {
    runs: [Attempt; 2],
    current: usize,
    online: bool,
    crashed: bool,
    lost: bool,
    replayed: bool,
    pub applied: [u8; 2],
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Claim(usize, u8),
    Spawn(usize, u8),
    Start(usize),
    Exit(usize),
    Reap(usize, u8),
    Publish(usize, u8),
    Lose(usize, u8),
    Inspect(usize),
    Replay(usize),
    CrashController,
    RestartController,
    Advance,
}
pub struct Protocol {
    pub mutant: bool,
}

impl Model for Protocol {
    type State = State;
    type Action = Action;
    fn init_states(&self) -> Vec<State> {
        vec![State {
            runs: Default::default(),
            current: 0,
            online: true,
            crashed: false,
            lost: false,
            replayed: false,
            applied: [0; 2],
        }]
    }
    fn actions(&self, _: &State, actions: &mut Vec<Action>) {
        use Action::*;
        actions.extend([CrashController, RestartController, Advance]);
        for run in 0..2 {
            actions.extend([Start(run), Exit(run), Inspect(run), Replay(run)]);
            for actor in 0..2 {
                actions.extend([
                    Claim(run, actor),
                    Spawn(run, actor),
                    Reap(run, actor),
                    Publish(run, actor),
                    Lose(run, actor),
                ]);
            }
        }
    }
    fn next_state(&self, old: &State, action: Action) -> Option<State> {
        use Action::*;
        let mut state = old.clone();
        match action {
            Claim(run, actor)
                if run <= state.current
                    && state.runs[run].supervisors[actor as usize] == Supervisor::Fresh =>
            {
                let attempt = &mut state.runs[run];
                attempt.supervisors[actor as usize] = if attempt.claim == self::Claim::Free {
                    attempt.claim = self::Claim::Owner(actor);
                    Supervisor::Claimed
                } else {
                    Supervisor::Rejected
                };
            }
            Spawn(run, actor)
                if state.runs[run].supervisors[actor as usize] == Supervisor::Claimed =>
            {
                state.runs[run].supervisors[actor as usize] = Supervisor::Spawned;
            }
            Start(run)
                if state.runs[run].worker == Worker::Cold
                    && state.runs[run].supervisors.contains(&Supervisor::Spawned) =>
            {
                let attempt = &mut state.runs[run];
                if attempt.process == self::Claim::Free {
                    attempt.process = attempt.claim;
                    attempt.worker = Worker::Running;
                } else {
                    attempt.worker = Worker::Cancelled;
                }
            }
            Exit(run) if state.runs[run].worker == Worker::Running => {
                state.runs[run].worker = Worker::Zombie
            }
            Reap(run, actor)
                if state.runs[run].supervisors[actor as usize] == Supervisor::Spawned
                    && matches!(state.runs[run].worker, Worker::Zombie | Worker::Cancelled) =>
            {
                state.runs[run].worker = Worker::Reaped;
                state.runs[run].supervisors[actor as usize] = Supervisor::Reaped;
            }
            Publish(run, actor)
                if state.runs[run].supervisors[actor as usize] == Supervisor::Reaped
                    || (self.mutant
                        && state.runs[run].supervisors[actor as usize] == Supervisor::Rejected) =>
            {
                let attempt = &mut state.runs[run];
                attempt.writes += 1;
                attempt.recorded = true;
                attempt.supervisors[actor as usize] = Supervisor::Finished;
            }
            // Process loss after waitpid but before the atomic exit rename.
            Lose(run, actor)
                if !state.lost
                    && state.runs[run].supervisors[actor as usize] == Supervisor::Reaped =>
            {
                state.lost = true;
                state.runs[run].supervisors[actor as usize] = Supervisor::Lost;
            }
            Inspect(run) if state.online && run <= state.current && !state.runs[run].inspected => {
                let attempt = &mut state.runs[run];
                if matches!(attempt.claim, self::Claim::Free) {
                    attempt.claim = self::Claim::Cancelled;
                }
                if attempt.supervisors.iter().any(|s| {
                    matches!(
                        s,
                        Supervisor::Claimed | Supervisor::Spawned | Supervisor::Reaped
                    )
                }) || attempt.worker == Worker::Running
                {
                    return None;
                }
                if attempt.process == self::Claim::Free {
                    attempt.process = self::Claim::Cancelled;
                }
                attempt.inspected = true;
                // A missing exit is authoritative absence, not an invented successful reply.
                if attempt.recorded && run == state.current && state.applied[run] == 0 {
                    state.applied[run] += 1;
                }
            }
            Replay(run) if state.online && run < state.current && !state.replayed => {
                state.replayed = true;
            }
            CrashController if state.online && !state.crashed => {
                state.online = false;
                state.crashed = true;
            }
            RestartController if !state.online => state.online = true,
            Advance if state.online && state.current == 0 && state.runs[0].inspected => {
                state.current = 1
            }
            _ => return None,
        }
        Some(state)
    }
    fn properties(&self) -> Vec<Property<Self>> {
        vec![
            Property::<Self>::always("one exit writer owns the claim", |_, s| {
                s.runs
                    .iter()
                    .all(|r| r.writes <= 1 && (r.writes == 0 || matches!(r.claim, Claim::Owner(_))))
            }),
            Property::<Self>::always("each generation applies at most once", |_, s| {
                s.applied.iter().all(|n| *n <= 1)
            }),
            Property::<Self>::always("applied replies have their own exit", |_, s| {
                s.runs
                    .iter()
                    .enumerate()
                    .all(|(i, r)| s.applied[i] == 0 || r.recorded)
            }),
            Property::<Self>::always("exit is published only after waitpid", |_, s| {
                s.runs
                    .iter()
                    .all(|r| !r.recorded || r.worker == Worker::Reaped)
            }),
            Property::<Self>::eventually("no zombie remains", |_, s| {
                s.current == 1
                    && s.runs.iter().all(|r| {
                        r.inspected
                            && r.worker != Worker::Zombie
                            && !r.supervisors.iter().any(|s| {
                                matches!(
                                    s,
                                    Supervisor::Claimed | Supervisor::Spawned | Supervisor::Reaped
                                )
                            })
                    })
            }),
            Property::<Self>::sometimes("lost exit is reconciled as absence", |_, s| {
                s.runs.iter().any(|r| {
                    r.inspected && r.supervisors.contains(&Supervisor::Lost) && !r.recorded
                })
            }),
            Property::<Self>::sometimes("restart overlaps worker exit", |_, s| {
                s.crashed && !s.online && s.runs.iter().any(|r| r.worker == Worker::Zombie)
            }),
            Property::<Self>::sometimes("both generations complete", |_, s| s.applied == [1, 1]),
        ]
    }
}

#[test]
fn p12_s4_exhaustive_protocol() {
    let started = Instant::now();
    let checker = Protocol { mutant: false }.checker().spawn_bfs().join();
    eprintln!(
        "P12/S4: {} unique states, {} visited, {:?}",
        checker.unique_state_count(),
        checker.state_count(),
        started.elapsed()
    );
    checker.assert_properties();
}

#[test]
fn p12_rejects_exit_without_claim_mutant() {
    let checker = Protocol { mutant: true }.checker().spawn_bfs().join();
    let trace = checker.assert_any_discovery("one exit writer owns the claim");
    eprintln!("seeded counterexample: {trace:?}");
}
