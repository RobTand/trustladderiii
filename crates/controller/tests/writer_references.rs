use serde_json::json;
use tl3_controller::{
    Error,
    adapters::workers::writer::{
        FakeWriter, MessageKind, WriteRequest, Writer, WriterReply, checked_draft,
    },
};
use tl3_github::message::{Policy, check_prose};

fn policy() -> Policy {
    Policy {
        own_issue: Some(12),
        own_pull_request: Some(34),
        ..Policy::default()
    }
}

fn input(kind: MessageKind) -> WriteRequest {
    WriteRequest {
        kind,
        facts: json!({
            "issue_title": "Add a blue button", "request": "Add a blue button",
            "obstacle": "The artwork needs permission.",
            "options": [
                {"action": "Choose another image", "outcome": "I can finish with a free image."},
                {"action": "Grant permission", "outcome": "I can use the original image."}
            ],
            "recommended_option": 0, "recommendation_reason": "This avoids a delay.",
            "how_to_answer": "Reply with option one or two.",
            "comment_excerpt": "Maybe do that", "question": "Should I continue?",
            "change": "Work is paused.", "outcome": "The button is blue."
        }),
        violations: vec![],
    }
}

struct DraftWriter {
    text: String,
    rerun: bool,
    calls: Vec<WriteRequest>,
}
impl Writer for DraftWriter {
    fn draft(&mut self, _: &str, input: &WriteRequest) -> tl3_controller::Result<WriterReply> {
        self.calls.push(input.clone());
        let mut reply = FakeWriter.draft("fake", input)?;
        if !self.rerun || self.calls.len() == 1 {
            match &mut reply {
                WriterReply::Question(question) => question.obstacle = self.text.clone(),
                WriterReply::Text(text) if input.kind == MessageKind::Clarification => {
                    text.text = format!("About \"Maybe do that\", {}?", self.text);
                }
                WriterReply::Text(text) => text.text = self.text.clone(),
            }
        }
        Ok(reply)
    }
}

fn writer(text: &str, rerun: bool) -> DraftWriter {
    DraftWriter {
        text: text.into(),
        rerun,
        calls: vec![],
    }
}

macro_rules! rejection {
    ($name:ident, $token:expr) => {
        #[test]
        fn $name() {
            for kind in [
                MessageKind::Question,
                MessageKind::Clarification,
                MessageKind::Note,
                MessageKind::Summary,
            ] {
                let mut writer = writer($token, false);
                assert!(
                    matches!(
                        checked_draft(&mut writer, "message", input(kind), &policy()),
                        Err(Error::MessageRejected)
                    ),
                    "{kind:?}: {}",
                    $token
                );
                assert_eq!(writer.calls.len(), 3);
                assert!(
                    writer.calls[1]
                        .violations
                        .iter()
                        .any(|v| v.contains("Remove") && v.contains($token)),
                    "{kind:?}: {:?}",
                    writer.calls[1].violations
                );
            }
        }
    };
}
rejection!(rejects_user_mentions_in_every_message_kind, "@octocat");
rejection!(rejects_team_mentions_in_every_message_kind, "@org/team");
rejection!(
    rejects_foreign_repositories_in_every_message_kind,
    "owner/repo#12"
);
rejection!(rejects_foreign_issues_in_every_message_kind, "#56");

#[test]
fn own_issue_and_pull_request_are_allowed_in_every_message_kind() {
    for kind in [
        MessageKind::Question,
        MessageKind::Clarification,
        MessageKind::Note,
        MessageKind::Summary,
    ] {
        for text in ["See #12", "See #34"] {
            let mut writer = writer(text, false);
            assert!(
                checked_draft(&mut writer, "message", input(kind), &policy()).is_ok(),
                "{kind:?}: {text}"
            );
            assert_eq!(writer.calls.len(), 1);
        }
    }
    assert!(check_prose("See #12", &Policy::default()).is_err());
}

#[test]
fn code_spans_are_allowed_in_every_message_kind() {
    for kind in [
        MessageKind::Question,
        MessageKind::Clarification,
        MessageKind::Note,
        MessageKind::Summary,
    ] {
        for text in [
            "Use `@octocat @org/team owner/repo#56 #56`",
            "Use ``@octocat ` #56``",
        ] {
            let mut writer = writer(text, false);
            assert!(
                checked_draft(&mut writer, "message", input(kind), &policy()).is_ok(),
                "{kind:?}: {text}"
            );
        }
    }
}

#[test]
fn code_blocks_are_allowed_but_surrounding_prose_is_checked() {
    for text in [
        "Example:\n\n    @octocat @org/team owner/repo#56 #56\n",
        "Example:\n\n```\n@octocat @org/team owner/repo#56 #56\n```\n",
        "Example:\n\n~~~~\n@octocat @org/team owner/repo#56 #56\n~~~~\n",
        "> Example:\n>\n> ```\n> @octocat #56\n> ```\n",
    ] {
        assert!(check_prose(text, &policy()).is_ok(), "{text}");
        for token in ["@octocat", "owner/repo#56", "#56"] {
            assert!(
                check_prose(&format!("{text}\nOutside {token}"), &policy()).is_err(),
                "{text}: {token}"
            );
        }
    }
}

#[test]
fn rejected_draft_gets_actionable_feedback_then_a_clean_draft() {
    for kind in [
        MessageKind::Question,
        MessageKind::Clarification,
        MessageKind::Note,
        MessageKind::Summary,
    ] {
        for token in ["@octocat", "@org/team", "owner/repo#12", "#56"] {
            let mut writer = writer(token, true);
            let result = checked_draft(&mut writer, "message", input(kind), &policy()).unwrap();
            assert_eq!(writer.calls.len(), 2, "{kind:?}: {token}");
            assert!(
                writer.calls[1]
                    .violations
                    .iter()
                    .any(|v| v.contains("Remove") && v.contains(token)),
                "{:?}",
                writer.calls[1].violations
            );
            assert!(!result.contains(token));
        }
    }
}

#[test]
fn checker_feedback_reaches_real_writer_protocol_and_fake_model() {
    use std::{
        io::Write,
        path::PathBuf,
        process::{Command, Stdio},
    };
    let package = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../workers");
    let uv = std::env::var_os("TL3_TEST_UV").unwrap_or_else(|| "uv".into());
    assert!(
        Command::new(uv)
            .args(["sync", "--locked", "--python", "3.14+gil", "--project"])
            .arg(&package)
            .status()
            .expect("install uv or set TL3_TEST_UV")
            .success()
    );
    let root = tempfile::tempdir().unwrap();
    let key_file = root.path().join("fixture-key");
    std::fs::write(&key_file, "fixture-only").unwrap();

    struct ProtocolWriter {
        python: PathBuf,
        key_file: PathBuf,
        calls: usize,
    }
    impl Writer for ProtocolWriter {
        fn draft(&mut self, _: &str, input: &WriteRequest) -> tl3_controller::Result<WriterReply> {
            self.calls += 1;
            let request = json!({
                "protocol": 1, "operation": {"node": 1, "generation": 0, "kind": "write_message"},
                "role": "writer", "inputs": input,
                "model": {"base_url": "http://model.invalid/v1", "name": "fixture-model", "effort": "high", "budget_tokens": 32000},
                "key_file": self.key_file,
            });
            let mut child = Command::new(&self.python)
                .arg(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/writer_reference_model.py"
                ))
                .env("LANGSMITH_TRACING", "false")
                .env("LANGCHAIN_TRACING_V2", "false")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(serde_json::to_string(&request).unwrap().as_bytes())
                .unwrap();
            let output = child.wait_with_output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let events: Vec<serde_json::Value> = String::from_utf8(output.stdout)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            assert_eq!(events.len(), 2);
            assert_eq!(
                events[0],
                json!({"event": "progress", "unit": "model_turn"})
            );
            assert_eq!(events[1]["event"], "reply");
            Ok(serde_json::from_value(events[1]["reply"].clone()).unwrap())
        }
    }
    for kind in [
        MessageKind::Question,
        MessageKind::Clarification,
        MessageKind::Note,
        MessageKind::Summary,
    ] {
        let mut writer = ProtocolWriter {
            python: package.join(".venv/bin/python"),
            key_file: key_file.clone(),
            calls: 0,
        };
        let draft = checked_draft(&mut writer, "protocol", input(kind), &policy()).unwrap();
        assert_eq!(writer.calls, 2, "{kind:?}");
        assert!(draft.contains("artwork needs permission"));
        assert!(!draft.contains('@') && !draft.contains('#'));
    }
}
