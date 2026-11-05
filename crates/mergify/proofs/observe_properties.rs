//! #743: composition assertions from the retired observe harnesses.
use super::check::Conclusion;
use super::observe::{Mergeable, PullView, QueueState, check_state, ci_is_infrastructure,
    mergeability_pending, merged_via_queue, queue_state};
use super::check::{CheckView, Status};

#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {
/// #743/K-observe-conflict: pending after classification implies unknown mergeability.
fn conflict(pull: PullView, report: Option<QueueState>) {
    let state = queue_state(pull, decide_report!(Option<QueueState>; report));
    assert(state == super::observe::queue_result(pull, report));
    if !pull.merged && report == Some(QueueState::Dequeued) {
        assert(state == if pull.mergeable == Some(false) || pull.mergeable_state == Mergeable::Dirty {
            QueueState::Conflict
        } else { QueueState::Dequeued });
    }
    if !pull.merged && !matches!(report, Some(QueueState::Merged | QueueState::Dequeued)) {
        assert(state == report.unwrap_or(QueueState::Unknown));
    }
    if mergeability_pending(state, pull) {
        assert(pull.mergeable != Some(false));
        assert(pull.mergeable != Some(true));
        match pull.mergeable {
            Some(value) => { assert(value == true || value == false); assert(false); },
            None => {},
        }
        assert(pull.mergeable.is_none());
        assert(matches!(pull.mergeable_state, Mergeable::Missing | Mergeable::Unknown));
    }
}
}

/// #743/K-observe-state: exact report and merge evidence over every typed input.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures result,))]
fn state(conclusion: Option<Conclusion>, report: QueueState, infra: bool, check: CheckView) -> bool {
    let fails = matches!(conclusion, Some(Conclusion::TimedOut | Conclusion::StartupFailure));
    let state = check_state(conclusion, decide_report!(QueueState; report));
    let report_state = check_state(check.conclusion, decide_report!(QueueState; report));
    let via = merged_via_queue(&check, decide_report!(QueueState; report_state));
    state == if fails { QueueState::FailedCi } else { report }
        && ci_is_infrastructure(conclusion, decide_report!(bool; infra)) == (fails || infra)
        && via == (check.status == Status::Completed
            && check.conclusion == Some(Conclusion::Success) && report == QueueState::Merged)
}
