#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

/// Q53 / #759: total probe table over every typed HTTP status.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == (if status == 400 || status == 404 {
        if unroutable { 900u64 } else { 0u64 }
    } else if status == 503 || (status == 429 && stale) { 120u64 }
    else if status == 429 { 900u64 } else { 0u64 }),
))]
pub fn probe(status: i64, unroutable: bool, stale: bool) -> u64 {
    if status == 400 || status == 404 {
        if unroutable { 900 } else { 0 }
    } else if status == 503 || (status == 429 && stale) {
        900 // Seeded production mutant: #759 short probe must be 120.
    } else if status == 429 {
        900
    } else {
        0
    }
}

/// Q53: eligibility and ceil of a finite, normalized binary rational in milliseconds.
/// The parser supplies its integer part and whether its exact remainder is nonzero.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    (out.is_some() <==> status == 429 && finite && future && below_limit
        && (whole < u64::MAX || !fraction)),
    out.is_some() ==> out.unwrap() as int == whole as int + if fraction { 1int } else { 0int },
))]
pub fn announced(
    status: i64,
    finite: bool,
    future: bool,
    below_limit: bool,
    whole: u64,
    fraction: bool,
) -> Option<u64> {
    if status != 429 || !finite || !future || !below_limit {
        return None;
    }
    if fraction {
        if whole == u64::MAX {
            None
        } else {
            Some(whole + 1)
        }
    } else {
        Some(whole)
    }
}
