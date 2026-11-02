//! Owed postmortems and code grades (#403, Q32). Both are recorded in the
//! commit that makes them owed, outside the kernel; the judge runs later.

use serde_json::Value;
use tl3_kernel::{Node, Outcome};

/// TL3's rubric version, in `rubric.json`. Raise it with any change that could
/// change a grade; regrading adds grades and leaves old ones as they are.
pub(crate) const RUBRIC_VERSION: i32 = 1;

/// Facts about a node's life that its world row no longer holds.
#[derive(Clone, Debug, Default)]
pub(crate) struct History {
    /// The kinds of the questions it asked Rob.
    pub questions: Vec<String>,
    /// Its incidents' cause classes, and whether each needed Rob.
    pub incidents: Vec<(String, bool)>,
    /// A worker run failed and was retried.
    pub run_failure: bool,
    /// Times the queue sent it back: CI failures, conflicts and dequeues.
    pub dequeues: u32,
    /// It merged with failing checks.
    pub escaped_ci: bool,
    /// Seconds spent in each phase, in order.
    pub phases: Vec<(String, u64)>,
}

/// Why a postmortem is owed, as the console lists it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reason {
    Escalation,
    NeededFix,
    Retry,
    ClosedWithoutMerge,
    Sampled,
    RobAsked,
}

impl Reason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Reason::Escalation => "escalation",
            Reason::NeededFix => "needed_fix",
            Reason::Retry => "retry",
            Reason::ClosedWithoutMerge => "closed_without_merge",
            Reason::Sampled => "sampled",
            Reason::RobAsked => "rob_asked",
        }
    }
}

/// Q32's selection at close: everything that needed a fix, a retry or Rob, and
/// everything that closed without merging, plus a sampled share of clean merges.
/// Returns the reason and, for a sampled node, the share in force.
pub(crate) fn close_reason(
    node: &Node,
    outcome: Outcome,
    history: &History,
    share: f64,
    sample_key: &str,
) -> Option<(Reason, Option<f64>)> {
    let _ = (node, outcome, history, share, sample_key);
    todo!()
}

/// The rubric's code grades at close: (dimension, grade, fact).
pub(crate) fn close_grades(
    node: &Node,
    outcome: Outcome,
    history: &History,
) -> Vec<(&'static str, Option<i16>, Value)> {
    let _ = (node, outcome, history);
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tl3_kernel::RepoId;

    fn clean() -> Node {
        let mut node = Node::new(RepoId(0));
        node.attempt = 1;
        node
    }

    fn reason(node: &Node, outcome: Outcome, history: &History) -> Option<Reason> {
        close_reason(node, outcome, history, 0.0, "node:0").map(|(reason, _)| reason)
    }

    #[test]
    fn selection_follows_q32() {
        let quiet = History::default();
        assert_eq!(reason(&clean(), Outcome::Merged, &quiet), None);
        let mut fixed = clean();
        fixed.round = 1;
        assert_eq!(
            reason(&fixed, Outcome::Merged, &quiet),
            Some(Reason::NeededFix)
        );
        let mut later = clean();
        later.attempt = 2;
        assert_eq!(
            reason(&later, Outcome::Merged, &quiet),
            Some(Reason::NeededFix)
        );
        let mut replanned = clean();
        replanned.replanned = true;
        assert_eq!(
            reason(&replanned, Outcome::Merged, &quiet),
            Some(Reason::NeededFix)
        );
        let retried = History {
            run_failure: true,
            ..History::default()
        };
        assert_eq!(
            reason(&clean(), Outcome::Merged, &retried),
            Some(Reason::Retry)
        );
        let incident = History {
            incidents: vec![("transport".into(), false)],
            ..History::default()
        };
        assert_eq!(
            reason(&clean(), Outcome::Merged, &incident),
            Some(Reason::Retry)
        );
        let asked = History {
            questions: vec!["cant_meet".into()],
            ..History::default()
        };
        assert_eq!(
            reason(&fixed, Outcome::Merged, &asked),
            Some(Reason::Escalation),
            "escalation outranks the fix it led to"
        );
        let needed_rob = History {
            incidents: vec![("permission".into(), true)],
            ..History::default()
        };
        assert_eq!(
            reason(&clean(), Outcome::Merged, &needed_rob),
            Some(Reason::Escalation)
        );
        for outcome in [
            Outcome::Stopped,
            Outcome::Replaced,
            Outcome::Duplicate,
            Outcome::AlreadyDone,
            Outcome::NotActionable,
        ] {
            assert_eq!(
                reason(&clean(), outcome, &quiet),
                Some(Reason::ClosedWithoutMerge),
                "{outcome:?}: every node that closes without merging gets one"
            );
        }
    }

    #[test]
    fn clean_merges_are_sampled_at_the_stored_share() {
        let quiet = History::default();
        assert_eq!(
            close_reason(&clean(), Outcome::Merged, &quiet, 1.0, "node:3"),
            Some((Reason::Sampled, Some(1.0)))
        );
        assert_eq!(
            close_reason(&clean(), Outcome::Merged, &quiet, 0.0, "node:3"),
            None
        );
        let sampled = (0..1000)
            .filter(|n| {
                close_reason(
                    &clean(),
                    Outcome::Merged,
                    &quiet,
                    0.25,
                    &format!("node:{n}"),
                )
                .is_some()
            })
            .count();
        assert!((180..=320).contains(&sampled), "{sampled} of 1000 at 0.25");
        for n in 0..50 {
            let key = format!("node:{n}");
            assert_eq!(
                close_reason(&clean(), Outcome::Merged, &quiet, 0.25, &key),
                close_reason(&clean(), Outcome::Merged, &quiet, 0.25, &key),
                "the same node is always sampled the same way"
            );
        }
    }

    fn graded(
        node: &Node,
        outcome: Outcome,
        history: &History,
    ) -> Vec<(&'static str, Option<i16>)> {
        close_grades(node, outcome, history)
            .into_iter()
            .map(|(dimension, grade, _)| (dimension, grade))
            .collect()
    }

    #[test]
    fn code_grades_record_the_facts_at_close() {
        let quiet = History {
            phases: vec![("Making".into(), 90), ("Checking".into(), 30)],
            ..History::default()
        };
        let grades = graded(&clean(), Outcome::Merged, &quiet);
        for expected in [
            ("reliability", Some(5)),
            ("escalation", Some(5)),
            ("queue", Some(5)),
            ("credit", None),
            ("questions", None),
            ("incidents", None),
            ("phase_time", None),
        ] {
            assert!(grades.contains(&expected), "{expected:?} in {grades:?}");
        }
        let facts = close_grades(&clean(), Outcome::Merged, &quiet);
        let phases = &facts.iter().find(|g| g.0 == "phase_time").unwrap().2;
        assert_eq!(phases["Making"], 90);
        let mut troubled = clean();
        troubled.attempt = 2;
        troubled.round = 1;
        let history = History {
            questions: vec!["cant_meet".into(), "out_of_credit".into()],
            incidents: vec![("permission".into(), true), ("transport".into(), false)],
            dequeues: 1,
            ..History::default()
        };
        let grades = graded(&troubled, Outcome::Merged, &history);
        assert!(grades.contains(&("reliability", Some(3))), "{grades:?}");
        assert!(grades.contains(&("escalation", Some(2))), "{grades:?}");
        assert!(grades.contains(&("queue", Some(4))), "{grades:?}");
        let escaped = History {
            escaped_ci: true,
            ..History::default()
        };
        assert!(graded(&clean(), Outcome::Merged, &escaped).contains(&("queue", Some(1))));
        let stopped = graded(&clean(), Outcome::Stopped, &History::default());
        assert!(stopped.contains(&("reliability", Some(1))), "{stopped:?}");
        assert!(
            stopped.contains(&("queue", None)),
            "an unmerged node has no queue grade"
        );
    }

    #[test]
    fn the_rubric_names_every_code_dimension() {
        let rubric: Value = serde_json::from_str(include_str!("rubric.json")).unwrap();
        assert_eq!(rubric["version"], RUBRIC_VERSION);
        for (dimension, _, _) in close_grades(&clean(), Outcome::Merged, &History::default()) {
            assert_eq!(
                rubric["dimensions"][dimension]["grader"], "code",
                "{dimension}"
            );
        }
    }
}
