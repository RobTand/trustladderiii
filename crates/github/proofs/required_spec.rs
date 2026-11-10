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
    let witness_6 = needs_statuses(&[Some(1), None]);
    assert(witness_6);
    let witness_7 = needs_statuses(&[Some(1)]);
    assert(!witness_7); let witness_8 = needs_statuses(&[]);
    assert(!witness_8);
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
}

use crate::{App, CheckView, Conclusion, Head, Status};
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
