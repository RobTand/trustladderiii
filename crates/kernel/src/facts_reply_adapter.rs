//! Resolve canonical reply work into one node and its exact optional notice.
use super::*;

#[cfg(verus_keep_ghost)]
verus! {
pub(super) open spec fn reply_items(nq: u64, change: ReplyChange,
    i: usize) -> Seq<Option<Out>> {
    Seq::new(OUTBOX as nat, |k: int| if k == 0 {
        match reply_family_values::item(nq, change) {
            Some(note) => Some(Out::Notice { node: NodeId(i as u64), notice: note }),
            None => None,
        }
    } else { None })
}
}

kernel_fn! {
/// Resolve deferred work through the shared Ask and Incident implementations.
pub(super) fn resolve_reply_value(change: ReplyChange, nq: u64, i: usize)
    -> (result: (Node, u64, Outbox)) [
    requires reply_family_values::callable(change),
        (change is Ask ==> nq < u64::MAX),
    ensures result.0 == reply_family_values::finish(nq, change),
        result.1 == reply_family_values::next_question(nq, change),
        result.2.len == match reply_family_values::item(nq, change) {
            Some(_) => 1usize, None => 0usize,
        },
        result.2.items@ =~= reply_items(nq, change, i),
] {
    let result = match change {
        ReplyChange::Done { node, note } => {
            let mut notes = Outbox::new();
            kernel_proof! { reveal(empty_outbox); }
            match note {
                Some(value) => notice(&mut notes, i, value),
                None => {},
            }
            (node, nq, notes)
        },
        ReplyChange::Ask { node, kind, cont, cost } => {
            ask_value(node, nq, i, kind, cont, cost)
        },
        ReplyChange::Incident { node, needs_rob } => {
            let (node, notes) = (node, Outbox::new());
            (node, nq, notes)
        },
    };
    kernel_proof! {
        assert(result.2.items@ =~= reply_items(nq, change, i));
    }
    result
}
}
