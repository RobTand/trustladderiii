use crate::{NodeId, World};

/// #771: reserves for one external event followed by the complete settle loop.
/// The event changes one node's generation at most twice; settle changes that
/// node's phase at most once more. At most one effect is emitted by apply and
/// one by settle. Question allocation occurs only in apply.
pub const GENERATION_STEP_BUDGET: u64 = 3;
pub const EFFECT_STEP_BUDGET: u64 = 2;
pub const QUESTION_STEP_BUDGET: u64 = 1;
/// Ingest initializes at most u8::MAX + RESERVE. Other credit entry changes are
/// at most one, except AddCredit, whose u8 input contributes at most 255.
/// pay's automatic top-up is at most one and is needed only if AddCredit did
/// not already cover the continuation, so their combined increase is <= 255.
pub const CREDIT_START_STEP_BUDGET: u32 = u8::MAX as u32 + crate::RESERVE;
pub const CREDIT_ADDED_STEP_BUDGET: u32 = u8::MAX as u32;
pub const CREDIT_SPENT_STEP_BUDGET: u32 = 1;
pub const CREDIT_EARNED_STEP_BUDGET: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CounterExhaustion {
    pub node: Option<NodeId>,
    pub counter: &'static str,
}

impl World {
    /// #771: a step may create one node, and each node can take a slot ticket
    /// followed by an agent-pool ticket during settle. The global reserve must
    /// scale with the world: one admission can unblock many Unplanned roots.
    pub fn ticket_step_budget(&self) -> Option<u64> {
        u64::try_from(self.nodes.len())
            .ok()?
            .checked_add(1)?
            .checked_mul(2)
    }

    /// #771: the executable progress theorem is scoped to counter headroom.
    pub fn headroom_ok(&self) -> bool {
        self.counter_exhaustion().is_none()
    }

    pub fn counter_exhaustion(&self) -> Option<CounterExhaustion> {
        // Red-phase stub. The boundary tests require checking every counter.
        None
    }
}
