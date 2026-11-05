//! Reviewer input decisions. JSON, paths, commit parsing and all reads stay in the shell.
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;
#[cfg(not(verus_keep_ghost))]
macro_rules! scan {
    (($condition:expr) [$($contract:tt)*] $body:block) => { while $condition $body };
}
#[cfg(verus_keep_ghost)]
macro_rules! scan {
    (($condition:expr) [$($contract:tt)*] $body:block) => {
        vstd::prelude::verus_exec_expr! { while $condition $($contract)* $body }
    };
}

pub mod bounds {
    #[cfg(verus_keep_ghost)]
    use vstd::prelude::*;
    /// Preserved existing behaviour (no rule), f5b0ba2f review.rs:84-85, :136-140.
    /// The diff cap is inclusive and counts bytes, before UTF-8 decoding.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
        out == (bytes <= 524288),
    ))]
    pub fn diff_allowed(bytes: usize) -> bool {
        bytes <= 512 * 1024
    }

    /// Preserved existing behaviour (no rule), f5b0ba2f review.rs:159-161 and
    /// findings.rs:87-89. Unicode scalar prefix, not a byte or grapheme bound.
    /// No unwind bound: this arithmetic contract holds for all machine sizes.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
        out <= chars, out <= cap, out == chars || out == cap,
    ))]
    pub fn prefix_len(chars: usize, cap: usize) -> usize {
        if chars < cap { chars } else { cap }
    }
}

pub mod history {
    #[cfg(verus_keep_ghost)]
    use vstd::prelude::*;
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
    #[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
    #[derive(Debug, PartialEq, Eq)]
    pub enum BaseSource {
        Check,
        Reviewed,
        Session,
    }

    /// f5b0ba2f docs/state-table.md:512-514 is re-review context. The precedence
    /// is preserved existing behaviour (no rule), review.rs:42-60 on that base.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
        out == if round_one && session {
            if reviewed { BaseSource::Reviewed } else { BaseSource::Session }
        } else { BaseSource::Check },
    ))]
    pub fn base_source(round_one: bool, session: bool, reviewed: bool) -> BaseSource {
        if round_one && session {
            if reviewed {
                BaseSource::Reviewed
            } else {
                BaseSource::Session
            }
        } else {
            BaseSource::Check
        }
    }

    /// Preserved existing behaviour (no rule), f5b0ba2f review.rs:63-65.
    /// An older reviewed history is reset only after the shell reads ancestry.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
        out == (different && !contains_check_base),
    ))]
    pub fn reset_base(different: bool, contains_check_base: bool) -> bool {
        different && !contains_check_base
    }
}

pub mod selection {
    #[cfg(verus_keep_ghost)]
    use vstd::prelude::*;

    /// Preserved existing behaviour (no rule), f5b0ba2f review.rs:91-130, :179-192.
    /// All matching entries survive, preserving order and duplicate positions.
    /// There is no collection count cap here. #704/#706/#719/#723 receipt
    /// shapes pass through unchanged; this function never rewrites payloads.
    /// Arbitrary-length scan, no bounded unwind or assumed input ordering.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
        out.len() <= flags.len(),
        forall|k: int| 0 <= k < out.len() ==> out[k] < flags.len(),
        forall|j: int| 0 <= j < flags.len() ==> out@.contains(j as usize) == flags[j],
        forall|a: int, b: int| 0 <= a < b < out.len() ==> out[a] < out[b],
    ))]
    pub fn indices(flags: &[bool]) -> Vec<usize> {
        let mut out: Vec<usize> = Vec::new();
        let mut i = 0;
        scan! { (i < flags.len()) [
            invariant i <= flags.len(), out.len() <= i,
                forall|k: int| 0 <= k < out.len() ==> out[k] < i,
                forall|j: int| 0 <= j < i ==> out@.contains(j as usize) == flags[j],
                forall|a: int, b: int| 0 <= a < b < out.len() ==> out[a] < out[b],
            decreases flags.len() - i,
        ] {
            if flags[i] { out.push(i); }
            i += 1;
        } }
        out
    }

    /// #717 is the wire-drift incident, not a new evidence-selection rule.
    /// Preserved existing behaviour (no rule), f5b0ba2f review.rs:147-153.
    /// Presence of a forcing, proof, or declaration key includes null values.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
        out == (forcing || proof || declared_symptom),
    ))]
    pub fn forced(forcing: bool, proof: bool, declared_symptom: bool) -> bool {
        forcing || proof || declared_symptom
    }

    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
    #[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
    #[derive(Debug, PartialEq, Eq)]
    pub enum HeadOutcome {
        Passed,
        Assertion,
        Error,
        Other,
    }

    /// Preserved existing behaviour (no rule), f5b0ba2f review.rs:109-125.
    /// Historical failure displacement requires a matching regression unit
    /// with real head evidence. No-report, filtered and absent outcomes cannot resolve it.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
        out == (regression && same_file && (outcome == HeadOutcome::Passed
            || outcome == HeadOutcome::Assertion || outcome == HeadOutcome::Error)),
    ))]
    pub fn resolves_historical(regression: bool, same_file: bool, outcome: HeadOutcome) -> bool {
        regression
            && same_file
            && matches!(
                outcome,
                HeadOutcome::Passed | HeadOutcome::Assertion | HeadOutcome::Error
            )
    }

    /// Preserved existing behaviour (no rule), f5b0ba2f review.rs:179-189.
    /// #706 confirms reviewer selection is by exact Git tree, not selector
    /// identity. Base runs use check-base tree; every other run uses head tree.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
        out == if base { base_matches } else { head_matches },
    ))]
    pub fn receipt_tree(base: bool, base_matches: bool, head_matches: bool) -> bool {
        if base { base_matches } else { head_matches }
    }
}
