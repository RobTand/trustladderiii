#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

/// #821: the first failed checked acceptance condition owns the refusal.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out <= 3,
    (out == 0) == (ending && receipt && log),
    !ending ==> out == 1,
    ending && !receipt ==> out == 2,
    ending && receipt && !log ==> out == 3,
))]
pub fn ending(ending: bool, receipt: bool, log: bool) -> u8 {
    if !ending {
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
