//! K4c: settle output composition over the real outbox drain. `drain_into`
//! appends `occupied(items, OUTBOX)`, the whole buffer with None dropped. These
//! lemmas relate that full drain to the written prefix without assuming a None
//! tail: each consumer proves the tail from the actual outbox writes.
use super::*;

verus! {
/// Draining only reads slots below `end`: buffers that agree there drain alike.
pub(super) proof fn occupied_prefix(a: Seq<Option<Out>>, b: Seq<Option<Out>>, end: nat)
    requires end <= a.len(), end <= b.len(),
        forall|k: int| 0 <= k < end ==> a[k] == b[k],
    ensures occupied(a, end) == occupied(b, end),
    decreases end,
{
    if end > 0 { occupied_prefix(a, b, (end - 1) as nat); }
}

/// No omission past the written prefix: empty slots from `len` to `end` add nothing.
pub(super) proof fn occupied_tail(items: Seq<Option<Out>>, len: nat, end: nat)
    requires len <= end <= items.len(),
        forall|k: int| len <= k < end ==> items[k].is_none(),
    ensures occupied(items, end) == occupied(items, len),
    decreases end,
{
    if end > len { occupied_tail(items, len, (end - 1) as nat); }
}

/// Order and completeness: when every slot below `len` is written, the drain is
/// exactly those outputs, in slot order, none omitted or duplicated.
pub(super) proof fn occupied_full(items: Seq<Option<Out>>, len: nat)
    requires len <= items.len(),
        forall|k: int| 0 <= k < len ==> items[k].is_some(),
    ensures occupied(items, len).len() == len,
        forall|k: int| 0 <= k < len ==> occupied(items, len)[k] == items[k].unwrap(),
    decreases len,
{
    if len > 0 {
        occupied_full(items, (len - 1) as nat);
        assert forall|k: int| 0 <= k < len implies occupied(items, len)[k] == items[k].unwrap() by {
            if k < len - 1 { assert(occupied(items, len)[k] == occupied(items, (len - 1) as nat)[k]); }
        }
    }
}

/// A fresh outbox drains to nothing.
pub(super) proof fn empty_drains_nothing(out: Outbox)
    requires empty_outbox(out),
    ensures occupied(out.items@, OUTBOX as nat) == Seq::<Out>::empty(),
{
    reveal(empty_outbox);
    occupied_tail(out.items@, 0, OUTBOX as nat);
}

/// The real tail invariant, carried by each write: an append at `len` keeps
/// every slot from the new length on empty.
pub(super) proof fn push_keeps_tail(o0: Outbox, o1: Outbox, x: Out)
    requires o0.len < OUTBOX, o0.items@.len() == OUTBOX,
        o1.len == o0.len + 1, o1.items@ == o0.items@.update(o0.len as int, Some(x)),
        forall|k: int| o0.len <= k < OUTBOX ==> o0.items@[k].is_none(),
    ensures forall|k: int| o1.len <= k < OUTBOX ==> o1.items@[k].is_none(),
        forall|k: int| 0 <= k < o0.len ==> o1.items@[k] == o0.items@[k],
        o1.items@[o0.len as int] == Some(x),
{
}

/// Drains of consecutive output sequences concatenate: dropping None commutes
/// with sequence concatenation, so per-pass drains compose like the relation's
/// per-pass emitted suffixes.
pub(super) proof fn occupied_concat(a: Seq<Option<Out>>, b: Seq<Option<Out>>)
    ensures occupied(a + b, (a.len() + b.len()) as nat)
        == occupied(a, a.len()) + occupied(b, b.len()),
    decreases b.len(),
{
    if b.len() == 0 {
        assert(a + b =~= a);
        occupied_prefix(a + b, a, a.len());
        assert(occupied(b, b.len()) =~= Seq::<Out>::empty());
        assert(occupied(a, a.len()) + occupied(b, b.len()) =~= occupied(a, a.len()));
    } else {
        let b0 = b.drop_last();
        occupied_concat(a, b0);
        assert((a + b).drop_last() =~= a + b0);
        occupied_prefix(a + b, a + b0, (a.len() + b0.len()) as nat);
        occupied_prefix(b, b0, b0.len());
        assert((a + b)[(a.len() + b.len()) - 1] == b[b.len() - 1]);
        match b[b.len() - 1] {
            Some(o) => {
                assert(occupied(a, a.len()) + occupied(b0, b0.len()).push(o)
                    =~= (occupied(a, a.len()) + occupied(b0, b0.len())).push(o));
            }
            None => {}
        }
    }
}

/// The real drain of one settle pass is its emitted suffix with None dropped.
/// The pass starts from a fresh outbox, and the empty tail from `len` on is a
/// premise each caller proves from the actual writes (new, push, I-D2).
pub(super) proof fn drain_is_emitted(fresh: Outbox, o: Outbox)
    requires empty_outbox(fresh), o.items@.len() == OUTBOX, o.len <= OUTBOX,
        forall|k: int| o.len <= k < OUTBOX ==> o.items@[k].is_none(),
    ensures fresh.len == 0,
        occupied(o.items@, OUTBOX as nat)
            == occupied(transition_safety::emitted(fresh, o), o.len as nat),
{
    reveal(empty_outbox);
    let e = transition_safety::emitted(fresh, o);
    assert(e =~= o.items@.subrange(0, o.len as int));
    occupied_tail(o.items@, o.len as nat, OUTBOX as nat);
    occupied_prefix(o.items@, e, o.len as nat);
}

/// Settle's Vec suffix after one more pass: if the Vec so far is the prefix plus
/// the relation's emitted sequence with None dropped, appending the pass's drain
/// keeps that correspondence for the concatenated relation output.
pub(super) proof fn vec_suffix_step(prefix: Seq<Out>, vec: Seq<Out>, rel: Seq<Option<Out>>,
    next: Seq<Option<Out>>, drained: Seq<Out>)
    requires vec == prefix + occupied(rel, rel.len()),
        drained == occupied(next, next.len()),
    ensures vec + drained == prefix + occupied(rel + next, (rel + next).len()),
{
    occupied_concat(rel, next);
    assert(vec + drained =~= prefix + (occupied(rel, rel.len()) + occupied(next, next.len())));
}
}
