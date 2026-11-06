//! #806: source-connected worker-reply safety at the real apply boundary.
use super::retirement::*;
use super::*;

kernel_fn! {
/// Protected-path reply row / #806: an admitted reply stops without spending,
/// asking a question, or retrying the plan. One real apply; no unwind.
pub(super) fn protected_paths(mut w: World, generation: u64) -> (result: World) [
    requires domain(w), accepted(w), bounded(w), storage_ordered(w),
    ensures ({
        let b = w.nodes@[0];
        let n = result.nodes@[0];
        matches!(b.phase, Phase::Ingesting | Phase::Auditing | Phase::Planning { .. })
            && current_item(b) == Some(Item::Await) && b.run == Some(generation)
            ==> n.phase == Phase::Stopping(Outcome::NotActionable)
                && n.credit == b.credit && n.question.is_none()
                && n.plan_retries == b.plan_retries
    }),
] proof [
    reveal(world_headroom); reveal_with_fuel(open_prefix, 3); reveal(storage_ordered);
    reveal(empty_outbox); reveal(accepted); reveal(node_rules); reveal(bounded);
] {
    kernel_proof! {
        open_prefix_monotone(w.nodes@, 0, w.nodes@.len());
        assert forall|j: int| 0 <= j < w.nodes@.len() implies node_headroom(w.nodes@[j]) by {}
        dependencies::indexable_definition(w);
    }
    let mut out = Outbox::new();
    apply(&mut w, Event::Reply { node: NodeId(0), generation, reply: Reply::ProtectedPaths }, &mut out);
    w
}
}
