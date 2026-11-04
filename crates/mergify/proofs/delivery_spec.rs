use vstd::prelude::*;
use crate::delivery_step::step;

verus! {
/// State-table "Effects": duplicate acknowledgements leave a final receipt final.
/// Unbounded induction step over both Boolean inputs, with no sequence bound.
fn acknowledgement_is_idempotent(delivered: bool, acknowledgement: bool) {
    let once = step(delivered, acknowledgement);
    let twice = step(once, acknowledgement);
    assert(twice == once);
}
}
