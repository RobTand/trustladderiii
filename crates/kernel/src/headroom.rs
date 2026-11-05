// Included in the kernel module so contracts can inspect its private storage.
#[cfg(verus_keep_ghost)]
verus! {
/// #771: public ghost view of the executable world's node-vector length.
pub closed spec fn world_node_count(world: World) -> nat { world.nodes@.len() }

/// #771: count open nodes without imposing a machine width on the proof sum.
pub closed spec fn open_prefix(nodes: Seq<Node>, end: nat) -> nat
    recommends end <= nodes.len(),
    decreases end,
{
    if end == 0 { 0 } else {
        open_prefix(nodes, (end - 1) as nat)
            + if nodes[end as int - 1].phase is Closed { 0nat } else { 1nat }
    }
}

/// #771: scratch scans over a prefix cannot exceed the full open-node count.
pub proof fn open_prefix_monotone(nodes: Seq<Node>, prefix: nat, end: nat)
    requires prefix <= end <= nodes.len(),
    ensures open_prefix(nodes, prefix) <= open_prefix(nodes, end)
        <= open_prefix(nodes, prefix) + (end - prefix),
    decreases end - prefix,
{
    reveal(open_prefix);
    if prefix < end {
        open_prefix_monotone(nodes, prefix, (end - 1) as nat);
    }
}

/// #771: an event cannot reopen a Closed node, so only an appended root
/// can increase the open-node scratch reserve.
pub(super) proof fn open_prefix_compare(before: Seq<Node>, after: Seq<Node>, end: nat)
    requires end <= before.len(), end <= after.len(),
        forall|j: int| 0 <= j < end ==> (before[j].phase is Closed ==> after[j].phase is Closed),
    ensures open_prefix(after, end) <= open_prefix(before, end),
    decreases end,
{
    reveal(open_prefix);
    if end > 0 { open_prefix_compare(before, after, (end - 1) as nat); }
}

/// #771: the exact per-node reserve checked at the controller boundary.
pub open spec fn node_headroom(node: Node) -> bool {
    node.generation as int + GENERATION_STEP_BUDGET <= u64::MAX as int
        && node.next_seq as int + EFFECT_STEP_BUDGET <= u64::MAX as int
        && node.credit.start as int + CREDIT_START_STEP_BUDGET <= u32::MAX as int
        && node.credit.earned as int + CREDIT_EARNED_STEP_BUDGET <= u32::MAX as int
        && node.credit.added as int + CREDIT_ADDED_STEP_BUDGET <= u32::MAX as int
        && node.credit.spent as int + CREDIT_SPENT_STEP_BUDGET <= u32::MAX as int
}

/// #771: mathematical statement of the executable apply-boundary scope.
/// This predicate describes reserves only. Step safety also needs the kernel's
/// consistency invariant for its bounded retry counters and valid questions.
pub closed spec fn world_headroom(world: World) -> bool {
    let open = open_prefix(world.nodes@, world.nodes@.len());
    open + 1 <= u32::MAX as int
        && 6 * (open + 1) <= u32::MAX as int
        && world.next_ticket as int + 2 * (world.nodes@.len() + 1) <= u64::MAX as int
        && world.next_question as int + QUESTION_STEP_BUDGET <= u64::MAX as int
        && forall|i: int| 0 <= i < world.nodes@.len()
            ==> node_headroom(world.nodes@[i])
}

/// #771: arithmetic facts required by the existing consistency checks.
/// This is not the full consistency invariant. In particular it says nothing
/// about gate ownership, operation identity, footprints or dependencies.
pub open spec fn arithmetic_consistent(node: Node) -> bool {
    node.run_failures <= 1 && node.fx_failures <= 1
        && node.run_incidents <= 2 && node.fx_incidents <= 2
        && node.credit.start as int + node.credit.earned as int + node.credit.added as int
            >= node.credit.spent as int
        && match node.question {
            None => true,
            Some(q) => q.cost == match q.kind {
                QKind::CantMeet | QKind::OutOfCredit => 1u8,
                _ => 0u8,
            } && (node.credit.withheld ==> q.cost == 0),
        }
}

/// Q54 / #771: shared postcondition for the real paid-retry handlers.
/// It bounds writes; it does not define an alternative transition relation.
#[verifier::opaque]
pub(super) closed spec fn retry_bounds(before: World, after: World, i: int,
    out_before: usize, out_after: usize) -> bool {
    (settle_shape(before.nodes@[i]) ==> settle_shape(after.nodes@[i]))
        && after.nodes@[i].repo == before.nodes@[i].repo
        && after.nodes@[i].footprint == before.nodes@[i].footprint
        && after.nodes@.len() == before.nodes@.len()
        && (forall|j: int| 0 <= j < before.nodes@.len() && j != i
            ==> after.nodes@[j] == before.nodes@[j])
        && after.nodes@[i].generation as int <= before.nodes@[i].generation as int + 1
        && after.nodes@[i].next_seq == before.nodes@[i].next_seq
        && after.nodes@[i].last_cmd == before.nodes@[i].last_cmd
        && after.nodes@[i].credit.start == before.nodes@[i].credit.start
        && after.nodes@[i].credit.earned == before.nodes@[i].credit.earned
        && after.nodes@[i].credit.added == before.nodes@[i].credit.added
        && after.nodes@[i].credit.spent as int <= before.nodes@[i].credit.spent as int + 1
        && after.next_ticket == before.next_ticket
        && after.next_question as int <= before.next_question as int + 1
        && after.gates@ == before.gates@
        && after.pool_capacity == before.pool_capacity
        && after.fail_fast == before.fail_fast
        && after.file_sets@ == before.file_sets@
        && after.dependencies@ == before.dependencies@
        && out_after as int <= out_before as int + 1
}

/// Reply rows / #771: ingest may initialize starting credit, while the other
/// handlers share the paid-retry allocation bounds.
#[verifier::opaque]
pub(super) closed spec fn reply_bounds(before: World, after: World, i: int,
    out_before: usize, out_after: usize) -> bool {
    (settle_shape(before.nodes@[i]) ==> settle_shape(after.nodes@[i]))
        && after.nodes@[i].repo == before.nodes@[i].repo
        && after.nodes@.len() == before.nodes@.len()
        && (forall|j: int| 0 <= j < before.nodes@.len() && j != i
            ==> after.nodes@[j] == before.nodes@[j])
        && after.nodes@[i].generation as int <= before.nodes@[i].generation as int + 1
        && after.nodes@[i].next_seq == before.nodes@[i].next_seq
        && after.nodes@[i].last_cmd == before.nodes@[i].last_cmd
        && after.nodes@[i].credit.start as int <= before.nodes@[i].credit.start as int + CREDIT_START_STEP_BUDGET
        && after.nodes@[i].credit.earned == before.nodes@[i].credit.earned
        && after.nodes@[i].credit.added == before.nodes@[i].credit.added
        && after.nodes@[i].credit.spent as int <= before.nodes@[i].credit.spent as int + 1
        && after.next_ticket == before.next_ticket
        && after.next_question as int <= before.next_question as int + 1
        && after.gates@ == before.gates@
        && after.pool_capacity == before.pool_capacity
        && after.fail_fast == before.fail_fast
        && after.file_sets@ == before.file_sets@
        && after.dependencies@ == before.dependencies@
        && out_after as int <= out_before as int + 1
}
}

// A step appends at most one node; Closed never becomes open again. Every
// u32 scan in admits/pool_free counts only open nodes in a consistent world.
kernel_fn! {
    /// #771: the u32 scan reserve is exactly one plus the current open count.
    fn scan_step_budget(open_nodes: usize) -> (result: Option<u32>) [
        ensures match result {
            Some(budget) => budget as int == open_nodes as int + 1,
            None => open_nodes as int + 1 > u32::MAX as int,
        },
    ] {
        u32::try_from(open_nodes).ok()?.checked_add(1)
    }
}

// Stopping can skip five agenda entries and then close. Every other local
// settle path takes at most four moves before blocking. Each changing pass
// consumes at least one move, so six per open node bounds the u32 pass counter.
kernel_fn! {
    /// #771: each open node contributes at most six local moves to settle.
    fn settle_step_budget(open_nodes: usize) -> (result: Option<u32>) [
        ensures match result {
            Some(budget) => budget as int == 6 * (open_nodes as int + 1),
            None => 6 * (open_nodes as int + 1) > u32::MAX as int,
        },
    ] {
        scan_step_budget(open_nodes)?.checked_mul(6)
    }
}

/// #771: u64 generation reserve for one event and its complete settle loop.
/// `run_failed` or `delivery_failed` can increment once and `raise_incident`
/// can increment once more through `set_phase`. Other apply paths increment
/// at most once. `advance_node` has at most one further phase change per node.
/// Thus 2 + 1 = 3 is a conservative per-step bound.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
pub const GENERATION_STEP_BUDGET: u64 = 3;
/// #771: u64 effect-sequence reserve. Apply can retry one effect through
/// `delivery_failed`; settle can emit at most once per node because that effect
/// blocks further moves. Reserving 1 + 1 = 2 also covers their composition.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
pub const EFFECT_STEP_BUDGET: u64 = 2;
/// #771: u64 question reserve. Apply chooses at most one of `ask_rob`, cleanup
/// failure's question, or one clarifying question. Settle allocates none.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
pub const QUESTION_STEP_BUDGET: u64 = 1;
/// #771: u32 starting-credit reserve. Ingest initializes this field once to
/// `size + RESERVE`, where size is u8, or `1 + RESERVE` for an early end.
/// The largest assignment is 255 + 3 = 258; it never accumulates deposits.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
pub const CREDIT_START_STEP_BUDGET: u32 = u8::MAX as u32 + crate::RESERVE;
/// #771: u32 added-credit reserve. `Credit::fund_and_pay` combines a u8 deposit
/// with the continuation's automatic top-up. A valid continuation costs <= 1.
/// A nonzero deposit covers that cost; a zero deposit tops up by at most one.
/// Therefore the combined increase is <= 255, including automatic top-ups.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
pub const CREDIT_ADDED_STEP_BUDGET: u32 = u8::MAX as u32;
/// #771: u32 spent-credit reserve. Apply either spends one credit directly or
/// pays one question continuation; settle spends none. Valid questions cost
/// zero or one, as checked by `World::check_node`.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
pub const CREDIT_SPENT_STEP_BUDGET: u32 = 1;
/// #771: u32 earned-credit reserve. `merge_credit` increases this field by one
/// and latches `merge_credited`, preventing another increase during the step.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
pub const CREDIT_EARNED_STEP_BUDGET: u32 = 1;

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CounterExhaustion {
    pub node: Option<NodeId>,
    pub counter: &'static str,
}

impl World {
    kernel_fn! {
        /// #771: each node can take a slot ticket followed by a pool ticket.
        /// The reserve includes the one new node that apply can append.
        pub fn ticket_step_budget(&self) -> (result: Option<u64>) [
            ensures match result {
                Some(budget) => budget as int == 2 * (world_node_count(*self) + 1),
                None => 2 * (world_node_count(*self) + 1) > u64::MAX as int,
            },
        ] {
            u64::try_from(self.nodes.len())
                .ok()?
                .checked_add(1)?
                .checked_mul(2)
        }
    }

    kernel_fn! {
        /// #771: the executable step-safety theorem is scoped to counter headroom.
        /// The u32 scratch counters reserve one new open root and six local
        /// settle moves per open node. Closed nodes do not contribute to these
        /// scans in a consistent world. This admits at most 715,827,881 open nodes;
        /// the mathematical progress sum itself has no machine-width bound.
        pub fn headroom_ok(&self) -> (result: bool) [
            ensures result == world_headroom(*self),
        ] {
            self.counter_exhaustion().is_none()
        }
    }

    kernel_fn! {
    /// #771: returning no incident is exactly the theorem's counter scope.
    pub fn counter_exhaustion(&self) -> (result: Option<CounterExhaustion>) [
        ensures result.is_none() == world_headroom(*self),
    ] proof [reveal(world_headroom); reveal(open_prefix);] {
        let mut open_nodes = 0usize;
        let mut index = 0usize;
        kernel_while! { (index < self.nodes.len()) [
            invariant
                index <= self.nodes.len(),
                open_nodes <= index,
                open_nodes as int == open_prefix(self.nodes@, index as nat),
            decreases self.nodes.len() - index,
        ] {
            if !self.nodes[index].phase.is_closed() {
                open_nodes += 1;
            }
            index += 1;
        } }
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
        let ticket_exhausted = match tickets {
            None => true,
            Some(budget) => self.next_ticket > u64::MAX - budget,
        };
        if ticket_exhausted {
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
        index = 0;
        kernel_while! { (index < self.nodes.len()) [
            invariant
                index <= self.nodes.len(),
                open_nodes as int == open_prefix(self.nodes@, self.nodes@.len()),
                6 * (open_nodes as int + 1) <= u32::MAX as int,
                self.next_ticket as int + 2 * (self.nodes@.len() + 1) <= u64::MAX as int,
                self.next_question as int + QUESTION_STEP_BUDGET <= u64::MAX as int,
                forall|j: int| 0 <= j < index ==> node_headroom(self.nodes@[j]),
            decreases self.nodes.len() - index,
        ] {
            let counter = node_counter_exhaustion(&self.nodes[index]);
            if let Some(counter) = counter {
                return Some(CounterExhaustion {
                    node: Some(NodeId(index as u64)),
                    counter,
                });
            }
            index += 1;
        } }
        None
    }
    }
}

kernel_fn! {
    /// #771: identify the first exhausted counter without altering the node.
    fn node_counter_exhaustion(node: &Node) -> (result: Option<&'static str>) [
        ensures result.is_none() == node_headroom(*node),
    ] {
        if node.generation > u64::MAX - GENERATION_STEP_BUDGET {
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
        }
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
