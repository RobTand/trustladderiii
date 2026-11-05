//! H8: required-head evaluation and the durable, per-workflow rerun budget.
//! The shell keeps parsing, sampling, payloads and all writes in their old order.
use super::enqueue::{RequiredState, Rerun, RerunResult};
use super::translate::Kind;
use super::{App, CheckView, Conclusion, Status};
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

pub use evaluation::*;
pub use rerun::*;
pub use runner::*;

pub mod evaluation {
    use super::*;

    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
    #[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Reported {
        Passed,
        Pending,
        Unknown,
        Tests,
    }
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
    #[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Evaluation {
        Missing,
        Reported(Reported),
        Unavailable,
        Infrastructure,
    }

    /// Q47/Q52: exactly one result for every required context. An absent or
    /// unreadable report cannot be classified as a CI failure. All typed inputs,
    /// with no sequence, identifier or attempt bound.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
        (out == Evaluation::Missing) == (!reports && (expects_app || statuses_available)),
        (out == Evaluation::Unavailable) == (!reports && !expects_app && !statuses_available),
        (out == Evaluation::Infrastructure) == (reports && state == RequiredState::Infrastructure),
        match out {
            Evaluation::Reported(report) => reports && match report {
                Reported::Passed => state == RequiredState::Passed,
                Reported::Pending => state == RequiredState::Pending,
                Reported::Unknown => state == RequiredState::Unknown,
                Reported::Tests => state == RequiredState::Tests,
            },
            _ => true,
        },
    ))]
    pub fn evaluate(
        reports: bool,
        expects_app: bool,
        statuses_available: bool,
        state: RequiredState,
    ) -> Evaluation {
        if !reports {
            if expects_app || statuses_available {
                Evaluation::Missing
            } else {
                Evaluation::Unavailable
            }
        } else {
            match state {
                RequiredState::Passed => Evaluation::Reported(Reported::Passed),
                RequiredState::Pending => Evaluation::Reported(Reported::Pending),
                RequiredState::Unknown => Evaluation::Reported(Reported::Unknown),
                RequiredState::Tests => Evaluation::Reported(Reported::Tests),
                RequiredState::Infrastructure => Evaluation::Infrastructure,
            }
        }
    }

    /// Q52: the upstream fact-kind projection composes with CC-T's route proof.
    /// Missing/unavailable are setup facts, never CiFailed. All typed inputs;
    /// the existing shell retains context/app payloads and permission messages.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
        match evaluation {
            Evaluation::Missing => out == Some(Kind::CheckMissing),
            Evaluation::Unavailable => out == Some(Kind::ChecksUnavailable),
            _ => out == None,
        },
        !matches!(out, Some(Kind::CiFailed { .. })),
    ))]
    pub fn q52_kind(evaluation: Evaluation) -> Option<Kind> {
        match evaluation {
            Evaluation::Missing => Some(Kind::CheckMissing),
            Evaluation::Unavailable => Some(Kind::ChecksUnavailable),
            Evaluation::Reported(_) | Evaluation::Infrastructure => None,
        }
    }

    /// Q52: a context must come from its expected app, if one is specified.
    /// All u64 app IDs, including missing apps; no bounded enumeration.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
        out == (context_matches && match expected_app {
            None => true,
            Some(id) => actual_app == Some(id),
        }),
    ))]
    pub fn matches_check(
        context_matches: bool,
        expected_app: Option<u64>,
        actual_app: Option<u64>,
    ) -> bool {
        context_matches
            && match expected_app {
                None => true,
                Some(id) => actual_app == Some(id),
            }
    }

    /// Q47: only completed Actions failures sample annotations. Total and unbounded.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
        out == (check.status == Status::Completed && check.app == App::Actions
            && check.conclusion == Some(Conclusion::Failure)),
    ))]
    pub fn needs_annotations(check: CheckView) -> bool {
        check.status == Status::Completed
            && check.app == App::Actions
            && check.conclusion == Some(Conclusion::Failure)
    }

    /// Publishing/Q47: exhaustive classification, including future values and
    /// absent conclusions. Unbounded over check IDs, app IDs, heads and times.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
        match (check.status, check.conclusion) {
            (Status::Completed, Some(Conclusion::Success | Conclusion::Neutral | Conclusion::Skipped)) => out == RequiredState::Passed,
            (Status::Completed, Some(Conclusion::ActionRequired | Conclusion::Stale)) => out == RequiredState::Pending,
            (Status::Completed, None) => out == RequiredState::Unknown,
            (Status::Completed, Some(conclusion)) =>
                if check.app == App::Actions && (matches!(conclusion,
                    Conclusion::Cancelled | Conclusion::TimedOut | Conclusion::StartupFailure)
                    || conclusion == Conclusion::Failure && runner_failure) {
                    out == RequiredState::Infrastructure
                } else { out == RequiredState::Tests },
            _ => out == RequiredState::Pending,
        },
    ))]
    pub fn required_check(check: CheckView, runner_failure: bool) -> RequiredState {
        if check.status != Status::Completed
            || matches!(
                check.conclusion,
                Some(Conclusion::ActionRequired | Conclusion::Stale)
            )
        {
            RequiredState::Pending
        } else if matches!(
            check.conclusion,
            Some(Conclusion::Success | Conclusion::Neutral | Conclusion::Skipped)
        ) {
            RequiredState::Passed
        } else if check.app == App::Actions
            && (matches!(
                check.conclusion,
                Some(Conclusion::Cancelled | Conclusion::TimedOut | Conclusion::StartupFailure)
            ) || check.conclusion == Some(Conclusion::Failure) && runner_failure)
        {
            RequiredState::Infrastructure
        } else if check.conclusion.is_some() {
            RequiredState::Tests
        } else {
            RequiredState::Unknown
        }
    }

    /// Q47/#503: only these completed conclusions have failure-log evidence.
    /// Total over future statuses/conclusions and absent conclusions; no bound.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
        out == (status == Status::Completed && matches!(conclusion,
            Some(Conclusion::Failure | Conclusion::TimedOut | Conclusion::Cancelled
                | Conclusion::StartupFailure | Conclusion::ActionRequired | Conclusion::Stale))),
    ))]
    pub fn evidence_failure(status: Status, conclusion: Option<Conclusion>) -> bool {
        status == Status::Completed
            && matches!(
                conclusion,
                Some(
                    Conclusion::Failure
                        | Conclusion::TimedOut
                        | Conclusion::Cancelled
                        | Conclusion::StartupFailure
                        | Conclusion::ActionRequired
                        | Conclusion::Stale
                )
            )
    }

    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
    #[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum LegacyStatus {
        Success,
        Failure,
        Error,
        Other,
    }

    /// Publishing/Q47: legacy commit statuses are code failures, never runner failures.
    /// Total over the parsed domain, with no sequence bound.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
        match status {
            LegacyStatus::Success => out == RequiredState::Passed,
            LegacyStatus::Failure | LegacyStatus::Error => out == RequiredState::Tests,
            LegacyStatus::Other => out == RequiredState::Pending,
        },
    ))]
    pub fn legacy_status(status: LegacyStatus) -> RequiredState {
        match status {
            LegacyStatus::Success => RequiredState::Passed,
            LegacyStatus::Failure | LegacyStatus::Error => RequiredState::Tests,
            LegacyStatus::Other => RequiredState::Pending,
        }
    }

    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
    #[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum FailureChoice {
        None,
        Tests,
        Infrastructure,
    }

    /// Publishing/Q47: any code failure takes precedence over runner failures.
    /// The payload and last-test ordering remain in the shell; unbounded checks.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
        (out == FailureChoice::Tests) == tests,
        (out == FailureChoice::Infrastructure) == (!tests && infrastructure),
        (out == FailureChoice::None) == (!tests && !infrastructure),
    ))]
    pub fn failure_choice(tests: bool, infrastructure: bool) -> FailureChoice {
        if tests {
            FailureChoice::Tests
        } else if infrastructure {
            FailureChoice::Infrastructure
        } else {
            FailureChoice::None
        }
    }

    #[cfg(verus_keep_ghost)]
    verus! {
        /// Q47/Q52: witnesses for every presence and check outcome; no preconditions.
        fn evaluation_witnesses() {
            let missing = evaluate(false, true, false, RequiredState::Tests);
            assert(missing == Evaluation::Missing);
            let missing_kind = q52_kind(missing);
            assert(missing_kind == Some(Kind::CheckMissing));
            let reported = evaluate(true, false, false, RequiredState::Tests);
            assert(reported == Evaluation::Reported(Reported::Tests));
            let infra = evaluate(true, false, false, RequiredState::Infrastructure);
            assert(infra == Evaluation::Infrastructure);
            let unavailable = evaluate(false, false, false, RequiredState::Infrastructure);
            assert(unavailable == Evaluation::Unavailable);
            let unavailable_kind = q52_kind(unavailable);
            assert(unavailable_kind == Some(Kind::ChecksUnavailable));
            let check = CheckView { id: 1, head: super::super::Head::Current,
                app: App::Actions, app_key: 1, name: super::super::Name::Other,
                name_key: 1, required: true, status: Status::Completed,
                conclusion: Some(Conclusion::Failure), completed_at: None };
            let runner = required_check(check, true);
            assert(runner == RequiredState::Infrastructure);
            let tests = required_check(check, false);
            assert(tests == RequiredState::Tests);
            let passed = CheckView { conclusion: Some(Conclusion::Success), ..check };
            let pass = required_check(passed, false);
            assert(pass == RequiredState::Passed);
            let pending = CheckView { status: Status::Queued, ..check };
            let wait = required_check(pending, true);
            assert(wait == RequiredState::Pending);
            let unknown = CheckView { conclusion: None, ..check };
            let unknown_state = required_check(unknown, false);
            assert(unknown_state == RequiredState::Unknown);
            let legacy = legacy_status(LegacyStatus::Error);
            assert(legacy == RequiredState::Tests);
            let test_first = failure_choice(true, true);
            let infra_only = failure_choice(false, true);
            let none = failure_choice(false, false);
            assert(test_first == FailureChoice::Tests);
            assert(infra_only == FailureChoice::Infrastructure);
            assert(none == FailureChoice::None);
            let matched = matches_check(true, Some(1), Some(1));
            let wrong_app = matches_check(true, Some(1), Some(2));
            assert(matched && !wrong_app);
            let sample = needs_annotations(check);
            let no_sample = needs_annotations(passed);
            assert(sample && !no_sample);
            let log = evidence_failure(Status::Completed, Some(Conclusion::Failure));
            let no_log = evidence_failure(Status::InProgress, Some(Conclusion::Failure));
            assert(log && !no_log);
        }
    }
}

pub mod runner {
    #[cfg(verus_keep_ghost)]
    use super::*;
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
    #[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Annotation {
        Ignore,
        RunnerFailure,
        OtherFailure,
    }
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
    #[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct RunnerEvidence {
        pub failure: bool,
        pub other: bool,
    }

    /// Q47: fold annotations after parsing severity and runner prefixes at the
    /// boundary. Unbounded induction step, not a fixed annotation-count model.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
        out.failure == (prior.failure || !matches!(parsed, Annotation::Ignore)),
        out.other == (prior.other || matches!(parsed, Annotation::OtherFailure)),
    ))]
    pub fn annotation(prior: RunnerEvidence, parsed: Annotation) -> RunnerEvidence {
        match parsed {
            Annotation::Ignore => prior,
            Annotation::RunnerFailure => RunnerEvidence {
                failure: true,
                other: prior.other,
            },
            Annotation::OtherFailure => RunnerEvidence {
                failure: true,
                other: true,
            },
        }
    }

    /// Q47: at least one failure and all failures are runner failures. No bound.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
        out == (evidence.failure && !evidence.other),
    ))]
    pub fn runner_failure(evidence: RunnerEvidence) -> bool {
        evidence.failure && !evidence.other
    }

    #[cfg(verus_keep_ghost)]
    verus! {
        /// Q47: empty, ignored, runner-only and mixed-annotation witnesses.
        fn runner_witnesses() {
            let empty = RunnerEvidence { failure: false, other: false };
            let empty_result = runner_failure(empty);
            assert(!empty_result);
            let ignored = annotation(empty, Annotation::Ignore);
            let ignored_result = runner_failure(ignored);
            assert(!ignored_result);
            let runner = annotation(empty, Annotation::RunnerFailure);
            let runner_result = runner_failure(runner);
            assert(runner_result);
            let mixed = annotation(runner, Annotation::OtherFailure);
            let mixed_result = runner_failure(mixed);
            assert(!mixed_result);
        }
    }
}

pub mod rerun {
    use super::*;
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
    #[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum RerunAction {
        Continue,
        PullChanged,
        Unknown,
        Finishing,
        ReadPull,
        Write,
    }

    /// Publishing/Q47/#751: one accepted or uncertain POST per workflow/head.
    /// This is an unbounded step theorem over every u64 attempt. Known refusals
    /// and RunInProgress release an unused reservation; they do not spend it.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
        match out.1 {
            RerunAction::Write => current_head && completed && writable == Some(true)
                && match prior { None => true, Some(saved) => !saved.accepted && saved.waiting && attempt <= saved.attempt }
                && out.0 == Some(Rerun { attempt, accepted: false, waiting: false }),
            RerunAction::Finishing => current_head && !completed
                && out.0 == Some(Rerun { attempt, accepted: false, waiting: true }),
            _ => out.0 == prior,
        },
        !current_head ==> out.1 == RerunAction::PullChanged,
        current_head ==> match prior {
            Some(saved) if saved.accepted || attempt > saved.attempt => out.1 == RerunAction::Continue,
            Some(saved) if !saved.waiting => out.1 == RerunAction::Unknown,
            _ => if !completed { out.1 == RerunAction::Finishing }
                else { match writable { None => out.1 == RerunAction::ReadPull,
                    Some(false) => out.1 == RerunAction::PullChanged,
                    Some(true) => out.1 == RerunAction::Write } },
        },
    ))]
    pub fn rerun_read(
        prior: Option<Rerun>,
        current_head: bool,
        attempt: u64,
        completed: bool,
        writable: Option<bool>,
    ) -> (Option<Rerun>, RerunAction) {
        if !current_head {
            return (prior, RerunAction::PullChanged);
        }
        if let Some(saved) = prior {
            if saved.accepted || attempt > saved.attempt {
                return (prior, RerunAction::Continue);
            }
            if !saved.waiting {
                return (prior, RerunAction::Unknown);
            }
        }
        if !completed {
            (
                Some(Rerun {
                    attempt,
                    accepted: false,
                    waiting: true,
                }),
                RerunAction::Finishing,
            )
        } else {
            match writable {
                None => (prior, RerunAction::ReadPull),
                Some(false) => (prior, RerunAction::PullChanged),
                Some(true) => (
                    Some(Rerun {
                        attempt,
                        accepted: false,
                        waiting: false,
                    }),
                    RerunAction::Write,
                ),
            }
        }
    }

    /// Publishing/Q47: an unknown response retains the durable reservation.
    /// Accepted receipts are final; only a known non-start releases it. No bound.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
        match (prior, result) {
            (_, RerunResult::Refused) => out == None,
            (Some(saved), RerunResult::Accepted) => out == Some(Rerun { accepted: true, ..saved }),
            (Some(saved), RerunResult::InProgress) => out == Some(Rerun { waiting: true, ..saved }),
            _ => out == prior,
        },
    ))]
    // Explicit branches let Verus check the receipt updates without closures.
    #[allow(clippy::manual_map)]
    pub fn rerun_done(prior: Option<Rerun>, result: RerunResult) -> Option<Rerun> {
        match result {
            RerunResult::Accepted => match prior {
                Some(saved) => Some(Rerun {
                    accepted: true,
                    ..saved
                }),
                None => None,
            },
            RerunResult::InProgress => match prior {
                Some(saved) => Some(Rerun {
                    waiting: true,
                    ..saved
                }),
                None => None,
            },
            RerunResult::Refused => None,
            RerunResult::Unknown => prior,
        }
    }

    /// #751: pending reconciles only from upstream attempt/status evidence.
    /// Stale heads stay pending; silence never refunds the budget. No bound.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
        out == match prior { None => false, Some(saved) => !saved.accepted
            && (!current_head || attempt <= saved.attempt && (!saved.waiting || !completed)) },
    ))]
    pub fn rerun_pending(
        prior: Option<Rerun>,
        current_head: bool,
        attempt: u64,
        completed: bool,
    ) -> bool {
        match prior {
            None => false,
            Some(saved) => {
                !saved.accepted
                    && (!current_head || attempt <= saved.attempt && (!saved.waiting || !completed))
            }
        }
    }

    /// #751: HTTP 408 is uncertain. Other 4xx refusals release an unused reservation.
    /// Total over every u16 status code, including future values.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
        out == ((400 <= status && status <= 407) || (409 <= status && status <= 499)),
    ))]
    pub fn http_refused(status: u16) -> bool {
        matches!(status, 400..=407 | 409..=499)
    }

    #[cfg(verus_keep_ghost)]
    verus! {
        /// Q47/#751: after either side of the POST crash boundary the saved
        /// intent prevents a second POST, for every subsequent attempt/status.
        /// Unbounded induction over restarts that retain persisted .reruns bytes.
        fn crash_receipt_blocks_second(attempt: u64, later: u64, completed: bool, writable: Option<bool>, current: bool) {
            let (receipt, action) = rerun_read(None, true, attempt, true, Some(true));
            assert(action == RerunAction::Write);
            let unknown = rerun_done(receipt, RerunResult::Unknown);
            let (next, second) = rerun_read(unknown, current, later, completed, writable);
            assert(second != RerunAction::Write);
            assert(next == unknown);
            let accepted = rerun_done(receipt, RerunResult::Accepted);
            let (last, third) = rerun_read(accepted, current, later, completed, writable);
            assert(third != RerunAction::Write);
            assert(last == accepted);
        }

        /// Q47: witnesses for every action, including stale-head rejection and
        /// known-refusal refund; two explicit steps, not a temporal bound.
        fn rerun_witnesses() {
            let (intent, write) = rerun_read(None, true, 1, true, Some(true));
            assert(write == RerunAction::Write);
            let (_, changed) = rerun_read(None, false, 1, true, Some(true));
            assert(changed == RerunAction::PullChanged);
            let (_, read) = rerun_read(None, true, 1, true, None);
            assert(read == RerunAction::ReadPull);
            let (waiting, finishing) = rerun_read(None, true, 1, false, None);
            assert(finishing == RerunAction::Finishing);
            let (_, retry) = rerun_read(waiting, true, 1, true, Some(true));
            assert(retry == RerunAction::Write);
            let (_, unknown) = rerun_read(intent, true, 1, true, Some(true));
            assert(unknown == RerunAction::Unknown);
            let (_, advanced) = rerun_read(intent, true, 2, true, Some(true));
            assert(advanced == RerunAction::Continue);
            let refunded = rerun_done(intent, RerunResult::Refused);
            assert(refunded == None);
            let uncertain = http_refused(408);
            let refused = http_refused(403);
            assert(!uncertain);
            assert(refused);
            let pending = rerun_pending(intent, true, 1, true);
            let done = rerun_pending(intent, true, 2, true);
            let stale = rerun_pending(intent, false, 2, true);
            assert(pending && !done && stale);
        }
    }
}
