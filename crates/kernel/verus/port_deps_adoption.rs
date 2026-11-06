//! #806 W7: exact adoption preparation frames over the real acknowledgement path.
use super::*;

verus! {
/// Q adoption / #806: the unchanged PrepareWorktree agenda branch.
#[verifier::opaque]
pub(super) open spec fn agenda(n: Node) -> Node {
    if !n.adopted && n.proposed.is_some() {
        retirement::phased(Node { adopted: true, head: n.proposed, checked: None, ..n }, Phase::Checking)
    } else { Node { pos: 1, ..n } }
}

/// Q adoption / #806: confirmed preparation resets delivery failures first.
#[verifier::opaque]
pub(super) open spec fn confirmed(n: Node) -> Node {
    agenda(Node { fx_failures: 0, fx_incidents: 0, ..n })
}

/// Q adoption / #806: the real matching acknowledgement clears its effect first.
#[verifier::opaque]
pub(super) open spec fn acknowledged(n: Node) -> Node {
    confirmed(Node { effect: None, ..n })
}

/// #806 adoption: exact local result, isolated from the inherited acknowledgement progress proof.
#[verifier::opaque]
pub(super) open spec fn ack_post(n: Node, m: Node, seq: u64, ack: Ack,
    out: Outbox, next: Outbox, question: u64, next_question: u64) -> bool {
    preparing(n) && seq == n.effect.unwrap().seq && ack == Ack::Confirmed
        ==> m == acknowledged(n) && next == out && next_question == question
}

/// #806 original adoption assumptions, without small-head or counter bounds.
pub(super) open spec fn preparing(n: Node) -> bool {
    n.phase == Phase::Making && n.pos == 0 && n.proposed.is_some()
        && n.effect.is_some() && n.effect.unwrap().kind == EffectKind::PrepareWorktree
        && !n.effect.unwrap().cancelling
}
}
