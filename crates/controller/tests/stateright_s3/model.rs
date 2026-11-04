use stateright::{Model, Property};

// The red commit deliberately bypasses the read-before-retry path in lib.rs.
const CONSULT_LEDGER_BEFORE_RETRY: bool = false;
pub const MAX_LOSSES: u8 = 2;
pub const MAX_DUPLICATES: u8 = 1;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Contract {
    Once,
    Converges,
    Message,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Phase {
    Intended,
    Sent,
    Delivering,
    Response,
    Unknown,
    Uncertain,
    Found,
    Absent,
    Confirmed,
    Fenced,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Effect {
    pub contract: Contract,
    pub epoch: u8,
    pub phase: Phase,
    pub pending: u8,
    pub recorded: bool,
    pub mutations: u8,
    pub copies: u8,
    pub at_target: bool,
    race: bool,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct State {
    pub effects: Vec<Effect>,
    pub fence: u8,
    pub losses: u8,
    pub duplicates: u8,
    retries: u8,
    rejected: u8,
    stale_mutation: bool,
    reconciled: Option<usize>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    CommitSent(usize),
    Dispatch(usize),
    Deliver(usize),
    DuplicateDelivery(usize),
    LoseRequest(usize),
    LoseResponse(usize),
    MarkUncertain(usize),
    ReadRecord(usize),
    RetrySameKey(usize),
    CommitConfirmed(usize),
    ReconcileCopies(usize),
    CancelStale(usize),
    AdvanceFence,
}

pub struct EffectModel {
    contracts: Vec<Contract>,
}

impl EffectModel {
    pub fn new(contracts: Vec<Contract>) -> Self {
        assert!((1..=2).contains(&contracts.len()));
        Self { contracts }
    }

    fn settled(state: &State) -> bool {
        state.effects.iter().all(|effect| match effect.phase {
            Phase::Fenced => effect.copies <= 1,
            Phase::Confirmed => match effect.contract {
                Contract::Once => effect.mutations == 1,
                Contract::Converges => effect.at_target,
                Contract::Message => effect.copies == 1,
            },
            _ => false,
        })
    }
}

impl Model for EffectModel {
    type State = State;
    type Action = Action;

    fn init_states(&self) -> Vec<State> {
        let initial = State {
            effects: self
                .contracts
                .iter()
                .enumerate()
                .map(|(key, &contract)| Effect {
                    contract,
                    epoch: key as u8,
                    phase: Phase::Intended,
                    pending: 0,
                    recorded: false,
                    mutations: 0,
                    copies: 0,
                    at_target: false,
                    race: false,
                })
                .collect(),
            fence: 0,
            losses: 0,
            duplicates: 0,
            retries: 0,
            rejected: 0,
            stale_mutation: false,
            reconciled: None,
        };
        let mut states = vec![initial.clone()];
        if initial.effects.len() == 2 {
            let mut same_epoch = initial;
            same_epoch.effects[1].epoch = 0;
            states.push(same_epoch);
        }
        states
    }

    fn actions(&self, state: &State, actions: &mut Vec<Action>) {
        use Action::*;
        for (key, effect) in state.effects.iter().enumerate() {
            match effect.phase {
                Phase::Intended if effect.epoch < state.fence => actions.push(CancelStale(key)),
                Phase::Intended => actions.push(CommitSent(key)),
                Phase::Sent => actions.push(Dispatch(key)),
                Phase::Delivering => {
                    actions.push(Deliver(key));
                    if state.losses < MAX_LOSSES {
                        actions.push(LoseRequest(key));
                    }
                    if effect.pending == 1 && state.duplicates < MAX_DUPLICATES {
                        actions.push(DuplicateDelivery(key));
                    }
                }
                Phase::Response => {
                    actions.push(CommitConfirmed(key));
                    if state.losses < MAX_LOSSES {
                        actions.push(LoseResponse(key));
                    }
                }
                Phase::Unknown => actions.push(MarkUncertain(key)),
                Phase::Uncertain => actions.push(ReadRecord(key)),
                Phase::Absent => actions.push(RetrySameKey(key)),
                Phase::Found => actions.push(CommitConfirmed(key)),
                Phase::Confirmed | Phase::Fenced => {
                    if effect.contract == Contract::Message && effect.copies > 1 {
                        actions.push(ReconcileCopies(key));
                    }
                }
            }
        }
        if state.fence == 0 {
            actions.push(AdvanceFence);
        }
    }

    fn next_state(&self, state: &State, action: Action) -> Option<State> {
        use Action::*;
        let mut next = state.clone();
        next.reconciled = None;
        let key = match action {
            CommitSent(k) | Dispatch(k) | Deliver(k) | DuplicateDelivery(k) | LoseRequest(k)
            | LoseResponse(k) | MarkUncertain(k) | ReadRecord(k) | RetrySameKey(k)
            | CommitConfirmed(k) | ReconcileCopies(k) | CancelStale(k) => k,
            AdvanceFence => {
                next.fence = 1;
                return Some(next);
            }
        };
        let effect = &mut next.effects[key];
        match action {
            CommitSent(_) => effect.phase = Phase::Sent,
            Dispatch(_) => {
                effect.phase = Phase::Delivering;
                effect.pending = 1;
                effect.race = false;
            }
            DuplicateDelivery(_) => {
                next.duplicates += 1;
                effect.pending += 1;
                // Two attempts can both read an absent marker before either POST.
                effect.race = !effect.recorded;
            }
            Deliver(_) => {
                if effect.epoch < next.fence {
                    next.rejected += 1;
                } else {
                    next.fence = next.fence.max(effect.epoch);
                    let apply = !effect.recorded
                        || effect.contract == Contract::Converges
                        || (effect.contract == Contract::Message && effect.race);
                    if apply {
                        next.stale_mutation |= effect.epoch < next.fence;
                        effect.mutations += 1;
                        effect.recorded = true;
                        match effect.contract {
                            Contract::Once => {}
                            Contract::Converges => effect.at_target = true,
                            Contract::Message => effect.copies += 1,
                        }
                    }
                }
                effect.pending -= 1;
                if effect.pending == 0 {
                    effect.phase = if effect.recorded {
                        Phase::Response
                    } else {
                        Phase::Fenced
                    };
                }
            }
            LoseRequest(_) => {
                next.losses += 1;
                effect.pending -= 1;
                if effect.pending == 0 {
                    effect.phase = Phase::Unknown;
                }
            }
            LoseResponse(_) => {
                next.losses += 1;
                effect.phase = Phase::Unknown;
            }
            MarkUncertain(_) => effect.phase = Phase::Uncertain,
            ReadRecord(_) => {
                if effect.recorded {
                    if !CONSULT_LEDGER_BEFORE_RETRY
                        && effect.contract == Contract::Once
                        && effect.epoch >= next.fence
                    {
                        // Mutant: POST again instead of reading the existing key.
                        effect.mutations += 1;
                    }
                    if effect.contract == Contract::Message {
                        effect.copies = 1;
                        next.reconciled = Some(key);
                    }
                    effect.phase = Phase::Found;
                } else if effect.epoch < next.fence {
                    effect.phase = Phase::Fenced;
                } else {
                    effect.phase = Phase::Absent;
                }
            }
            RetrySameKey(_) => {
                next.retries += 1;
                effect.phase = Phase::Intended;
            }
            CommitConfirmed(_) => effect.phase = Phase::Confirmed,
            ReconcileCopies(_) => {
                effect.copies = 1;
                next.reconciled = Some(key);
            }
            CancelStale(_) => effect.phase = Phase::Fenced,
            AdvanceFence => unreachable!(),
        }
        Some(next)
    }

    fn properties(&self) -> Vec<Property<Self>> {
        let mut properties = vec![
            Property::<Self>::always("once effects mutate at most once per key", |_, s| {
                s.effects
                    .iter()
                    .all(|e| e.contract != Contract::Once || e.mutations <= 1)
            }),
            Property::<Self>::always("converging effects preserve their target", |_, s| {
                s.effects
                    .iter()
                    .all(|e| e.contract != Contract::Converges || e.mutations == 0 || e.at_target)
            }),
            Property::<Self>::always("reconciliation never deletes the last message", |_, s| {
                s.effects
                    .iter()
                    .all(|e| e.contract != Contract::Message || !e.recorded || e.copies >= 1)
            }),
            Property::<Self>::always("a reconciled marker has exactly one copy", |_, s| {
                s.reconciled.is_none_or(|key| s.effects[key].copies == 1)
            }),
            Property::<Self>::always("an older epoch never mutates upstream", |_, s| {
                !s.stale_mutation
            }),
            Property::<Self>::eventually(
                "healthy fair delivery settles every eligible key",
                |_, s| Self::settled(s),
            ),
            Property::<Self>::sometimes("two losses are reachable", |_, s| s.losses == MAX_LOSSES),
            Property::<Self>::sometimes("a duplicate delivery is reachable", |_, s| {
                s.duplicates == MAX_DUPLICATES
            }),
            Property::<Self>::sometimes("authoritative absence retries the same key", |_, s| {
                s.retries > 0
            }),
            Property::<Self>::sometimes("upstream rejects a fenced delivery", |_, s| {
                s.rejected > 0
            }),
            Property::<Self>::sometimes(
                "healthy completion is reachable without fencing",
                |_, s| s.effects.iter().all(|e| e.phase == Phase::Confirmed),
            ),
        ];
        if self.contracts.contains(&Contract::Message) {
            properties.push(Property::<Self>::sometimes(
                "a temporary duplicate message is reachable",
                |_, s| s.effects.iter().any(|e| e.copies == 2),
            ));
        }
        properties
    }
}
