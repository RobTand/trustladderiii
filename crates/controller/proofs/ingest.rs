#[path = "../../contracts/src/roles/ingest_retention.rs"]
mod decision;
use decision::*;
use vstd::prelude::*;
verus! {
/// Ingest/Audit/#840: all Boolean inputs, no bounded string or source assumptions.
fn retention_total(audit: bool, complete: bool, fits: bool) {
    let actual = decide(audit, complete, fits);
    assert((actual == Action::Keep) == complete);
    assert((actual == Action::Object) == (!complete && audit));
    assert((actual == Action::Append) == (!complete && !audit && fits));
    assert((actual == Action::Repair) == (!complete && !audit && !fits));
}
/// #840: each production ending has an explicit reachable input, four cases suffice.
fn retention_witness(case: u8) -> (result: bool)
    requires case < 4,
    ensures result,
{
    match case {
        0 => decide(false, true, false) == Action::Keep,
        1 => decide(true, false, false) == Action::Object,
        2 => decide(false, false, true) == Action::Append,
        _ => decide(false, false, false) == Action::Repair,
    }
}
}
fn main() {}
