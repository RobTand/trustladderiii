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
inert_quote!(unclosed_fence, "```\n@octocat\n#5", "    ```\n    @octocat\n    #5");
inert_quote!(stray_quote, r#"Keep a stray ""#, r#"    Keep a stray ""#);
inert_quote!(marker, "<!-- tl3:effect:spoof -->", "    < !-- tl3:effect:spoof -->");
inert_quote!(line_endings, "first\r\n@org/team\r#5\n\nlast", "    first\n    @org/team\n    #5\n    \n    last");

#[test]
fn long_quote() {
    let quote = format!("{}@octocat #5", "界".repeat(2_000));
    assert_eq!(
        rendered(&quote),
        format!("{NOTE}\n\nYour change:\n\n    {}\n\n[Change shortened to 2,000 characters.]", "界".repeat(2_000))
    );
}

#[test]
fn quote_at_limit() {
    let quote = "é".repeat(2_000);
    assert_eq!(rendered(&quote), format!("{NOTE}\n\nYour change:\n\n    {quote}"));
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
