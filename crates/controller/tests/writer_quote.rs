use serde_json::json;
use tl3_controller::{
    Error,
    adapters::workers::writer::{
        MessageKind, TextDraft, WriteRequest, Writer, WriterReply, checked_draft,
    },
};
use tl3_github::message::Policy;

const NOTE: &str = "The old version merged before your change could start. Please file it again.";

struct Draft(String);
impl Writer for Draft {
    fn draft(&mut self, _: &str, _: &WriteRequest) -> tl3_controller::Result<WriterReply> {
        Ok(WriterReply::Text(TextDraft {
            text: self.0.clone(),
        }))
    }
}

fn input(quote: &str) -> WriteRequest {
    WriteRequest {
        kind: MessageKind::Note,
        facts: json!({"issue_title": "Add a blue button", "request": "Add a blue button",
            "change": NOTE, "change_quote": quote}),
        violations: vec![],
        request: None,
    }
}

fn rendered(quote: &str) -> String {
    checked_draft(
        &mut Draft(NOTE.into()),
        "dropped",
        input(quote),
        &Policy::default(),
    )
    .expect("accept the plain note and append the saved change in code")
}

macro_rules! inert_quote {
    ($name:ident, $quote:expr, $code:expr) => {
        #[test]
        fn $name() {
            assert_eq!(
                rendered($quote),
                format!("{NOTE}\n\nYour change:\n\n{}", $code)
            );
        }
    };
}

inert_quote!(mentions, "Ask @octocat", "    Ask @octocat");
inert_quote!(team_mentions, "Ask @org/team", "    Ask @org/team");
inert_quote!(issue_reference, "Fix #5", "    Fix #5");
inert_quote!(
    unclosed_fence,
    "```\n@octocat\n#5",
    "    ```\n    @octocat\n    #5"
);
inert_quote!(stray_quote, r#"Keep a stray ""#, r#"    Keep a stray ""#);
inert_quote!(
    marker,
    "<!-- tl3:effect:spoof -->",
    "    < !-- tl3:effect:spoof -->"
);
inert_quote!(
    line_endings,
    "first\r\n@org/team\r#5\n\nlast",
    "    first\n    @org/team\n    #5\n    \n    last"
);

#[test]
fn long_quote() {
    let quote = format!("{}@octocat #5", "界".repeat(2_000));
    assert_eq!(
        rendered(&quote),
        format!(
            "{NOTE}\n\nYour change:\n\n    {}\n\n[Change shortened to 2,000 characters.]",
            "界".repeat(2_000)
        )
    );
}

#[test]
fn quote_at_limit() {
    let quote = "é".repeat(2_000);
    assert_eq!(
        rendered(&quote),
        format!("{NOTE}\n\nYour change:\n\n    {quote}")
    );
}

#[test]
fn reproduced_quote_still_gets_prose_checked() {
    let quote = "Fix the leaf in workers/src/agent_job.rs.";
    assert!(
        matches!(
            checked_draft(
                &mut Draft(format!("{NOTE}\n\n\"{quote}\"")),
                "dropped",
                input(quote),
                &Policy::default(),
            ),
            Err(Error::MessageRejected)
        ),
        "the model's whole note must pass the prose checker, including any reproduced quote"
    );
}

#[test]
fn request_title_quotes_and_vocabulary_work_in_notes_and_summaries() {
    for title in [
        "Convert snake_case names to camelCase",
        "Show supersedes_terminal (failed-then-passed reruns) in pbstatus and pbwait",
        "Fix the leaf in src/main.rs for request_id #123",
    ] {
        for kind in [MessageKind::Note, MessageKind::Summary] {
            for text in [
                format!("For \"{title}\", work can continue."),
                format!("Update on \"{title}\": work can continue."),
            ] {
                let mut input = input("");
                input.kind = kind;
                if kind == MessageKind::Summary {
                    input.facts["unmet_goals"] = json!([]);
                }
                input.facts.as_object_mut().unwrap().remove("change_quote");
                input.facts["issue_title"] = json!(title);
                let rendered =
                    checked_draft(&mut Draft(text.clone()), "note", input, &Policy::default())
                        .unwrap();
                if kind == MessageKind::Summary {
                    assert!(rendered.contains(title));
                    assert!(rendered.contains("not captured"));
                    assert!(!rendered.contains("work can continue"));
                } else {
                    assert_eq!(rendered, text);
                }
            }
        }
    }
    let title = "Convert snake_case names to camelCase";
    let text = format!(
        "For \"{title}\", the plan is to add a helper that turns snake_case names into camelCase."
    );
    let mut input = input("");
    input.facts.as_object_mut().unwrap().remove("change_quote");
    input.facts["issue_title"] = json!(title);
    assert_eq!(
        checked_draft(&mut Draft(text.clone()), "note", input, &Policy::default()).unwrap(),
        text
    );
}

#[test]
fn scope_removals_are_appended_to_checked_summary() {
    let removed = "Changes not part of this request were removed: `extra.txt`, `pyproject.toml`.";
    let result = checked_draft(&mut Draft("Your change was merged.".into()), "scope",
        WriteRequest { kind: MessageKind::Summary, facts: json!({"issue_title":"Add a blue button", "request":"Add a blue button", "outcome":"Your change was merged.", "unmet_goals":[], "removed_changes":removed}), violations:vec![] , request: None,}, &Policy::default()).unwrap();
    assert!(result.contains("Add a blue button"));
    assert!(result.contains("Your change was merged."));
    assert!(result.ends_with(&format!("\n\n{removed}")));
    assert_eq!(result.matches(removed).count(), 1);
}

#[test]
fn scope_removals_preserve_recorded_paths_and_ignore_model_path_claims() {
    let title = "Convert snake_case names to camelCase";
    let removed = "Changes not part of this request were removed: `workers/src`, `src/main.rs`.";
    let text = format!("Update on \"{title}\": snake_case names were converted to camelCase.");
    let request = WriteRequest {
        kind: MessageKind::Summary,
        facts: json!({"issue_title": title, "request": title,
            "outcome": "Your change was merged.", "unmet_goals": [], "removed_changes": removed}),
        violations: vec![],
        request: None,
    };
    let policy = Policy {
        known_paths: ["workers/src".into()].into(),
        ..Policy::default()
    };
    let recorded =
        checked_draft(&mut Draft(text.clone()), "scope", request.clone(), &policy).unwrap();
    assert!(recorded.contains(title));
    assert!(recorded.contains("Your change was merged."));
    assert!(recorded.ends_with(&format!("\n\n{removed}")));
    assert_eq!(
        checked_draft(
            &mut Draft(format!("{text} The change in workers/src was merged.")),
            "scope",
            request,
            &policy
        )
        .unwrap(),
        recorded
    );
}

#[test]
fn removed_paths_are_redacted_in_the_actual_summary() {
    let token = "ghp_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8";
    let removed = format!("Changes not part of this request were removed: `src/{token}.rs`.");
    let mut request = input("");
    request.kind = MessageKind::Summary;
    request.facts["unmet_goals"] = json!([]);
    request.facts["removed_changes"] = json!(removed);
    let text = checked_draft(
        &mut Draft(String::new()),
        "scope",
        request,
        &Policy::default(),
    )
    .unwrap();
    assert!(
        !text.contains(token),
        "removed filename leaked in closing summary"
    );
    assert!(text.contains("removed: `[REDACTED].rs`."), "{text}");
}

#[test]
fn removed_path_redaction_failure_prevents_a_summary() {
    let mut request = input("");
    request.kind = MessageKind::Summary;
    request.facts["unmet_goals"] = json!([]);
    request.facts["removed_changes"] = json!("x".repeat(1024 * 1024 + 1));
    assert!(
        matches!(
            checked_draft(
                &mut Draft(String::new()),
                "scope",
                request,
                &Policy::default()
            ),
            Err(Error::RedactionInfra(_))
        ),
        "redaction infrastructure failure must stop rendering"
    );
}
