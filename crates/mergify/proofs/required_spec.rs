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
