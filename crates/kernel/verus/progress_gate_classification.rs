//! Q30/Q39 / #806: exact outside-event classification for mixed gate worlds.
use super::progress_gate::*;
use super::*;

verus! {
/// #806: the no-witness classification includes every original assertion.
pub(super) open spec fn waiting(w: World) -> bool {
    forall|i: int| 0 <= i < 4 ==> !w.nodes@[i].paused && !w.nodes@[i].holds
        && !w.nodes@[i].pool && w.nodes@[i].run.is_none()
        && (w.nodes@[i].phase is Ready || w.nodes@[i].phase == Phase::Unplanned)
}

/// #806: exact actor, generation, head and fresh Resume key; every other event is excluded.
pub(super) open spec fn classified(w: World, event: Event) -> bool {
    match event {
        Event::Reply { node, generation, reply: Reply::MakerDone { head } } =>
            node.0 < 4 && w.nodes@[node.0 as int].phase == Phase::Making
                && !w.nodes@[node.0 as int].paused && w.nodes@[node.0 as int].run == Some(generation)
                && head == match w.nodes@[node.0 as int].head { Some(h) => h, None => HeadId(0) },
        Event::Fact { node, fact: Fact::PrMerged { head, checks_passed } } =>
            node.0 < 4 && w.nodes@[node.0 as int].phase == Phase::Queued
                && !w.nodes@[node.0 as int].paused && checks_passed
                && head == match w.nodes@[node.0 as int].head { Some(h) => h, None => HeadId(0) },
        Event::Command { node, id, command: Command::Resume } =>
            node.0 < 4 && w.nodes@[node.0 as int].paused
                && id as int == w.nodes@[node.0 as int].last_cmd as int + 1,
        _ => false,
    }
}

/// #806: classification follows #805's exact selection contract of real witness.
pub(super) proof fn classify(w: World, event: Option<Event>)
    requires input(w), witness_selection(w, event),
    ensures match event { Some(e) => classified(w, e), None => waiting(w) },
{
    reveal(initial_node); reveal(witness_selection); reveal(chosen_for_node);
    reveal(outside_available);
    match event {
        None => {
            assert forall|i: int| 0 <= i < 4 implies !w.nodes@[i].paused && !w.nodes@[i].holds
                && !w.nodes@[i].pool && w.nodes@[i].run.is_none()
                && (w.nodes@[i].phase is Ready || w.nodes@[i].phase == Phase::Unplanned) by {
                assert(!outside_available(w, i));
            }
        },
        Some(e) => {
            let i = choose|i: int| 0 <= i < w.nodes@.len() && chosen_for_node(w, i, e);
            assert(classified(w, e));
        },
    }
}
}

kernel_fn! {
/// #806: run the actual selector and retain both the witness and no-witness assertions.
pub(super) fn verify_classification(w: &World) -> (event: Option<Event>) [
    requires input(*w),
    ensures match event { Some(e) => classified(*w, e), None => waiting(*w) },
] {
    kernel_proof! { input_scope(*w); }
    verify_checked(w);
    let event = w.witness();
    kernel_proof! { classify(*w, event); }
    event
}
}

kernel_fn! {
/// #806: compose exact classification with the real actor relocation used by Kani.
pub(super) fn verify_progress(w: World) -> (result: (World, Option<Event>)) [
    requires input(w),
    ensures progress_measure(w) <= 8000, input(result.0), progress_measure(result.0) == progress_measure(w),
        match result.1 {
            None => waiting(result.0),
            Some(event) => classified(result.0, event) && match event {
                Event::Reply { node, .. } | Event::Fact { node, .. } | Event::Command { node, .. } => node == NodeId(0),
                _ => false,
            },
        },
] {
    kernel_proof! { input_measure_bound(w); }
    let selected = verify_classification(&w);
    let event = match selected { Some(e) => e, None => { return (w, None); } };
    let (actor, moved) = match event {
        Event::Reply { node, generation, reply } =>
            (node, Event::Reply { node: NodeId(0), generation, reply }),
        Event::Fact { node, fact } => (node, Event::Fact { node: NodeId(0), fact }),
        Event::Command { node, id, command } =>
            (node, Event::Command { node: NodeId(0), id, command }),
        _ => { kernel_proof! { assert(false); } return (w, None); },
    };
    let after = progress_gate_permutation::swap_actor(w, actor.0 as usize);
    (after, Some(moved))
}
}
