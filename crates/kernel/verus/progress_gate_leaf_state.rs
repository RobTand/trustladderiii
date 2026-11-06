//! Q48 / #806: complete-checker preservation for the real leaf sequence.
use super::*;

verus! {
/// #806: original four leaves, with positive width generalized beyond W2/W3.
pub(super) open spec fn domain(w: World) -> bool {
    retirement_wide::domain(w) && w.nodes@.len() == 4 && w.gates@.len() == 1
        && w.file_sets@.len() == 0 && w.next_question == 0
}

/// #806: these generous reserves include every counter in the original constructor.
pub(super) open spec fn low(w: World) -> bool {
    w.next_ticket <= 1024 && forall|j: int| 0 <= j < w.nodes@.len() ==>
        w.nodes@[j].generation <= 1024 && w.nodes@[j].next_seq <= 1024
        && w.nodes@[j].credit.start <= 1024 && w.nodes@[j].credit.earned <= 1024
        && w.nodes@[j].credit.added <= 1024 && w.nodes@[j].credit.spent <= 1024
}

/// Q48: only Ready(Making), Queued, the freshly granted maker, and successful
/// Stopped teardown occur in the original leaf sequence.
pub(super) open spec fn tracked(n: Node) -> bool {
    !n.paused && !n.waiting_incident && n.effect.is_none() && n.question.is_none()
        && (n.phase == Phase::Ready(Resume::Making) && n.pos == 0
            || n.phase == Phase::Queued
            || n.phase == Phase::Making && n.pos == 0
            || progress_gate_leaf_steps::teardown_shape(n))
}

/// #806: exact result of one real tracked advance; all cases are source-connected.
#[verifier::opaque]
pub(super) open spec fn updated(w: World, i: int) -> Node {
    let n = w.nodes@[i];
    if n.phase is Ready {
        retirement_gate::slot_node(n, w.next_ticket,
            retirement_gate::slot(w, i) && retirement_gate::line(w, i))
    } else if n.phase == Phase::Making {
        progress_gate_leaf_steps::emitted(n, EffectKind::PrepareWorktree)
    } else if n.phase is Stopping { progress_gate_leaf_steps::cleanup_advance(n) }
    else { n }
}

/// #806: the scheduler changes one node; the other world tables keep their values.
pub(super) open spec fn frame(before: World, after: World, i: int) -> bool {
    after.nodes@ == before.nodes@.update(i, after.nodes@[i])
        && after.gates@ == before.gates@ && after.file_sets@ == before.file_sets@
        && after.dependencies@ == before.dependencies@ && after.fail_fast == before.fail_fast
        && after.pool_capacity == before.pool_capacity && after.next_question == before.next_question
        && after.next_ticket == before.next_ticket
}

/// Q48 / #806: expose scheduler input facts for one world, before mutation.
pub(super) proof fn advance_input(w: World, i: int)
    requires domain(w), retirement_wide::accepted(w), low(w), storage_ordered(w), 0 <= i < 4,
    ensures scheduling_safe(w), world_headroom(w), advance_pre(w.nodes@[i], w.next_ticket, 0),
{
    reveal(retirement_wide::accepted); reveal(retirement::node_rules);
    reveal(world_headroom); reveal(storage_ordered); reveal_with_fuel(open_prefix, 5);
}

/// Q48 / #806: exact node-local scheduler result, folded before checker composition.
pub(super) proof fn advance_result(before: World, after: World, i: int, changed: bool)
    requires domain(before), retirement_wide::accepted(before), low(before), storage_ordered(before),
        0 <= i < 4, tracked(before.nodes@[i]),
        before.nodes@[i].generation <= 1000, before.nodes@[i].next_seq <= 1000,
        before.nodes@[i].phase is Ready ==> before.nodes@[i].ticket.is_some(),
        after.nodes@ == before.nodes@.update(i, after.nodes@[i]),
        after.gates@ == before.gates@, after.file_sets@ == before.file_sets@,
        after.dependencies@ == before.dependencies@, after.fail_fast == before.fail_fast,
        after.pool_capacity == before.pool_capacity, after.next_question == before.next_question,
        advance_post(before.nodes@[i], after.nodes@[i], before.next_ticket, after.next_ticket, changed),
        progress_gate_leaf_steps::advance_details(before.nodes@[i], after.nodes@[i], before.next_ticket, after.next_ticket),
        before.nodes@[i].phase == Phase::Queued ==> !changed,
        before.nodes@[i].phase is Ready ==> after.nodes@[i] == retirement_gate::slot_node(
            before.nodes@[i], before.next_ticket, retirement_gate::slot(before, i) && retirement_gate::line(before, i))
            && after.next_ticket == retirement_gate::slot_ticket(before.nodes@[i], before.next_ticket),
    ensures frame(before, after, i), after.nodes@[i] == updated(before, i), low(after),
        storage_ordered(after), after.nodes@[i].generation <= before.nodes@[i].generation + 1,
        after.nodes@[i].next_seq <= before.nodes@[i].next_seq + 1,
{
    reveal(updated); reveal(advance_post); reveal(progress_gate_leaf_steps::advance_details);
    reveal(retirement_gate::slot_node); reveal(storage_ordered); reveal(footprint::ordered_table);
    reveal(retirement_wide::accepted); reveal(retirement::node_rules);
    assert forall|j: int| 0 <= j < 4 implies after.nodes@[j].generation <= 1024
        && after.nodes@[j].next_seq <= 1024 && after.nodes@[j].credit.start <= 1024
        && after.nodes@[j].credit.earned <= 1024 && after.nodes@[j].credit.added <= 1024
        && after.nodes@[j].credit.spent <= 1024 by {}
}

/// Q48 / #806: fold the complete production checker from its independent
/// node, pair, capacity and ticket conditions, without exposing another world.
pub(super) proof fn fold_safe(w: World)
    requires domain(w), w.gates@.len() > 0, w.pool_capacity > 0,
        forall|r: int| 0 <= r < w.gates@.len() ==> w.gates@[r].w > 0
            && retirement_wide::holders(w, RepoId(r as u64), 4) <= w.gates@[r].w,
        forall|j: int| 0 <= j < 4 ==> retirement::node_rules(w, w.nodes@[j]),
        forall|a: int, b: int| 0 <= a < b < 4 ==> retirement_wide::pair(w, a, b),
        pool_users(w.nodes@, 4) <= w.pool_capacity,
        forall|a: int, b: int| 0 <= a < 4 && 0 <= b < 4 && a != b ==>
            (w.nodes@[a].ticket.is_some() ==> w.nodes@[a].ticket != w.nodes@[b].ticket),
    ensures retirement_wide::accepted(w),
{
    reveal(retirement_wide::accepted);
}

/// Q48: exact single-node update preserves every production checker condition.
pub(super) proof fn update_safe(before: World, after: World, i: int)
    requires domain(before), retirement_wide::accepted(before), low(before), 0 <= i < 4,
        tracked(before.nodes@[i]), before.nodes@[i].ticket.is_some() || !(before.nodes@[i].phase is Ready),
        frame(before, after, i), after.nodes@[i] == updated(before, i),
    ensures domain(after), retirement_wide::accepted(after),
{
    progress_gate_leaf_preserve::update_node(before, after, i);
    progress_gate_leaf_preserve::update_pairs(before, after, i);
    progress_gate_leaf_preserve::update_capacity(before, after, i);
    progress_gate_leaf_preserve::update_tickets(before, after, i);
    fold_safe(after);
}
}

pub(super) use super::progress_gate_leaf_advance::advance_leaf;
