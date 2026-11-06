//! #806 W7 slice 2: real-handler contract consumers for original assertions.
use super::*;

verus! {
/// #806 prerequisite status: a repeated Pending observation preserves its row.
pub(super) fn pending_contract_gap(mut w: World) -> (result: World)
    requires step_scope(w), w.nodes@.len() == 1, w.dependencies@.len() == 1,
        w.dependencies@[0] == (Dependency { node: NodeId(0),
            target: DependencyTarget { kind: PrerequisiteKind::Issue, number: 2761, request: None },
            status: DependencyStatus::Pending, rejected: false }),
    ensures result.dependencies@ == w.dependencies@,
{
    let mut out = Outbox::new();
    proof { reveal(step_scope); reveal(empty_outbox); }
    apply(&mut w, Event::Fact { node: NodeId(0), fact: Fact::Dependency {
        target: DependencyTarget { kind: PrerequisiteKind::Issue, number: 2761, request: None },
        status: DependencyStatus::Pending } }, &mut out);
    w
}

/// #806 prerequisite registration: an already-started request refuses a new key.
pub(super) fn registration_contract_gap(mut w: World) -> (result: World)
    requires step_scope(w), w.nodes@.len() == 1, w.dependencies@.len() == 0,
        w.nodes@[0].planning_started,
    ensures result.dependencies@.len() == 0,
{
    let mut out = Outbox::new();
    proof { reveal(step_scope); reveal(empty_outbox); }
    apply(&mut w, Event::Fact { node: NodeId(0), fact: Fact::Dependency {
        target: DependencyTarget { kind: PrerequisiteKind::Issue, number: 2762, request: None },
        status: DependencyStatus::Pending } }, &mut out);
    w
}

/// #806 prerequisite removal: real compaction retains the unrelated terminal row.
pub(super) fn removal_contract_gap(mut w: World) -> (result: World)
    requires step_scope(w), w.nodes@.len() == 1, w.dependencies@.len() == 2,
        w.nodes@[0].phase == Phase::Ingesting, !w.nodes@[0].planning_started,
        w.dependencies@[0].node == NodeId(0),
        w.dependencies@[0].target.kind == PrerequisiteKind::Issue,
        w.dependencies@[0].target.number == 2761,
        w.dependencies@[1].target.number == 2762,
    ensures result.dependencies@ == seq![w.dependencies@[1]],
{
    let mut out = Outbox::new();
    proof { reveal(step_scope); reveal(empty_outbox); }
    apply(&mut w, Event::Fact { node: NodeId(0), fact: Fact::DependencyRemove {
        kind: PrerequisiteKind::Issue, number: 2761 } }, &mut out);
    w
}

/// #806 adoption: the original matching first preparation adopts the proposed head.
pub(super) fn adoption_contract_gap(mut w: World) -> (result: World)
    requires step_scope(w), w.nodes@.len() == 2,
        w.nodes@[0].phase == Phase::Making, w.nodes@[0].pos == 0,
        !w.nodes@[0].adopted, w.nodes@[0].proposed.is_some(),
        w.nodes@[0].effect.is_some(),
        w.nodes@[0].effect.unwrap().kind == EffectKind::PrepareWorktree,
        !w.nodes@[0].effect.unwrap().cancelling,
    ensures result.nodes@[0].adopted,
        result.nodes@[0].head == w.nodes@[0].proposed,
        result.nodes@[0].phase == Phase::Checking,
{
    let seq = w.nodes[0].effect.unwrap().seq;
    let mut out = Outbox::new();
    proof { reveal(step_scope); reveal(world_headroom); reveal(empty_outbox); }
    on_ack(&mut w, 0, seq, Ack::Confirmed, &mut out);
    w
}
}
