use serde_json::json;
use tl3_github::{
    closing_issues,
    webhook::{self, Event},
};

#[test]
fn adoption_closing_keywords_are_local_unique_and_unambiguous() {
    assert_eq!(closing_issues("Closes #42"), vec![42]);
    assert_eq!(
        closing_issues("fixed #7; RESOLVES #42; closes #7"),
        vec![7, 42]
    );
    assert!(
        closing_issues("References #42, discloses #8, closes other/repo#9, fixes #0").is_empty()
    );
    assert!(closing_issues("Closes #42suffix or fixes #18446744073709551616").is_empty());
}

#[test]
fn adoption_webhook_keeps_label_actor_body_and_pinned_head() {
    let mut body: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/pull-request-merged.json")).unwrap();
    body["action"] = json!("labeled");
    body["label"] = json!({"name":"tl3"});
    body["pull_request"]["body"] = json!("Closes #42");
    let parsed = webhook::parse(
        "adopt-17",
        "pull_request",
        &serde_json::to_vec(&body).unwrap(),
    )
    .unwrap();
    let Some(Event::PullRequestLabeled {
        pull_request,
        actor,
        label,
        ..
    }) = parsed.event
    else {
        panic!("labelled PR evidence was lost")
    };
    assert_eq!(label.name, "tl3");
    assert_eq!(actor.id, body["sender"]["id"].as_u64().unwrap());
    assert_eq!(pull_request.body.as_deref(), Some("Closes #42"));
    assert_eq!(pull_request.head.sha, body["pull_request"]["head"]["sha"]);
}
