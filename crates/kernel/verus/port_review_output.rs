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

/// State-table notice rows: one exact Notice update preserves every output slot.
pub(super) proof fn notice_append(before: Outbox, after: Outbox, i: usize, notice: Notice)
    requires before.len < OUTBOX,
        after.items@ == before.items@.update(before.len as int,
            Some(Out::Notice { node: NodeId(i as u64), notice })),
    ensures retirement::notices_only(before) ==> retirement::notices_only(after),
{
    reveal(retirement::notices_only);
    if !retirement::notices_only(before) { return; }
    assert(before.items@.len() == OUTBOX);
    assert forall|k: int| 0 <= k < OUTBOX implies match #[trigger] after.items@[k] {
        None | Some(Out::Notice { .. }) => true, _ => false,
    } by {
        if k != before.len {
            assert(after.items@[k] == before.items@[k]);
            assert(match before.items@[k] {
                None | Some(Out::Notice { .. }) => true, _ => false,
            });
        } else {
            assert(after.items@[k] == Some(Out::Notice { node: NodeId(i as u64), notice }));
        }
    }
}

}
