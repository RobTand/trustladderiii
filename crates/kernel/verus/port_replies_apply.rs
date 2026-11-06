//! #806: source-connected worker-reply safety at the real apply boundary.
use super::retirement::*;
use super::*;

kernel_fn! {
/// Protected-path reply row / #806: an admitted reply stops without spending,
/// asking a question, or retrying the plan. One real apply; no unwind.
pub(super) fn protected_paths(w: World, generation: u64) -> (result: World) [
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
] {
    let (after, _) = port_replies_exec::verify_reply_apply(w, generation, Reply::ProtectedPaths);
    after
}
}
