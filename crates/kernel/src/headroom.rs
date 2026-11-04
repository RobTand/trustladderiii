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
        let tickets = self.ticket_step_budget();
        if tickets.is_none_or(|budget| self.next_ticket > u64::MAX - budget) {
            return Some(CounterExhaustion {
                node: None,
                counter: "next_ticket",
            });
        }
        if self.next_question > u64::MAX - QUESTION_STEP_BUDGET {
            return Some(CounterExhaustion {
                node: None,
                counter: "next_question",
            });
        }
        for (index, node) in self.nodes.iter().enumerate() {
            let counter = if node.generation > u64::MAX - GENERATION_STEP_BUDGET {
                Some("generation")
            } else if node.next_seq > u64::MAX - EFFECT_STEP_BUDGET {
                Some("next_seq")
            } else if node.credit.start > u32::MAX - CREDIT_START_STEP_BUDGET {
                Some("credit.start")
            } else if node.credit.earned > u32::MAX - CREDIT_EARNED_STEP_BUDGET {
                Some("credit.earned")
            } else if node.credit.added > u32::MAX - CREDIT_ADDED_STEP_BUDGET {
                Some("credit.added")
            } else if node.credit.spent > u32::MAX - CREDIT_SPENT_STEP_BUDGET {
                Some("credit.spent")
            } else {
                None
            };
            if let Some(counter) = counter {
                return Some(CounterExhaustion {
                    node: Some(NodeId(index as u64)),
                    counter,
                });
            }
        }
        None
    }
}
