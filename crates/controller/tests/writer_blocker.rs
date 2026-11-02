use serde_json::json;
use tl3_controller::{
    Error,
    adapters::workers::writer::{
        FakeWriter, MessageKind, WriteRequest, Writer, WriterReply, checked_draft, fixed_question,
    },
};
use tl3_github::message::Policy;

const REPORT: &str = include_str!(
    "../../../workers/tests/fixtures/question-findings/prismabuild-2636-planner-cant-meet.json"
);
const BLOCKER: &str = "The change needs tools/ci/bazel_gate.py, which the protected-path rule tools/ci/** forbids me to edit.";

fn input(source: &str, report: &str) -> WriteRequest {
    WriteRequest {
        kind: MessageKind::Question,
        facts: json!({
            "issue_title": "Rerun tests that exceed their budget",
            "request": "Rerun tests that exceed their budget",
            "obstacle": "I could not finish your request with the approach I tried.",
            "options": [
                {"action": "Try again with guidance", "outcome": "I will try again using your guidance."},
                {"action": "Change the request", "outcome": "I will start from your new request."},
                {"action": "Stop", "outcome": "I will stop this work."}
            ],
            "recommended_option": 0, "recommendation_reason": "This gives the work a next step.",
            "how_to_answer": "Reply on the issue with your chosen option.",
            "findings": {"source": source, "outcome": "cant_meet", "report": report,
                "decision": "What should I change about the approach so the request can be met?"}
        }),
        violations: vec![],
        request: None,
    }
}

fn real_report() -> String {
    serde_json::from_str::<serde_json::Value>(REPORT).unwrap()["report"]
        .as_str()
        .unwrap()
        .to_owned()
}

struct Scripted {
    obstacles: Vec<String>,
    calls: Vec<WriteRequest>,
}
impl Writer for Scripted {
    fn draft(&mut self, _: &str, input: &WriteRequest) -> tl3_controller::Result<WriterReply> {
        self.calls.push(input.clone());
        let WriterReply::Question(mut draft) = FakeWriter.draft("fake", input)? else {
            unreachable!()
        };
        draft.obstacle = self.obstacles.remove(0);
        if draft.obstacle == BLOCKER {
            draft.options[0].outcome = "Grant a protected-path exception for tools/ci/bazel_gate.py so I can try the whole request again.".into();
            draft.options[1].outcome =
                "Narrow to the runner-only part and leave the rest for work outside TL3.".into();
        }
        Ok(WriterReply::Question(draft))
    }
}

fn scripted(obstacles: &[&str]) -> Scripted {
    Scripted {
        obstacles: obstacles.iter().map(|s| (*s).into()).collect(),
        calls: vec![],
    }
}

#[test]
fn generic_planner_and_maker_questions_are_rejected_and_redrafted() {
    for source in ["planner", "maker"] {
        let mut writer = scripted(&["I could not finish your request.", BLOCKER]);
        let policy = Policy {
            known_paths: ["tools/ci/bazel_gate.py".into()].into(),
            ..Policy::default()
        };
        let text = checked_draft(&mut writer, "ask", input(source, &real_report()), &policy)
            .expect("a corrected question can name the reported file and rule");
        assert_eq!(writer.calls.len(), 2, "a generic draft must be refused");
        assert!(
            writer.calls[1]
                .violations
                .iter()
                .any(|v| v.contains("blocker")),
            "{:?}",
            writer.calls[1].violations
        );
        for detail in [
            "tools/ci/bazel_gate.py",
            "tools/ci/**",
            "Grant a protected-path exception",
            "runner-only",
            "outside TL3",
            "Change the request",
            "Stop",
        ] {
            assert!(text.contains(detail), "missing {detail}: {text}");
        }
    }
}

#[test]
fn rule_only_reports_also_reject_generic_questions() {
    let mut writer = scripted(&[
        "I could not finish your request.",
        "Rule Q27 requires your approval before this change can proceed.",
    ]);
    checked_draft(
        &mut writer,
        "rule",
        input("maker", "Rule Q27 requires approval."),
        &Policy::default(),
    )
    .unwrap();
    assert_eq!(writer.calls.len(), 2, "a named rule is a blocker too");
}

#[test]
fn path_or_rule_is_enough_but_title_and_options_do_not_replace_the_obstacle() {
    for obstacle in [
        "I cannot edit tools/ci/bazel_gate.py without your approval.",
        "The protected-path rule prevents the requested change.",
    ] {
        let mut writer = scripted(&[obstacle]);
        checked_draft(
            &mut writer,
            "anchor",
            input("planner", &real_report()),
            &Policy::default(),
        )
        .unwrap();
    }
    let mut request = input("planner", &real_report());
    request.facts["issue_title"] = json!("Change tools/ci/bazel_gate.py");
    request.facts["options"][0]["outcome"] = json!("Grant a protected-path exception.");
    let mut writer = scripted(&["I could not finish your request."; 3]);
    assert!(matches!(
        checked_draft(&mut writer, "generic", request, &Policy::default()),
        Err(Error::MessageRejected)
    ));
}

#[test]
fn reported_paths_do_not_allow_unrelated_paths_or_internal_vocabulary() {
    for obstacle in [
        "The protected-path rule blocks tools/ci/secret.py.",
        "The leaf cannot edit tools/ci/bazel_gate.py.",
        "The protected-path rule blocks tools/ci/bazel_gate.py/secret.py.",
    ] {
        let mut writer = scripted(&[obstacle; 3]);
        assert!(
            matches!(
                checked_draft(
                    &mut writer,
                    "unsafe",
                    input("planner", &real_report()),
                    &Policy::default()
                ),
                Err(Error::MessageRejected)
            ),
            "{obstacle}"
        );
    }
}

#[test]
fn fixed_fallback_cannot_post_a_generic_question_over_a_reported_blocker() {
    assert!(matches!(
        fixed_question(
            &input("planner", &real_report()),
            Some("What should I change?"),
            &Policy::default()
        ),
        Err(Error::MessageRejected)
    ));
}
