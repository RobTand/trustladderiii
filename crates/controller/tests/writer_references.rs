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
    DraftWriter { text: text.into(), rerun, calls: vec![] }
}

macro_rules! rejection {
    ($name:ident, $token:expr) => {
        #[test]
        fn $name() {
            for kind in [MessageKind::Question, MessageKind::Clarification, MessageKind::Note, MessageKind::Summary] {
                let mut writer = writer($token, false);
                assert!(matches!(checked_draft(&mut writer, "message", input(kind), &policy()), Err(Error::MessageRejected)), "{kind:?}: {}", $token);
                assert_eq!(writer.calls.len(), 3);
                assert!(writer.calls[1].violations.iter().any(|v| v.contains("Remove") && v.contains($token)), "{kind:?}: {:?}", writer.calls[1].violations);
            }
        }
    };
}
rejection!(rejects_user_mentions_in_every_message_kind, "@octocat");
rejection!(rejects_team_mentions_in_every_message_kind, "@org/team");
rejection!(rejects_foreign_repositories_in_every_message_kind, "owner/repo#12");
rejection!(rejects_foreign_issues_in_every_message_kind, "#56");

#[test]
fn own_issue_and_pull_request_are_allowed_in_every_message_kind() {
    for kind in [MessageKind::Question, MessageKind::Clarification, MessageKind::Note, MessageKind::Summary] {
        for text in ["See #12", "See #34"] {
            let mut writer = writer(text, false);
            assert!(checked_draft(&mut writer, "message", input(kind), &policy()).is_ok(), "{kind:?}: {text}");
            assert_eq!(writer.calls.len(), 1);
        }
    }
    assert!(check_prose("See #12", &Policy::default()).is_err());
}

#[test]
fn code_spans_are_allowed_in_every_message_kind() {
    for kind in [MessageKind::Question, MessageKind::Clarification, MessageKind::Note, MessageKind::Summary] {
        for text in ["Use `@octocat @org/team owner/repo#56 #56`", "Use ``@octocat ` #56``"] {
            let mut writer = writer(text, false);
            assert!(checked_draft(&mut writer, "message", input(kind), &policy()).is_ok(), "{kind:?}: {text}");
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
            assert!(check_prose(&format!("{text}\nOutside {token}"), &policy()).is_err(), "{text}: {token}");
        }
    }
}

#[test]
fn rejected_draft_gets_actionable_feedback_then_a_clean_draft() {
    for kind in [MessageKind::Question, MessageKind::Clarification, MessageKind::Note, MessageKind::Summary] {
        for token in ["@octocat", "@org/team", "owner/repo#12", "#56"] {
            let mut writer = writer(token, true);
            let result = checked_draft(&mut writer, "message", input(kind), &policy()).unwrap();
            assert_eq!(writer.calls.len(), 2, "{kind:?}: {token}");
            assert!(writer.calls[1].violations.iter().any(|v| v.contains("Remove") && v.contains(token)), "{:?}", writer.calls[1].violations);
            assert!(!result.contains(token));
        }
    }
}
