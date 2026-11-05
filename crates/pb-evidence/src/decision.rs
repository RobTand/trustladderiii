#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

/// #821: the first failed checked acceptance condition owns the refusal.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out <= 3,
    (out == 0) == (ending_ok && receipt && log),
    !ending_ok ==> out == 1,
    ending_ok && !receipt ==> out == 2,
    ending_ok && receipt && !log ==> out == 3,
))]
pub fn ending(ending_ok: bool, receipt: bool, log: bool) -> u8 {
    if !ending_ok {
        1
    } else if !receipt {
        2
    } else if !log {
        3
    } else {
        0
    }
}

/// #821 / Q55: every post-wait combination returns the original first refusal.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out <= 3,
    (out == 0) == ((!completed || !base || proof) && (!completed || !forced || (forcing && selection)) && (!symptom_required || symptom)),
    completed && base && !proof ==> out == 1,
    (!completed || !base || proof) && completed && forced && (!forcing || !selection) ==> out == 2,
    (!completed || !base || proof) && (!completed || !forced || (forcing && selection)) && symptom_required && !symptom ==> out == 3,
))]
#[allow(clippy::too_many_arguments)]
pub fn consistency(
    completed: bool,
    base: bool,
    proof: bool,
    forced: bool,
    forcing: bool,
    selection: bool,
    symptom_required: bool,
    symptom: bool,
) -> u8 {
    if completed && base && !proof {
        1
    } else if completed && forced && (!forcing || !selection) {
        2
    } else if symptom_required && !symptom {
        3
    } else {
        0
    }
}

/// #821 / Q55: an alias or parent-child dependency forbids patch reordering.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == (!same_attribute && !left_parent && !right_parent),
))]
pub fn independent(same_attribute: bool, left_parent: bool, right_parent: bool) -> bool {
    !same_attribute && !left_parent && !right_parent
}

/// #821: all four checked outer-ending fields are necessary; Python scalar decoding is separate.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == (key && status && exit && succeeded),
))]
pub fn header(key: bool, status: bool, exit: bool, succeeded: bool) -> bool {
    key && status && exit && succeeded
}

/// #821: publication authenticity requires both strict verification and its receipt digest.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == (verified && digest),
))]
pub fn receipt(verified: bool, digest: bool) -> bool {
    verified && digest
}

/// #821: neither matching length nor matching digest alone authenticates log bytes.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == (length && digest),
))]
pub fn log(length: bool, digest: bool) -> bool {
    length && digest
}

/// #821: a summary needs every checked shape group, including exact nonnegative totals.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == (object && status && note && totals && rows),
))]
pub fn shape(object: bool, status: bool, note: bool, totals: bool, rows: bool) -> bool {
    object && status && note && totals && rows
}

/// #821: a complete log has one summary, never zero or multiple summaries.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == (count == 1),
))]
pub fn unique(count: u64) -> bool {
    count == 1
}

/// #821: every checked symptom receipt field is necessary.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == (sequence && digest && summary),
))]
pub fn symptom(sequence: bool, digest: bool, summary: bool) -> bool {
    sequence && digest && summary
}
