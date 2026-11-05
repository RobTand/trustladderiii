//! #799/#804: pytest return and finish records are independent facts.

#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Checking,
    Summary,
}

/// #804: only a classified xdist teardown-thread error preserves an emitted finish.
/// The Python edge classifies the exception and traceback; this function sees facts.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == (if !raised { returned } else if xdist_teardown && finished.is_some() {
        finished.unwrap()
    } else { 3 }),
))]
pub fn process_exit(
    returned: i32,
    finished: Option<i32>,
    raised: bool,
    xdist_teardown: bool,
) -> i32 {
    if !raised {
        return returned;
    }
    match finished {
        Some(code) if xdist_teardown => code,
        _ => 3,
    }
}

/// #804: status 3 is refused in both modes even with a matching finish record.
/// Both modes share this gate; their later consistency checks remain separate.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == (finished == Some(exit) && (exit == 0 || exit == 1 || exit == 2 || exit == 4 || exit == 5)),
))]
pub fn consistent(_mode: Mode, finished: Option<i32>, exit: i32) -> bool {
    finished == Some(exit) && matches!(exit, 0 | 1 | 2 | 4 | 5)
}

/// #804/report.parse: only the named outcomes determine failure/exit consistency.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == (!(exit != 0 && all_passed) && !(exit == 0 && any_failure)),
))]
pub fn checking_outcomes(exit: i32, all_passed: bool, any_failure: bool) -> bool {
    !(exit != 0 && all_passed || exit == 0 && any_failure)
}

/// #804/test_tool: interrupted summary needs collection evidence; status 1 needs failures.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == (!(exit == 2 && !collection_error)
        && !(exit == 0 && any_failure) && !(exit == 1 && !any_failure)),
))]
pub fn summary_outcomes(exit: i32, collection_error: bool, any_failure: bool) -> bool {
    !(exit == 2 && !collection_error || exit == 0 && any_failure || exit == 1 && !any_failure)
}
