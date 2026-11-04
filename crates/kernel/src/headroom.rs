use crate::{NodeId, World};

// A step appends at most one node; Closed never becomes open again. Every
// u32 scan in admits/pool_free counts only open nodes in a consistent world.
fn scan_step_budget(open_nodes: usize) -> Option<u32> {
    u32::try_from(open_nodes).ok()?.checked_add(1)
}

// Stopping can skip five agenda entries and then close. Every other local
// settle path takes at most four moves before blocking. Each changing pass
// consumes at least one move, so six per open node bounds the u32 pass counter.
fn settle_step_budget(open_nodes: usize) -> Option<u32> {
    scan_step_budget(open_nodes)?.checked_mul(6)
}

/// #771: u64 generation reserve for one event and its complete settle loop.
/// `run_failed` or `delivery_failed` can increment once and `raise_incident`
/// can increment once more through `set_phase`. Other apply paths increment
/// at most once. `advance_node` has at most one further phase change per node.
/// Thus 2 + 1 = 3 is a conservative per-step bound.
pub const GENERATION_STEP_BUDGET: u64 = 3;
/// #771: u64 effect-sequence reserve. Apply can retry one effect through
/// `delivery_failed`; settle can emit at most once per node because that effect
/// blocks further moves. Reserving 1 + 1 = 2 also covers their composition.
pub const EFFECT_STEP_BUDGET: u64 = 2;
/// #771: u64 question reserve. Apply chooses at most one of `ask_rob`, cleanup
/// failure's question, or one clarifying question. Settle allocates none.
pub const QUESTION_STEP_BUDGET: u64 = 1;
/// #771: u32 starting-credit reserve. Ingest initializes this field once to
/// `size + RESERVE`, where size is u8, or `1 + RESERVE` for an early end.
/// The largest assignment is 255 + 3 = 258; it never accumulates deposits.
pub const CREDIT_START_STEP_BUDGET: u32 = u8::MAX as u32 + crate::RESERVE;
/// #771: u32 added-credit reserve. `Credit::fund_and_pay` combines a u8 deposit
/// with the continuation's automatic top-up. A valid continuation costs <= 1.
/// A nonzero deposit covers that cost; a zero deposit tops up by at most one.
/// Therefore the combined increase is <= 255, including automatic top-ups.
pub const CREDIT_ADDED_STEP_BUDGET: u32 = u8::MAX as u32;
/// #771: u32 spent-credit reserve. Apply either spends one credit directly or
/// pays one question continuation; settle spends none. Valid questions cost
/// zero or one, as checked by `World::check_node`.
pub const CREDIT_SPENT_STEP_BUDGET: u32 = 1;
/// #771: u32 earned-credit reserve. `merge_credit` increases this field by one
/// and latches `merge_credited`, preventing another increase during the step.
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
    /// The u32 scratch counters also reserve one new open root and six local
    /// settle moves per open node. Closed nodes do not contribute to these
    /// scans in a consistent world. This admits at most 715,827,881 open nodes;
    /// the mathematical progress sum itself has no machine-width bound.
    pub fn headroom_ok(&self) -> bool {
        self.counter_exhaustion().is_none()
    }

    pub fn counter_exhaustion(&self) -> Option<CounterExhaustion> {
        let open_nodes = self.nodes.iter().filter(|n| !n.phase.is_closed()).count();
        if scan_step_budget(open_nodes).is_none() {
            return Some(CounterExhaustion {
                node: None,
                counter: "node_scan",
            });
        }
        if settle_step_budget(open_nodes).is_none() {
            return Some(CounterExhaustion {
                node: None,
                counter: "settle_passes",
            });
        }
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

#[cfg(test)]
mod tests {
    use super::{scan_step_budget, settle_step_budget};

    /// #771: scratch u32 scans need room for every open node and one new root.
    #[test]
    fn scan_budget_checks_the_actual_u32_width() {
        assert_eq!(scan_step_budget(0), Some(1));
        assert_eq!(scan_step_budget(1024), Some(1025));
        assert_eq!(scan_step_budget(u32::MAX as usize - 1), Some(u32::MAX));
        assert_eq!(scan_step_budget(u32::MAX as usize), None);
    }

    /// #771: a changing settle pass consumes at least one of six moves per node.
    #[test]
    fn settle_budget_reserves_all_six_internal_moves_per_open_node() {
        assert_eq!(settle_step_budget(0), Some(6));
        assert_eq!(settle_step_budget(1024), Some(6150));
        assert_eq!(settle_step_budget(715_827_881), Some(4_294_967_292));
        assert_eq!(settle_step_budget(715_827_882), None);
    }
}
