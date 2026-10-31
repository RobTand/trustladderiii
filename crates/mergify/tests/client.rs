use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};
use tl3_github::{AppConfig, CheckRun, ClientOptions, GitHub, PullRequest};
use tl3_mergify::{Error, Mergify, QUEUE_LABEL, QueueState, is_merge_queue_pr, observe};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, header, method, path, query_param},
};

fn fixture(name: &str, part: &str) -> Value {
    let file = format!(
        "{}/tests/fixtures/{name}/{part}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap()
}

fn recorded(name: &str) -> (PullRequest, Vec<CheckRun>) {
    (
        serde_json::from_value(fixture(name, "pull")).unwrap(),
        serde_json::from_value(fixture(name, "checks")).unwrap(),
    )
}

#[test]
fn recorded_checks_recognize_queue_facts() {
    for (name, expected) in [
        ("merged", QueueState::Merged),
        ("waiting", QueueState::Waiting),
        ("running", QueueState::ChecksRunning),
        ("failed", QueueState::FailedCi),
        ("conflict", QueueState::Conflict),
    ] {
        let (pr, checks) = recorded(name);
        let approved = pr.head.sha.clone();
        let result = observe(pr, &checks, &approved);
        assert_eq!(result.state, expected, "{name}");
        assert!(!result.unapproved_merge, "{name}");
    }
}

#[test]
fn documented_queue_title_distinguishes_waiting_from_running() {
    let (pr, mut checks) = recorded("running");
    let approved = pr.head.sha.clone();
    let check = checks
        .iter_mut()
        .find(|c| c.name == tl3_mergify::QUEUE_CHECK)
        .unwrap();
    check.output.as_mut().unwrap().title = Some("In merge queue".into());
    assert_eq!(observe(pr, &checks, &approved).state, QueueState::Queued);
}

#[test]
fn temporary_queue_prs_require_both_the_branch_prefix_and_mergify_author() {
    let (original, _) = recorded("running");
    for (branch, author, expected) in [
        ("mergify/merge-queue/main/pr-1", "mergify[bot]", true),
        ("mergify/merge-queue/main/pr-1", "someone", false),
        ("feature", "mergify[bot]", false),
        ("mergify/bp/main/pr-1", "mergify[bot]", false),
        ("mergify/merge-queue-spoof/main", "mergify[bot]", false),
    ] {
        let mut pr = original.clone();
        pr.head.branch = branch.into();
        pr.user.login = author.into();
        assert_eq!(is_merge_queue_pr(&pr), expected, "{branch}, {author}");
    }
}

#[test]
fn merged_head_without_tl3_approval_is_flagged_even_without_checks() {
    let (pr, checks) = recorded("merged");
    let approved = "different-approved-head";
    for checks in [&checks[..], &[]] {
        let result = observe(pr.clone(), checks, approved);
        assert_eq!(result.state, QueueState::Merged);
        assert!(result.unapproved_merge);
        assert_eq!(result.pull_request.head.sha, pr.head.sha);
    }
    let (pr, checks) = recorded("running");
    assert!(!observe(pr, &checks, approved).unapproved_merge);
}

#[test]
fn ignores_unrelated_spoofed_and_old_head_checks() {
    let (pr, checks) = recorded("running");
    let original = checks
        .iter()
        .find(|c| c.name == tl3_mergify::QUEUE_CHECK)
        .unwrap();
    for change in [0, 1, 2, 3, 4] {
        let mut check = original.clone();
        match change {
            0 => check.name = "Summary".into(),
            1 => check.head_sha = "previous-head".into(),
            2 => check.app.as_mut().unwrap().id = 1,
            3 => check.app.as_mut().unwrap().slug = "unrelated".into(),
            4 => check.app = None,
            _ => unreachable!(),
        }
        assert_eq!(
            observe(pr.clone(), &[check], &pr.head.sha).state,
            QueueState::Unknown
        );
    }
}

#[test]
fn latest_check_wins() {
    let (pr, mut checks) = recorded("waiting");
    let approved = pr.head.sha.clone();
    let mut older = checks
        .iter()
        .find(|c| c.name == tl3_mergify::QUEUE_CHECK)
        .unwrap()
        .clone();
    older.id = 1;
    older.output.as_mut().unwrap().title = Some("Dequeued — checks failed".into());
    older.output.as_mut().unwrap().summary = None;
    checks.push(older);
    checks.reverse();
    assert_eq!(observe(pr, &checks, &approved).state, QueueState::Waiting);
}

#[test]
fn only_github_confirms_merge_even_when_queue_check_claims_success() {
    let (mut pr, checks) = recorded("merged");
    let approved = pr.head.sha.clone();
    pr.merged_at = None;
    pr.state = "open".into();
    let result = observe(pr, &checks, &approved);
    assert_eq!(result.state, QueueState::Unknown);
    assert!(!result.unapproved_merge);
}

#[test]
fn invalid_or_future_metadata_stays_unknown() {
    let (pr, checks) = recorded("running");
    let mut check = checks
        .into_iter()
        .find(|c| c.name == tl3_mergify::QUEUE_CHECK)
        .unwrap();
    for payload in [
        "not json",
        r#"{"version":2,"state":"queued"}"#,
        r#"{"version":1,"state":"future"}"#,
    ] {
        check.output.as_mut().unwrap().summary = Some(format!(
            "<span itemprop=\"mergify.v1.{}\"></span>",
            STANDARD.encode(payload)
        ));
        assert_eq!(
            observe(pr.clone(), &[check.clone()], &pr.head.sha).state,
            QueueState::Unknown
        );
    }
    check.output.as_mut().unwrap().summary = Some("<span itemprop=\"mergify.v1.!\"></span>".into());
    assert_eq!(
        observe(pr.clone(), &[check.clone()], &pr.head.sha).state,
        QueueState::Unknown
    );
    check.output = None;
    assert_eq!(
        observe(pr.clone(), &[check], &pr.head.sha).state,
        QueueState::Unknown
    );
}

#[test]
fn machine_metadata_does_not_confuse_interruption_or_deferred_checks_with_failure() {
    let (pr, checks) = recorded("running");
    let mut check = checks
        .into_iter()
        .find(|c| c.name == tl3_mergify::QUEUE_CHECK)
        .unwrap();
    for (payload, expected) in [
        (json!({"version":1,"state":"queued"}), QueueState::Queued),
        (
            json!({"version":1,"state":"checking","step":"checks_deferred"}),
            QueueState::Queued,
        ),
        (
            json!({"version":1,"state":"checking","step":"future"}),
            QueueState::Unknown,
        ),
        (
            json!({"version":1,"state":"dequeued","dequeue_reason":{"code":"CHECKS_TIMEOUT"}}),
            QueueState::FailedCi,
        ),
        // An interrupted run says nothing about the change; TL3 re-enqueues it as an infrastructure failure.
        (
            json!({"version":1,"state":"dequeued","dequeue_reason":{"code":"CHECKS_INTERRUPTED"}}),
            QueueState::FailedCi,
        ),
    ] {
        check.output.as_mut().unwrap().summary = Some(format!(
            "<span itemprop=\"mergify.v1.{}\"></span>",
            STANDARD.encode(payload.to_string())
        ));
        assert_eq!(
            observe(pr.clone(), &[check.clone()], &pr.head.sha).state,
            expected
        );
        assert_eq!(
            tl3_mergify::ci_is_infrastructure(&check),
            payload["dequeue_reason"]["code"] == "CHECKS_TIMEOUT"
                || payload["dequeue_reason"]["code"] == "CHECKS_INTERRUPTED"
        );
    }
}

#[test]
fn generic_dequeue_on_a_dirty_pull_is_conflict() {
    for (mergeable, mergeable_state) in [
        (json!(false), json!("dirty")),
        (Value::Null, json!("dirty")),
        (json!(false), json!("unknown")),
    ] {
        let mut pr = fixture("failed", "pull");
        pr["mergeable"] = mergeable;
        pr["mergeable_state"] = mergeable_state;
        let pr: PullRequest = serde_json::from_value(pr).unwrap();
        let checks: Vec<CheckRun> = serde_json::from_value(fixture("dequeued", "checks")).unwrap();
        assert_eq!(
            observe(pr.clone(), &checks, &pr.head.sha).state,
            QueueState::Conflict
        );
    }
}

#[test]
fn generic_dequeue_on_a_clean_pull_retains_the_dequeue() {
    let mut pr = fixture("failed", "pull");
    pr["mergeable"] = json!(true);
    pr["mergeable_state"] = json!("clean");
    let pr: PullRequest = serde_json::from_value(pr).unwrap();
    let checks: Vec<CheckRun> = serde_json::from_value(fixture("dequeued", "checks")).unwrap();
    assert_eq!(
        observe(pr.clone(), &checks, &pr.head.sha).state,
        QueueState::Dequeued
    );
}

#[test]
fn recorded_live_dequeue_without_machine_metadata_is_conflict() {
    let mut pr = fixture("failed", "pull");
    let checks: Vec<CheckRun> = serde_json::from_value(fixture("live-dequeued", "checks")).unwrap();
    pr["head"]["sha"] = json!(checks[0].head_sha);
    pr["mergeable"] = json!(false);
    pr["mergeable_state"] = json!("dirty");
    let pr: PullRequest = serde_json::from_value(pr).unwrap();
    assert_eq!(
        observe(pr.clone(), &checks, &pr.head.sha).state,
        QueueState::Conflict
    );
}

#[test]
fn dequeue_titles_report_terminal_state_without_machine_metadata() {
    let (pr, _) = recorded("running");
    let mut check: CheckRun =
        serde_json::from_value(fixture("live-dequeued", "checks")[0].clone()).unwrap();
    check.head_sha = pr.head.sha.clone();
    check.output.as_mut().unwrap().summary = None;
    for (title, expected, infra) in [
        ("Dequeued — checks interrupted", QueueState::FailedCi, true),
        ("Dequeued — checks timeout", QueueState::FailedCi, true),
        (
            "Dequeued — conflict with another pull request",
            QueueState::Conflict,
            false,
        ),
        ("Dequeued — a future reason", QueueState::Dequeued, false),
    ] {
        check.output.as_mut().unwrap().title = Some(title.into());
        assert_eq!(
            observe(pr.clone(), &[check.clone()], &pr.head.sha).state,
            expected,
            "{title}"
        );
        assert_eq!(tl3_mergify::ci_is_infrastructure(&check), infra, "{title}");
    }
}

#[test]
fn recorded_live_queued_stub_confirms_queue_membership() {
    let (pr, mut checks) = recorded("running");
    let check = checks
        .iter_mut()
        .find(|check| check.name == tl3_mergify::QUEUE_CHECK)
        .unwrap();
    check.status = "completed".into();
    check.conclusion = Some("neutral".into());
    check.output.as_mut().unwrap().title = Some("The pull request #75 is queued".into());
    check.output.as_mut().unwrap().summary = Some("The pull request #75 is queued, this summary will be updated once the queue checks have started.".into());
    assert_eq!(
        observe(pr.clone(), &checks, &pr.head.sha).state,
        QueueState::Queued
    );
}

async fn client(server: &MockServer) -> Mergify {
    Mock::given(method("POST"))
        .and(path("/app/installations/3/access_tokens"))
        .respond_with(
            ResponseTemplate::new(201)
                .set_body_json(json!({"token":"fake-token","expires_at":"2099-01-01T00:00:00Z"})),
        )
        .expect(1)
        .mount(server)
        .await;
    let github = GitHub::new(
        AppConfig {
            id: 123,
            slug: "factory".into(),
            pem: include_str!("fixtures/throwaway-key.pem").into(),
            webhook_secret: "fake".into(),
        },
        3,
        ClientOptions {
            api_url: format!("{}/", server.uri()).parse().unwrap(),
            mutation_interval: Duration::ZERO,
            max_retries: 0,
            ..ClientOptions::default()
        },
    )
    .unwrap();
    Mergify::new(github)
}

async fn get(server: &MockServer, endpoint: &str, value: Value, count: u64) {
    Mock::given(method("GET"))
        .and(path(endpoint))
        .and(header("authorization", "Bearer fake-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(value))
        .expect(count)
        .mount(server)
        .await;
}

#[tokio::test]
async fn reads_recorded_state_through_github_rest() {
    for (name, expected) in [
        ("running", QueueState::ChecksRunning),
        ("failed", QueueState::FailedCi),
        ("merged", QueueState::Merged),
        ("conflict", QueueState::Conflict),
    ] {
        let server = MockServer::start().await;
        let client = client(&server).await;
        let pr = fixture(name, "pull");
        let number = pr["number"].as_u64().unwrap();
        let approved = if name == "merged" {
            "different-approved-head"
        } else {
            pr["head"]["sha"].as_str().unwrap()
        };
        let prefix = "/repos/test/repo";
        get(&server, &format!("{prefix}/pulls/{number}"), pr.clone(), 2).await;
        Mock::given(method("GET"))
            .and(path(format!(
                "{prefix}/commits/{}/check-runs",
                pr["head"]["sha"].as_str().unwrap()
            )))
            .and(query_param("filter", "all"))
            .and(query_param("per_page", "100"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"check_runs":fixture(name,"checks")})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let result = client.read("test/repo", number, approved).await.unwrap();
        assert_eq!(result.state, expected);
        assert_eq!(result.unapproved_merge, name == "merged");
    }
}

#[tokio::test]
async fn queue_and_dequeue_converge_when_label_is_already_present_or_absent() {
    let server = MockServer::start().await;
    let client = client(&server).await;
    let pr = fixture("waiting", "pull");
    get(&server, "/repos/test/repo/pulls/2425", pr.clone(), 2).await;
    Mock::given(method("POST"))
        .and(path("/repos/test/repo/issues/2425/labels"))
        .and(header("authorization", "Bearer fake-token"))
        .and(body_json(json!({"labels": [QUEUE_LABEL]})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"name": "unrelated"}, {"name": QUEUE_LABEL}
        ])))
        .expect(2)
        .mount(&server)
        .await;
    let calls = AtomicUsize::new(0);
    Mock::given(method("DELETE"))
        .and(path(format!(
            "/repos/test/repo/issues/2425/labels/{QUEUE_LABEL}"
        )))
        .and(header("authorization", "Bearer fake-token"))
        .respond_with(move |_: &wiremock::Request| {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                ResponseTemplate::new(200).set_body_json(json!([{"name": "unrelated"}]))
            } else {
                ResponseTemplate::new(404)
            }
        })
        .expect(2)
        .mount(&server)
        .await;
    let head = pr["head"]["sha"].as_str().unwrap();
    client.queue("test/repo", 2425, head).await.unwrap();
    client.queue("test/repo", 2425, head).await.unwrap();
    client.dequeue("test/repo", 2425).await.unwrap();
    client.dequeue("test/repo", 2425).await.unwrap();
    assert!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|r| { !r.url.path().contains("comments") && r.method.as_str() != "PUT" })
    );
}

#[tokio::test]
async fn refuses_unapproved_or_closed_heads_before_adding_the_label() {
    for closed in [false, true] {
        let server = MockServer::start().await;
        let client = client(&server).await;
        let mut pr = fixture("waiting", "pull");
        let approved = if closed {
            pr["state"] = json!("closed");
            pr["head"]["sha"].as_str().unwrap()
        } else {
            "old-head"
        };
        get(&server, "/repos/test/repo/pulls/2425", pr.clone(), 1).await;
        let result = client.queue("test/repo", 2425, approved).await;
        assert!(matches!(
            (closed, result),
            (false, Err(Error::HeadChanged)) | (true, Err(Error::PullRequestClosed))
        ));
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }
}

#[tokio::test]
async fn a_push_during_read_requires_a_new_observation() {
    let server = MockServer::start().await;
    let client = client(&server).await;
    let mut pr = fixture("waiting", "pull");
    let before = pr.clone();
    pr["head"]["sha"] = json!("new-head");
    let calls = AtomicUsize::new(0);
    Mock::given(method("GET"))
        .and(path("/repos/test/repo/pulls/2425"))
        .respond_with(move |_: &wiremock::Request| {
            ResponseTemplate::new(200).set_body_json(if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                before.clone()
            } else {
                pr.clone()
            })
        })
        .expect(2)
        .mount(&server)
        .await;
    let head = fixture("waiting", "pull")["head"]["sha"]
        .as_str()
        .unwrap()
        .to_string();
    get(
        &server,
        &format!("/repos/test/repo/commits/{head}/check-runs"),
        json!({"check_runs":[]}),
        1,
    )
    .await;
    assert!(matches!(
        client.read("test/repo", 2425, &head).await,
        Err(Error::ChangedDuringRead)
    ));
}

#[tokio::test]
async fn propagates_github_failures_without_inventing_facts() {
    let server = MockServer::start().await;
    let client = client(&server).await;
    Mock::given(method("GET"))
        .and(path("/repos/test/repo/pulls/7"))
        .respond_with(ResponseTemplate::new(403))
        .expect(1)
        .mount(&server)
        .await;
    assert!(matches!(
        client.read("test/repo", 7, "approved").await,
        Err(Error::GitHub(tl3_github::Error::Http { status: 403 }))
    ));
    Mock::given(method("DELETE"))
        .and(path(format!(
            "/repos/test/repo/issues/7/labels/{QUEUE_LABEL}"
        )))
        .respond_with(ResponseTemplate::new(403))
        .expect(1)
        .mount(&server)
        .await;
    assert!(matches!(
        client.dequeue("test/repo", 7).await,
        Err(Error::GitHub(tl3_github::Error::Http { status: 403 }))
    ));
}

#[test]
fn future_encoding_and_unrecorded_titles_stay_unknown() {
    let (pr, checks) = recorded("running");
    let mut check = checks
        .into_iter()
        .find(|c| c.name == tl3_mergify::QUEUE_CHECK)
        .unwrap();
    check.output.as_mut().unwrap().summary =
        Some("<span itemprop=\"mergify.v2.payload\"></span>".into());
    assert_eq!(
        observe(pr.clone(), &[check.clone()], &pr.head.sha).state,
        QueueState::Unknown
    );
    check.output.as_mut().unwrap().summary = None;
    for title in [
        "Waiting for queue conditions",
        "Waiting for queue conditions to match",
        "A future queue state",
    ] {
        check.output.as_mut().unwrap().title = Some(title.into());
        assert_eq!(
            observe(pr.clone(), &[check.clone()], &pr.head.sha).state,
            QueueState::Unknown,
            "{title}"
        );
    }
}

#[test]
fn a_timed_out_queue_check_is_an_infrastructure_failure_without_a_known_title() {
    let (pr, mut checks) = recorded("running");
    let check = checks
        .iter_mut()
        .find(|check| check.name == tl3_mergify::QUEUE_CHECK)
        .unwrap();
    check.status = "completed".into();
    check.conclusion = Some("timed_out".into());
    check.output = None;
    let state = observe(pr.clone(), &checks, &pr.head.sha);
    assert_eq!(state.state, QueueState::FailedCi);
    assert!(tl3_mergify::ci_is_infrastructure(
        state.check.as_ref().unwrap()
    ));
}

#[test]
fn documented_manual_dequeue_is_not_a_retry_or_conflict() {
    let (mut pr, _) = recorded("running");
    pr.mergeable = Some(false);
    pr.mergeable_state = Some("dirty".into());
    let mut check: CheckRun =
        serde_json::from_value(fixture("manual", "checks")[0].clone()).unwrap();
    check.head_sha = pr.head.sha.clone();
    for title_only in [false, true] {
        if title_only {
            let output = check.output.as_mut().unwrap();
            output.summary = None;
            output.title = Some("Dequeued — pull request manually dequeued".into());
        }
        let state = observe(pr.clone(), &[check.clone()], &pr.head.sha).state;
        assert!(
            !matches!(
                state,
                QueueState::Dequeued
                    | QueueState::FailedCi
                    | QueueState::Conflict
                    | QueueState::Unknown
            ),
            "a documented manual dequeue must report a person stop, even on a dirty PR"
        );
    }
}
