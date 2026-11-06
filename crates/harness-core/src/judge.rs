//! Q40/Q42/Q55: the basic check table, before execution-shell overrides.

#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

// SEAM(planner-contracts): replace with contracts type when shared.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Demonstrating,
    Reproducing,
    Regression,
    BehaviorPreserving,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Passed,
    Assertion,
    AddedName,
    Error,
    Skipped,
    Missing,
    Uncollected,
    NoReport,
}

/// #709/#713: capture validates the declaration and call-phase match together.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    Undeclared,
    Mismatch,
    Matched,
    DeclaredSymptom,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Input {
    pub kind: Kind,
    pub base: Outcome,
    pub head: Outcome,
    pub forced: Option<Outcome>,
    pub reason: Reason,
}

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decision {
    pub passed: bool,
    pub pre_existing: bool,
    pub infrastructure: bool,
}

/// State-table Checks / Q40/Q42/Q55: all enum values, one named test; no loop bound.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out =>
    requires input.forced.is_none() || matches!(input.kind, Kind::Demonstrating),
    ensures
    out.pre_existing == (matches!(input.kind, Kind::Regression)
        && matches!(input.base, Outcome::Assertion | Outcome::Error)),
    out.infrastructure == (matches!(input.base, Outcome::NoReport)
        || matches!(input.head, Outcome::NoReport)
        || matches!(input.forced, Some(Outcome::NoReport))),
    out.passed == (
        (match input.kind {
            Kind::Demonstrating => if input.forced.is_some() {
                matches!(input.base, Outcome::Assertion)
                    && matches!(input.head, Outcome::Passed)
                    && matches!(input.forced, Some(Outcome::Passed))
            } else {
                matches!(input.base, Outcome::Assertion | Outcome::AddedName)
                    && matches!(input.head, Outcome::Passed)
            },
            Kind::Reproducing => matches!(input.base, Outcome::Assertion | Outcome::AddedName)
                && matches!(input.head, Outcome::Passed),
            Kind::Regression => matches!(input.base,
                Outcome::Passed | Outcome::Assertion | Outcome::AddedName | Outcome::Error)
                && (matches!(input.head, Outcome::Passed)
                    || matches!(input.base, Outcome::Assertion | Outcome::Error)
                        && matches!(input.head, Outcome::Assertion | Outcome::Error)),
            Kind::BehaviorPreserving => matches!(input.base, Outcome::Passed)
                && matches!(input.head, Outcome::Passed),
        })
        && (!matches!(input.base, Outcome::Assertion)
            || !matches!(input.kind, Kind::Demonstrating | Kind::Reproducing)
            || matches!(input.reason, Reason::Matched | Reason::DeclaredSymptom))
    ),
))]
pub fn judge(input: Input) -> Decision {
    let pre_existing = matches!(input.kind, Kind::Regression)
        && matches!(input.base, Outcome::Assertion | Outcome::Error);
    let infrastructure = matches!(input.base, Outcome::NoReport)
        || matches!(input.head, Outcome::NoReport)
        || matches!(input.forced, Some(Outcome::NoReport));
    let candidate = match input.kind {
        Kind::Demonstrating if input.forced.is_some() => {
            matches!(input.base, Outcome::Assertion)
                && matches!(input.head, Outcome::Passed)
                && matches!(input.forced, Some(Outcome::Passed))
        }
        Kind::Demonstrating | Kind::Reproducing => {
            matches!(input.base, Outcome::Assertion | Outcome::AddedName)
                && matches!(input.head, Outcome::Passed)
        }
        Kind::Regression => {
            let available = !matches!(
                input.base,
                Outcome::Skipped | Outcome::Missing | Outcome::Uncollected | Outcome::NoReport
            );
            available
                && (matches!(input.head, Outcome::Passed)
                    || pre_existing && matches!(input.head, Outcome::Assertion | Outcome::Error))
        }
        Kind::BehaviorPreserving => {
            matches!(input.base, Outcome::Passed) && matches!(input.head, Outcome::Passed)
        }
    };
    let reason_ok = !matches!(input.base, Outcome::Assertion)
        || !matches!(input.kind, Kind::Demonstrating | Kind::Reproducing)
        || matches!(input.reason, Reason::Matched | Reason::DeclaredSymptom);
    Decision {
        passed: candidate && reason_ok,
        pre_existing,
        infrastructure,
    }
}

#[cfg(not(verus_keep_ghost))]
impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Demonstrating => "demonstrating",
            Self::Reproducing => "reproducing",
            Self::Regression => "regression",
            Self::BehaviorPreserving => "behavior-preserving",
        }
    }
}

#[cfg(not(verus_keep_ghost))]
impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Assertion => "assertion",
            Self::AddedName => "added-name",
            Self::Error => "error",
            Self::Skipped => "skipped",
            Self::Missing => "missing",
            Self::Uncollected => "uncollected",
            Self::NoReport => "no-report",
        }
    }
}

/// Failure text is presentation data; only its phase and classification affect refusal choice.
#[cfg(not(verus_keep_ghost))]
#[derive(Clone, Debug)]
pub struct Failure<'a> {
    pub phase: &'a str,
    pub exception: &'a str,
}

#[cfg(not(verus_keep_ghost))]
#[derive(Clone, Debug)]
pub struct FailurePhase<'a> {
    pub phase: &'a str,
    pub failed: bool,
    pub assertion: bool,
    pub exception: &'a str,
}

#[cfg(not(verus_keep_ghost))]
fn when(phase: &str) -> &'static str {
    match phase {
        "setup" => "in setup",
        "call" => "during the call",
        "teardown" => "during teardown",
        _ => "in the test",
    }
}

/// Q40/Q42: exact Python refusal wording, given facts captured before report clipping.
#[cfg(not(verus_keep_ghost))]
pub fn base_refusal(
    kind: Kind,
    base: Outcome,
    failure: Option<&Failure<'_>>,
    phases: &[FailurePhase<'_>],
) -> String {
    let verb = if kind == Kind::Demonstrating {
        "demonstrate"
    } else {
        "reproduce"
    };
    if base == Outcome::Passed {
        return format!(
            "This test passed on the base. A {} test must fail by an assertion on the base, or lack a name this change adds, so it does not {verb} the change. Make it assert the behavior the change adds.",
            kind.as_str()
        );
    }
    let Some(failure) = failure.filter(|_| base == Outcome::Error) else {
        return String::new();
    };
    if phases
        .iter()
        .any(|p| p.failed && matches!(p.phase, "setup" | "teardown"))
    {
        let failures = ["setup", "call", "teardown"]
            .into_iter()
            .filter_map(|phase| {
                let fact = phases.iter().find(|p| p.phase == phase && p.failed)?;
                let classification = if fact.assertion {
                    "an assertion failure"
                } else {
                    "an error"
                };
                let exception = if phase == failure.phase {
                    failure.exception
                } else {
                    fact.exception
                };
                let exception = if exception.is_empty() {
                    "an unknown exception"
                } else {
                    exception
                };
                Some(format!("{classification} {} (`{exception}`)", when(phase)))
            })
            .collect::<Vec<_>>()
            .join("; and ");
        return format!(
            "On the base this test had {failures}. A setup or teardown failure blocks assertion RED, so it does not {verb} the change. Fix the setup or teardown error and keep the assertion in the test body."
        );
    }
    let what = if failure.exception.is_empty() {
        "an error".into()
    } else {
        format!("`{}`", failure.exception)
    };
    match failure.phase {
        "collection" => format!(
            "On the base this test file could not be collected: {what}. An import failure counts only when it names a module or name this change adds, and this one does not, so the test does not {verb} the change. Import the new name inside the test body, or make the base run fail by an assertion."
        ),
        "setup" => format!(
            "On the base this test failed in setup with {what}. A setup or fixture error is not an assertion failure, so it does not {verb} the change. Make the base run fail by an assertion in the test body."
        ),
        phase => format!(
            "On the base this test raised {what} {}. That is neither an assertion failure nor a missing name this change adds, so it does not {verb} the change. Make the base run fail by an assertion, or import the new name inside the test body.",
            when(phase)
        ),
    }
}

/// Q55/#393: forcing failures take precedence over the base outcome.
#[cfg(not(verus_keep_ghost))]
pub fn forced_base_refusal(
    base: Outcome,
    failure: Option<&Failure<'_>>,
    phases: &[FailurePhase<'_>],
) -> String {
    if let Some(failure) = failure.filter(|f| f.exception.starts_with("ForcingError")) {
        return format!(
            "The plan's forcing could not be applied: {}. The forcing is part of the plan, so no change to the code or tests can fix it; report the goal as not met and quote this message.",
            failure.exception
        );
    }
    if phases
        .iter()
        .any(|p| p.failed && matches!(p.phase, "setup" | "teardown"))
    {
        return base_refusal(Kind::Demonstrating, base, failure, phases).replacen(
            "On the base ",
            "On the base under the forcing ",
            1,
        );
    }
    match base {
        Outcome::Assertion => String::new(),
        Outcome::Passed => "This test passed on the base under the forcing, so the forcing does not reproduce the flake. A forced reproduction needs the old test to fail by an assertion on the base while the forcing holds.".into(),
        Outcome::Missing | Outcome::Uncollected => "A forced reproduction runs the old test as the base wrote it, and this test is not defined at the base. Name the existing flaky test.".into(),
        _ => {
            let what = failure.filter(|f| !f.exception.is_empty()).map_or_else(|| base.as_str().to_owned(), |f| format!("`{}`", f.exception));
            format!("On the base under the forcing this test ended with {what}, not an assertion failure, so it does not reproduce the flake.")
        }
    }
}

#[cfg(not(verus_keep_ghost))]
fn clipped(message: String, marker: bool) -> String {
    if message.chars().count() <= 1000 {
        return message;
    }
    if marker {
        format!(
            "{} [truncated]",
            message.chars().take(988).collect::<String>()
        )
    } else {
        message.chars().take(1000).collect()
    }
}

/// #822: report text follows the original refusal and declared-reason precedence.
#[cfg(not(verus_keep_ghost))]
pub fn message(
    input: Input,
    head_message: &str,
    forced_message: &str,
    failure: Option<&Failure<'_>>,
    phases: &[FailurePhase<'_>],
    declaration: &str,
) -> String {
    if judge(input).passed {
        return String::new();
    }
    let mut result = head_message.to_owned();
    if let Some(during) = input.forced {
        let refusal = forced_base_refusal(input.base, failure, phases);
        if !refusal.is_empty() {
            result = refusal;
        } else if during != Outcome::Passed {
            result = format!(
                "On the head under the forcing this test ended {}: the fix must hold while the flake's condition is forced.",
                during.as_str()
            );
            if !forced_message.is_empty() {
                result.push_str(&format!("\n{forced_message}"));
            }
        } else {
            result = format!(
                "On the head without the forcing this test ended {}: the fix must not depend on the forcing.{}",
                input.head.as_str(),
                if result.is_empty() {
                    String::new()
                } else {
                    format!("\n{result}")
                }
            );
        }
        result = clipped(result, true);
    } else if matches!(input.kind, Kind::Demonstrating | Kind::Reproducing) {
        let refusal = base_refusal(input.kind, input.base, failure, phases);
        if !refusal.is_empty() {
            result = clipped(
                format!(
                    "{refusal}{}",
                    if result.is_empty() {
                        String::new()
                    } else {
                        format!("\nOn the head: {result}")
                    }
                ),
                true,
            );
        }
    }
    // The reason refusal applies only when the outcome table would otherwise pass.
    let candidate = judge(Input {
        reason: Reason::Matched,
        ..input
    })
    .passed;
    if candidate
        && matches!(input.base, Outcome::Assertion)
        && matches!(input.kind, Kind::Demonstrating | Kind::Reproducing)
    {
        match input.reason {
            Reason::Undeclared => {
                result =
                    "Assertion RED needs a declared assertion reason in proof.red_message.".into()
            }
            Reason::Mismatch => {
                result =
                    format!("RED did not fail for the declared assertion reason: {declaration}")
            }
            _ => (),
        }
    }
    if result.is_empty() {
        result = format!(
            "Check {}: base {}, head {}.",
            input.kind.as_str(),
            input.base.as_str(),
            input.head.as_str()
        );
    }
    clipped(result, false)
}
