//! #743: original-domain reachability witnesses, also run as Rust tests.
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

use super::check::{App, CheckView, Conclusion, Head, Name, Status, Time};
use super::facts::{blocking_failure, checks_passed, checks_settled, queue_blocked};
use super::observe::{
    Mergeable, PullView, QueueState, check_state, ci_is_infrastructure, mergeability_pending,
    merged_via_queue, queue_state, rule_concluded, select,
};

/// #743: concrete typed check shared by the original bounded witnesses.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result =>
    ensures result == (CheckView { id: 1, head: Head::Current, app: App::Mergify,
        app_key: 1, name: Name::QueueCheck, name_key: 1, required: false,
        status: Status::Completed, conclusion: Some(Conclusion::Success),
        completed_at: Some(Time { secs: 1, nanos: 0 }) }),
))]
fn check() -> CheckView {
    CheckView {
        id: 1,
        head: Head::Current,
        app: App::Mergify,
        app_key: 1,
        name: Name::QueueCheck,
        name_key: 1,
        required: false,
        status: Status::Completed,
        conclusion: Some(Conclusion::Success),
        completed_at: Some(Time { secs: 1, nanos: 0 }),
    }
}

/// #743/K-observe-select: all four covers use exactly the original three checks.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures result,))]
fn select_covers() -> bool {
    let c = check();
    let outsider = CheckView {
        head: Head::Other,
        id: 2,
        ..c
    };
    let impostor = CheckView {
        app: App::MergifySlug,
        id: 3,
        ..c
    };
    select(&[outsider, outsider, impostor], Name::QueueCheck).is_none()
        && select(&[c, c, outsider], Name::QueueCheck) == Some(1)
        && select(&[c, outsider, outsider], Name::QueueCheck) == Some(0)
        && select(&[c, outsider, impostor], Name::QueueCheck) == Some(0)
}

/// #743/K-observe-merged: recorded merge, delayed merge, and absent report.
/// The merged callback has a false call precondition. The outer world is
/// executable; verification of its real path must prove that no call occurs.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures result,))]
fn merged_covers() -> bool {
    let p = PullView {
        merged: true,
        mergeable: None,
        mergeable_state: Mergeable::Missing,
    };
    let open = PullView { merged: false, ..p };
    queue_state(
        p,
        decide_report!(unread Option<QueueState>; Some(QueueState::FailedCi)),
    ) == QueueState::Merged
        && queue_state(
            open,
            decide_report!(Option<QueueState>; Some(QueueState::Merged)),
        ) == QueueState::Unknown
        && queue_state(open, decide_report!(Option<QueueState>; None)) == QueueState::Unknown
}

/// #743/K-observe-conflict: dirty despite true, false, plain, and pending.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures result,))]
fn conflict_covers() -> bool {
    let dirty = PullView {
        merged: false,
        mergeable: Some(true),
        mergeable_state: Mergeable::Dirty,
    };
    let refused = PullView {
        mergeable: Some(false),
        mergeable_state: Mergeable::Other,
        ..dirty
    };
    let plain = PullView {
        mergeable: Some(true),
        mergeable_state: Mergeable::Other,
        ..dirty
    };
    let pending = PullView {
        mergeable: None,
        mergeable_state: Mergeable::Missing,
        ..dirty
    };
    queue_state(
        dirty,
        decide_report!(Option<QueueState>; Some(QueueState::Dequeued)),
    ) == QueueState::Conflict
        && queue_state(
            refused,
            decide_report!(Option<QueueState>; Some(QueueState::Dequeued)),
        ) == QueueState::Conflict
        && queue_state(
            plain,
            decide_report!(Option<QueueState>; Some(QueueState::Dequeued)),
        ) == QueueState::Dequeued
        && mergeability_pending(
            queue_state(
                pending,
                decide_report!(Option<QueueState>; Some(QueueState::Dequeued)),
            ),
            pending,
        )
}

/// #743/K-observe-rule: conclusion, equal time without conclusion, earlier rule.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures result,))]
fn rule_covers() -> bool {
    let q = check();
    let rule = CheckView {
        name: Name::QueueRule,
        ..q
    };
    let pending = CheckView {
        conclusion: None,
        ..rule
    };
    let early = CheckView {
        completed_at: Some(Time { secs: 0, nanos: 0 }),
        ..rule
    };
    rule_concluded(&[q, rule], Some(0))
        && !rule_concluded(&[q, pending], Some(0))
        && !rule_concluded(&[q, early], Some(0))
}

/// #743/K-observe-state: timeout, ordinary report, queue merge, unsuccessful merge.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures result,))]
fn state_covers() -> bool {
    let c = check();
    let failed = CheckView {
        conclusion: Some(Conclusion::Failure),
        ..c
    };
    check_state(
        Some(Conclusion::TimedOut),
        decide_report!(unread QueueState; QueueState::Queued),
    ) == QueueState::FailedCi
        && check_state(None, decide_report!(QueueState; QueueState::Conflict))
            == QueueState::Conflict
        && ci_is_infrastructure(
            Some(Conclusion::StartupFailure),
            decide_report!(bool; false),
        )
        && merged_via_queue(&c, decide_report!(QueueState; QueueState::Merged))
        && !merged_via_queue(&failed, decide_report!(QueueState; QueueState::Merged))
}

/// #743/K-checks-passed: superseding pass, distinct app, absent head, two groups.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures result,))]
fn passed_covers() -> bool {
    let good = check();
    let bad = CheckView {
        id: 0,
        conclusion: Some(Conclusion::Failure),
        ..good
    };
    let another_app = CheckView {
        app_key: 2,
        conclusion: Some(Conclusion::Failure),
        ..good
    };
    let another_group = CheckView {
        name_key: 2,
        ..good
    };
    let outsider = CheckView {
        head: Head::Other,
        ..good
    };
    let superseded = [bad, good];
    let apps = [good, another_app];
    let absent = [outsider];
    let two = [good, another_group];
    decide_proof! {
        assert(super::check::kept(superseded@, 1, true));
        assert(exists|i: int| super::check::kept(superseded@, i, true));
        assert forall|i: int| super::check::kept(superseded@, i, true)
            implies super::facts::passing(superseded@[i]) by { assert(i == 0 || i == 1); }
        assert(super::check::kept(apps@, 1, true));
        assert(super::check::kept(two@, 0, true));
        assert(exists|i: int| super::check::kept(two@, i, true));
        assert forall|i: int| super::check::kept(two@, i, true)
            implies super::facts::passing(two@[i]) by { assert(i == 0 || i == 1); }
    }
    checks_passed(&superseded)
        && !checks_passed(&apps)
        && !checks_passed(&absent)
        && checks_passed(&two)
}

/// #447/K-queue-blocked: two failures, required, superseded, running, Mergify slug.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures result,))]
fn blocked_covers() -> bool {
    let bad = CheckView {
        app: App::Actions,
        conclusion: Some(Conclusion::Failure),
        ..check()
    };
    let second = CheckView { name_key: 2, ..bad };
    let required = CheckView {
        required: true,
        ..bad
    };
    let newer = CheckView {
        id: 2,
        conclusion: Some(Conclusion::Success),
        ..bad
    };
    let running = CheckView {
        status: Status::InProgress,
        ..bad
    };
    let slug = CheckView {
        app: App::MergifySlug,
        ..running
    };
    blocking_failure(&[bad, second], 0)
        && blocking_failure(&[bad, second], 1)
        && !blocking_failure(&[required], 0)
        && !blocking_failure(&[bad, newer], 0)
        && !blocking_failure(&[bad, newer], 1)
        && !checks_settled(&[running])
        && checks_settled(&[slug])
        && queue_blocked(QueueState::Waiting, true, true)
}

#[cfg(test)]
mod tests {
    #[test]
    fn observe_select() {
        assert!(super::select_covers());
    }
    #[test]
    fn observe_merged() {
        assert!(super::merged_covers());
    }
    #[test]
    fn observe_conflict() {
        assert!(super::conflict_covers());
    }
    #[test]
    fn observe_rule_concluded() {
        assert!(super::rule_covers());
    }
    #[test]
    fn observe_check_state() {
        assert!(super::state_covers());
    }
    #[test]
    fn checks_passed_total() {
        assert!(super::passed_covers());
    }
    #[test]
    fn queue_blocked_total() {
        assert!(super::blocked_covers());
    }

    /// #743: the original Kani mutable-capture assertions also run on the real functions.
    #[test]
    fn lazy_reports() {
        use super::*;
        for merged in [false, true] {
            let p = PullView {
                merged,
                mergeable: None,
                mergeable_state: Mergeable::Missing,
            };
            let mut read = false;
            queue_state(p, || {
                read = true;
                None
            });
            assert_ne!(read, merged);
        }
        for conclusion in [
            None,
            Some(Conclusion::TimedOut),
            Some(Conclusion::StartupFailure),
            Some(Conclusion::Success),
            Some(Conclusion::Failure),
            Some(Conclusion::Neutral),
            Some(Conclusion::Cancelled),
            Some(Conclusion::Skipped),
            Some(Conclusion::ActionRequired),
            Some(Conclusion::Stale),
            Some(Conclusion::Other),
        ] {
            let fails = matches!(
                conclusion,
                Some(Conclusion::TimedOut | Conclusion::StartupFailure)
            );
            let mut read = false;
            check_state(conclusion, || {
                read = true;
                QueueState::Queued
            });
            assert_ne!(read, fails);
        }
    }
}
