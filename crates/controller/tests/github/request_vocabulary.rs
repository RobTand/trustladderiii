use super::*;
use tl3_controller::adapters::workers::writer::TextDraft;

struct RequestWriter {
    text: String,
    calls: usize,
}

impl Writer for RequestWriter {
    fn draft(&mut self, _: &str, _: &WriteRequest) -> tl3_controller::Result<WriterReply> {
        self.calls += 1;
        Ok(WriterReply::Text(TextDraft {
            text: self.text.clone(),
        }))
    }
}

pub(super) fn all() {
    for mode in [
        "title",
        "body",
        "incident",
        "negative",
        "scope",
        "scope-path",
    ] {
        run(mode);
    }
}

pub(super) fn run(mode: &str) {
    let title = if mode == "incident" {
        "Show supersedes_terminal (failed-then-passed reruns) in pbstatus and pbwait"
    } else {
        "Convert snake_case names to camelCase"
    };
    let text = match mode {
        "title" => format!(
            "For \"{title}\", the plan is to add a helper that turns snake_case names into camelCase."
        ),
        "body" => "The storage_pool names can be converted.".into(),
        "incident" => format!("Update on \"{title}\": supersedes_terminal needs your help."),
        "negative" => "The other_word needs help.".into(),
        "scope" => format!("Update on \"{title}\": snake_case names were converted to camelCase."),
        "scope-path" => "The change in workers/src was merged.".into(),
        _ => panic!("unknown vocabulary case"),
    };
    let api = Server::new();
    let mut issue = fixture(false)["issue"].clone();
    issue["title"] = json!(title);
    issue["body"] = json!("Also convert storage_pool names.");
    api.runtime.block_on(async {
        Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/repos/owner/sandbox/issues/12"))
            .respond_with(ResponseTemplate::new(200).set_body_json(issue))
            .with_priority(1)
            .mount(&api.server)
            .await;
    });
    let root = tempfile::tempdir().unwrap();
    let mut adapter = adapters::github::Adapter::new(
        BTreeMap::from([(3, client(&api.uri()))]),
        RequestWriter {
            text: text.clone(),
            calls: 0,
        },
        0,
        Policy::default(),
        root.path(),
        Some(tl3_config::OperationsIssue {
            owner: "owner".into(),
            name: "tl3".into(),
            installation_id: 3,
            number: 17,
        }),
    )
    .unwrap();
    let mut effect = delivery(Notice::Note(tl3_kernel::NoteKind::Replanned));
    if mode == "incident" {
        effect.output = Out::Notice {
            node: NodeId(0),
            notice: Notice::Incident { needs_rob: true },
        };
        effect.repository.name = "tl3".into();
        effect.issue = 17;
        effect.question_context = Some(json!({
            "issue_title": title,
            "request": "Fix the request (https://github.com/owner/sandbox/issues/12)",
            "incident_id": "5.0",
            "signature": "GitHub:permission",
        }));
    }
    let removed = "Changes not part of this request were removed: `workers/src`, `src/main.rs`.";
    if matches!(mode, "scope" | "scope-path") {
        effect.output = Out::Notice {
            node: NodeId(0),
            notice: Notice::Summary(Outcome::Merged),
        };
        effect.paths = vec!["workers/src".into()];
        effect.scope_removals = Some(removed.into());
    }
    let completion = adapter.execute(&effect).unwrap();
    if matches!(mode, "negative" | "scope-path") {
        assert_ne!(completion.ack, tl3_kernel::Ack::Confirmed);
        assert_eq!(api.state.lock().unwrap().posts, 0);
    } else {
        assert_eq!(
            completion.ack,
            tl3_kernel::Ack::Confirmed,
            "{mode}: request vocabulary must reach the checker"
        );
        adapter.execute(&effect).unwrap();
        let state = api.state.lock().unwrap();
        assert_eq!(state.posts, 1);
        let posted = state.comments[0]["body"].as_str().unwrap();
        assert!(posted.contains(&text));
        if mode == "scope" {
            assert_eq!(posted.matches(removed).count(), 1);
        }
    }
    println!("test request_vocabulary_{mode} ... ok");
}
