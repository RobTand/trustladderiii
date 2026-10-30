use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::collections::BTreeMap;
use tl3_github::webhook::*;

#[derive(Default)]
struct MemoryInbox {
    deliveries: BTreeMap<String, Delivery>,
}

impl Inbox for MemoryInbox {
    type Error = std::convert::Infallible;
    fn insert(&mut self, delivery: Delivery) -> Result<bool, Self::Error> {
        use std::collections::btree_map::Entry;
        match self.deliveries.entry(delivery.id.clone()) {
            Entry::Vacant(entry) => {
                entry.insert(delivery);
                Ok(true)
            }
            Entry::Occupied(_) => Ok(false),
        }
    }
}

fn signature(body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(b"throwaway").unwrap();
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

#[test]
fn recorded_payloads_parse_into_the_expected_variants() {
    let fixtures = [
        (
            "issues",
            include_bytes!("fixtures/issues-labeled.json").as_slice(),
            0,
        ),
        (
            "issue_comment",
            include_bytes!("fixtures/issue-comment-created.json").as_slice(),
            1,
        ),
        (
            "pull_request",
            include_bytes!("fixtures/pull-request-closed.json").as_slice(),
            2,
        ),
        (
            "pull_request",
            include_bytes!("fixtures/pull-request-merged.json").as_slice(),
            3,
        ),
        (
            "pull_request",
            include_bytes!("fixtures/pull-request-synchronize.json").as_slice(),
            4,
        ),
        (
            "check_run",
            include_bytes!("fixtures/check-run-completed.json").as_slice(),
            5,
        ),
        (
            "check_suite",
            include_bytes!("fixtures/check-suite-completed.json").as_slice(),
            6,
        ),
        (
            "pull_request",
            include_bytes!("fixtures/pull-request-unlabeled.json").as_slice(),
            7,
        ),
        (
            "merge_group",
            include_bytes!("fixtures/merge-group-checks-requested.json").as_slice(),
            8,
        ),
    ];
    for (name, body, expected) in fixtures {
        let delivery =
            verify_and_parse(b"throwaway", &signature(body), "delivery-1", name, body).unwrap();
        let event = delivery.event.unwrap();
        let actual = match event {
            Event::PullRequestUnlabeled { label, .. } => {
                assert_eq!(label.name, "tl3-queue");
                7
            }
            Event::MergeGroup { merge_group, .. } => {
                assert_eq!(merge_group.head_sha, "batch-head");
                8
            }
            Event::PullRequestUpdated { .. } | Event::CheckRunUpdated { .. } => {
                panic!("unexpected variant")
            }
            Event::IssueLabeled {
                actor,
                label,
                context,
                issue,
            } => {
                assert_eq!(actor.id, 42);
                assert_eq!(label.name, "tl3");
                assert_eq!(context.installation_id, 3);
                assert_eq!(context.repository.full_name(), "owner/sandbox");
                assert_eq!(issue.number, 12);
                0
            }
            Event::CommentCreated { comment, actor, .. } => {
                assert_eq!(comment.body, "Please continue.");
                assert_eq!(actor.id, 42);
                1
            }
            Event::PullRequestClosed { pull_request, .. } => {
                assert!(pull_request.merged_at.is_none());
                2
            }
            Event::PullRequestMerged { pull_request, .. } => {
                assert!(pull_request.merged_at.is_some());
                3
            }
            Event::PullRequestSynchronized { before, after, .. } => {
                assert_eq!(before, "def5678");
                assert_eq!(after, "abc1234");
                4
            }
            Event::CheckRunCompleted { check_run, .. } => {
                assert_eq!(check_run.conclusion.as_deref(), Some("success"));
                5
            }
            Event::CheckSuiteCompleted { check_suite, .. } => {
                assert_eq!(check_suite.id, 23);
                6
            }
        };
        assert_eq!(actual, expected);
    }
}

#[test]
fn only_robs_label_on_an_issue_can_start_work() {
    let body = include_bytes!("fixtures/issues-labeled.json");
    let mut event = verify_and_parse(b"throwaway", &signature(body), "delivery-2", "issues", body)
        .unwrap()
        .event
        .unwrap();
    assert!(event.starts_request(42));
    assert!(!event.starts_request(99));
    if let Event::IssueLabeled { issue, .. } = &mut event {
        issue.pull_request = Some(tl3_github::PullLink {
            url: "https://example.invalid/".into(),
        });
    }
    assert!(!event.starts_request(42));
}

#[test]
fn bad_signatures_and_bad_payloads_do_not_consume_delivery_ids() {
    let body = include_bytes!("fixtures/issues-labeled.json");
    let mut inbox = MemoryInbox::default();
    for sig in ["", "sha1=abcd", "sha256=zz", "sha256=0000"] {
        assert!(matches!(
            receive(b"throwaway", sig, "delivery-3", "issues", body, &mut inbox),
            Err(ReceiveError::Webhook(WebhookError::Signature))
        ));
    }
    let mut altered = body.to_vec();
    altered.push(b' ');
    assert!(
        verify_and_parse(
            b"throwaway",
            &signature(body),
            "delivery-3",
            "issues",
            &altered
        )
        .is_err()
    );
    assert!(inbox.deliveries.is_empty());
    assert!(
        receive(
            b"throwaway",
            &signature(body),
            "delivery-3",
            "issues",
            body,
            &mut inbox
        )
        .unwrap()
    );
    assert!(
        !receive(
            b"throwaway",
            &signature(body),
            "delivery-3",
            "issues",
            body,
            &mut inbox
        )
        .unwrap()
    );
    assert_eq!(inbox.deliveries.len(), 1);
    let malformed = br#"{"action":"labeled"}"#;
    assert_eq!(
        verify_and_parse(
            b"throwaway",
            &signature(malformed),
            "delivery-4",
            "issues",
            malformed
        )
        .unwrap_err(),
        WebhookError::Payload
    );
    let ignored = br#"{"action":"edited"}"#;
    assert!(
        verify_and_parse(
            b"throwaway",
            &signature(ignored),
            "delivery-4",
            "issues",
            ignored
        )
        .unwrap()
        .event
        .is_none()
    );
}

#[test]
fn check_completions_do_not_require_a_sender() {
    let mut payload: serde_json::Value =
        serde_json::from_slice(include_bytes!("fixtures/check-run-completed.json")).unwrap();
    payload.as_object_mut().unwrap().remove("sender");
    let body = serde_json::to_vec(&payload).unwrap();
    assert!(
        verify_and_parse(
            b"throwaway",
            &signature(&body),
            "delivery-5",
            "check_run",
            &body
        )
        .is_ok()
    );
}

#[test]
fn inbox_failure_is_retryable_without_losing_the_event() {
    struct Broken;
    impl Inbox for Broken {
        type Error = ();
        fn insert(&mut self, _: Delivery) -> Result<bool, ()> {
            Err(())
        }
    }
    let body = include_bytes!("fixtures/issues-labeled.json");
    assert!(matches!(
        receive(
            b"throwaway",
            &signature(body),
            "delivery-6",
            "issues",
            body,
            &mut Broken
        ),
        Err(ReceiveError::Inbox(()))
    ));
    let mut inbox = MemoryInbox::default();
    assert!(
        receive(
            b"throwaway",
            &signature(body),
            "delivery-6",
            "issues",
            body,
            &mut inbox
        )
        .unwrap()
    );
}

#[test]
fn pull_request_updates_and_merge_groups_retain_typed_and_raw_evidence() {
    let mut pull: serde_json::Value =
        serde_json::from_slice(include_bytes!("fixtures/pull-request-synchronize.json")).unwrap();
    pull["action"] = serde_json::json!("opened");
    let merge = serde_json::json!({"action": "checks_requested", "repository": pull["repository"], "installation": pull["installation"],
        "merge_group": {"head_sha": "abc1234", "head_ref": "queue/main/test", "base_sha": "def5678", "base_ref": "main"}});
    for (name, payload) in [
        ("pull_request", pull),
        ("merge_group", merge),
        (
            "ping",
            serde_json::json!({"zen": "Retain the verified event"}),
        ),
    ] {
        let body = serde_json::to_vec(&payload).unwrap();
        let delivery =
            verify_and_parse(b"throwaway", &signature(&body), "delivery-7", name, &body).unwrap();
        assert_eq!(delivery.payload, payload);
        assert_eq!(delivery.event_name, name);
        match name {
            "pull_request" => assert!(
                matches!(delivery.event, Some(Event::PullRequestUpdated { action, .. }) if action == "opened")
            ),
            "merge_group" => assert!(
                matches!(delivery.event, Some(Event::MergeGroup { action, .. }) if action == "checks_requested")
            ),
            _ => assert!(delivery.event.is_none()),
        }
    }
}
