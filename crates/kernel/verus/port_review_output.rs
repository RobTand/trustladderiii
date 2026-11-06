//! #806: exact output composition without widening the allocation queries.
use super::*;

verus! {
/// State-table publishing rows: only effects naming a commit carry the head.
#[verifier::opaque]
pub(super) open spec fn selected_head(n: Node, kind: EffectKind) -> Option<HeadId> {
    if matches!(kind, EffectKind::Push | EffectKind::PostApproval
        | EffectKind::OpenPr | EffectKind::Enqueue) { n.head } else { None }
}

/// #806: exact emitted output slot, hidden from unrelated node-frame obligations.
#[verifier::opaque]
pub(super) open spec fn effect_post(before: Seq<Option<Out>>, before_len: usize,
    after: Seq<Option<Out>>, n: Node, i: usize, kind: EffectKind) -> bool {
    after == before.update(before_len as int,
        Some(Out::Effect { node: NodeId(i as u64), seq: n.next_seq, kind,
            head: selected_head(n, kind) }))
}

}
