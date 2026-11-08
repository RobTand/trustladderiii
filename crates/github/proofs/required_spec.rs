use crate::required::*;
use vstd::prelude::*;

verus! {
/// Q52, #447, #2705; cdacce8f docs/state-table.md:542-545,555,578.
/// Concrete non-vacuity checks for head selection, ties, empty lists and gates.
fn selection_and_reads_witnesses() {
    let items = [Candidate {id: 9, eligible: true}, Candidate {id: 10, eligible: false}, Candidate {id: 9, eligible: true}];
    let witness_0 = latest(&items, false);
    assert(witness_0 == Some(0));
    let witness_1 = latest(&items, true);
    assert(witness_1 == Some(2));
    let witness_2 = latest(&[], false);
    assert(witness_2.is_none());
    let newer = [Candidate {id: 1, eligible: true}, Candidate {id: 2, eligible: true}];
    let witness_3 = latest(&newer, false);
    assert(witness_3 == Some(1));
    let witness_4 = needs_checks(false);
    assert(witness_4); let witness_5 = needs_checks(true);
    assert(!witness_5);
    let witness_6 = needs_statuses(&[Some(1), None]);
    assert(witness_6);
    let witness_7 = needs_statuses(&[Some(1)]);
    assert(!witness_7); let witness_8 = needs_statuses(&[]);
    assert(!witness_8);
    let witness_9 = status_read(ReadResult::Http(403));
    assert(witness_9 == StatusRead::Unavailable);
    let witness_10 = status_read(ReadResult::Http(404));
    assert(witness_10 == StatusRead::Unavailable);
    let witness_11 = status_read(ReadResult::Http(500));
    assert(witness_11 == StatusRead::Propagate);
    let witness_12 = status_read(ReadResult::Other);
    assert(witness_12 == StatusRead::Propagate);
    let witness_13 = status_read(ReadResult::Success);
    assert(witness_13 == StatusRead::Available);
    let witness_14 = rule_matches(true, None, Some(1));
    assert(witness_14);
    let witness_15 = rule_matches(true, Some(1), Some(1));
    assert(witness_15);
    let witness_16 = rule_matches(true, Some(1), None);
    assert(!witness_16);
    let witness_17 = rule_matches(true, Some(1), Some(2));
    assert(!witness_17);
    let witness_18 = rule_matches(false, None, None);
    assert(!witness_18);
}

/// Q52, #447, #2705; cdacce8f docs/state-table.md:541-542,578.
/// Every annotation class is reachable; mixed failures must stay test failures.
fn annotation_witnesses() {
    let witness_19 = needs_annotations(true, true, true);
    assert(witness_19);
    let witness_20 = needs_annotations(false, true, true);
    assert(!witness_20);
    let witness_21 = needs_annotations(true, false, true);
    assert(!witness_21);
    let witness_22 = needs_annotations(true, true, false);
    assert(!witness_22);
    let witness_23 = runner_failure(&[Annotation::Ignore, Annotation::RunnerFailure]);
    assert(witness_23);
    let witness_24 = runner_failure(&[]);
    assert(!witness_24);
    let witness_25 = runner_failure(&[Annotation::Ignore]);
    assert(!witness_25);
    let witness_26 = runner_failure(&[Annotation::RunnerFailure, Annotation::OtherFailure]);
    assert(!witness_26);
}

/// Q52, #447, #2705; cdacce8f docs/state-table.md:541-545,555,578.
/// All aggregate outcomes and classic-status classes have concrete witnesses.
fn aggregation_witnesses() {
    let start = Summary { ready: true, failure: Failure::None };
    let passed = record(start, Contribution::Passed);
    assert(passed.ready && passed.failure == Failure::None);
    let pending = record(start, Contribution::Pending);
    assert(!pending.ready && pending.failure == Failure::None);
    let infra = record(start, Contribution::Infrastructure);
    assert(!infra.ready && infra.failure == Failure::Infrastructure);
    let tests = record(start, Contribution::Tests);
    assert(!tests.ready && tests.failure == Failure::Tests);
    let witness_27 = record(infra, Contribution::Tests);
    assert(witness_27.failure == Failure::Tests);
    let witness_28 = record(tests, Contribution::Infrastructure);
    assert(witness_28.failure == Failure::Tests);
    let witness_29 = record(pending, Contribution::Passed);
    assert(!witness_29.ready);
    let witness_30 = classic_status(ClassicStatus::Success);
    assert(witness_30 == Contribution::Passed);
    let witness_31 = classic_status(ClassicStatus::Failure);
    assert(witness_31 == Contribution::Tests);
    let witness_32 = classic_status(ClassicStatus::Error);
    assert(witness_32 == Contribution::Tests);
    let witness_33 = classic_status(ClassicStatus::Other);
    assert(witness_33 == Contribution::Pending);
}

/// Q52, #447, #2705; cdacce8f docs/state-table.md:542,578.
/// Every rerun outcome, eligibility gate and loop result is reachable.
fn rerun_witnesses() {
    let witness_34 = rerun(Failure::Infrastructure);
    assert(witness_34);
    let witness_35 = rerun(Failure::Tests);
    assert(!witness_35); let witness_36 = rerun(Failure::None);
    assert(!witness_36);
    let witness_37 = matching_suite(true, Some(1), 1);
    assert(witness_37);
    let witness_38 = matching_suite(false, Some(1), 1);
    assert(!witness_38);
    let witness_39 = matching_suite(true, None, 1);
    assert(!witness_39);
    let witness_40 = matching_suite(true, Some(2), 1);
    assert(!witness_40);
    let witness_41 = writable(true, true, false);
    assert(witness_41);
    let witness_42 = writable(false, true, false);
    assert(!witness_42); let witness_43 = writable(true, false, false);
    assert(!witness_43);
    let witness_44 = writable(true, true, true);
    assert(!witness_44);
    let witness_45 = needs_fence(Some(false));
    assert(witness_45); let witness_46 = needs_fence(None);
    assert(!witness_46); let witness_47 = needs_fence(Some(true));
    assert(!witness_47);
    let witness_48 = rerun_result(DispatchResult::Accepted);
    assert(witness_48 == RerunResult::Accepted);
    let witness_49 = rerun_result(DispatchResult::InProgress);
    assert(witness_49 == RerunResult::InProgress);
    let witness_50 = rerun_result(DispatchResult::Forbidden);
    assert(witness_50 == RerunResult::Refused);
    let witness_51 = rerun_result(DispatchResult::RateLimited);
    assert(witness_51 == RerunResult::Refused);
    let witness_52 = rerun_result(DispatchResult::Http(407));
    assert(witness_52 == RerunResult::Refused);
    let witness_53 = rerun_result(DispatchResult::Http(408));
    assert(witness_53 == RerunResult::Unknown);
    let witness_54 = rerun_result(DispatchResult::Http(409));
    assert(witness_54 == RerunResult::Refused);
    let witness_55 = rerun_result(DispatchResult::Http(500));
    assert(witness_55 == RerunResult::Unknown);
    let witness_56 = rerun_result(DispatchResult::Other);
    assert(witness_56 == RerunResult::Unknown);
    let witness_57 = rerun_loop(true);
    assert(witness_57 == Loop::ReadAgain); let witness_58 = rerun_loop(false);
    assert(witness_58 == Loop::Done);
}
}

use crate::{App, CheckView, Conclusion, Head, Status};
use crate::check::Name;
verus! {
/// K-required-check: Q47, docs/state-machine.md:681-688 and state-table.md:540-541,549,575.
/// The exact enum partition is preserved existing behaviour (no rule).
/// #447/state-table.md:545,578 concern queue conditions; #608 records served
/// RobTand/prismabuild#2705, not a classifier rule. All inputs are unbounded.
/// This is the retired Kani oracle, independent of the executable classifier.
pub open spec fn expected_required_check(check: CheckView, runner: bool) -> RequiredState {
    match (check.status, check.conclusion) {
        (Status::Completed, Some(Conclusion::Success | Conclusion::Neutral | Conclusion::Skipped)) => RequiredState::Passed,
        (Status::Completed, Some(Conclusion::ActionRequired | Conclusion::Stale)) => RequiredState::Pending,
        (Status::Completed, Some(conclusion)) => {
            if check.app == App::Actions &&
                (matches!(conclusion, Conclusion::Cancelled | Conclusion::TimedOut | Conclusion::StartupFailure)
                    || conclusion == Conclusion::Failure && runner) {
                RequiredState::Infrastructure
            } else { RequiredState::Tests }
        },
        (Status::Completed, None) => RequiredState::Unknown,
        _ => RequiredState::Pending,
    }
}
}

verus! {
/// Direct replacement of the old Kani assertion over arbitrary CheckView and
/// runner inputs. Its independent oracle retains every arm of that assertion.
fn required_check_oracle(check: CheckView, runner: bool) {
    let actual = required_check(check, runner);
    assert(actual == expected_required_check(check, runner));
}

/// Port of enqueue_required_check's six covers at base 504f10b0:343-354.
/// Q47, docs/state-machine.md:681-688; state-table.md:540-541,575.
/// Exact enum partition is preserved existing behaviour (no rule).
/// Full CheckView/u64 domain is proved by the real function's ensures;
/// these concrete executions establish every old cover, without an unwind.
fn required_check_covers() {
    let mut check = CheckView { id: 0, head: Head::Current, app: App::Actions,
        app_key: 0, name_key: 0, required: true,
        status: Status::Completed, conclusion: Some(Conclusion::Success), completed_at: None };
    let r1 = required_check(check, false);
    assert(r1 == RequiredState::Passed);
    check.status = Status::InProgress;
    let r2 = required_check(check, false);
    assert(r2 == RequiredState::Pending);
    check.status = Status::Completed;
    check.conclusion = None;
    let r3 = required_check(check, false);
    assert(r3 == RequiredState::Unknown);
    check.conclusion = Some(Conclusion::Failure);
    let r4 = required_check(check, false);
    assert(r4 == RequiredState::Tests);
    let r5 = required_check(check, true);
    assert(r5 == RequiredState::Infrastructure);
    assert(check.conclusion == Some(Conclusion::Failure));
    check.conclusion = Some(Conclusion::Cancelled);
    let r6 = required_check(check, false);
    assert(r6 == RequiredState::Infrastructure);
    assert(check.conclusion == Some(Conclusion::Cancelled));
}
}
