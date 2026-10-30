use base64::{Engine, engine::general_purpose::STANDARD};
use rand::rngs::OsRng;
use rsa::{
    RsaPrivateKey,
    pkcs8::{EncodePrivateKey, LineEnding},
};
use serde_json::{Value, json};
use std::{
    sync::{
        LazyLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tl3_github::{AppConfig, CheckRun, ClientOptions, Comment, GitHub, PullRequest};
use tl3_mergify::{Error, Mergify, QueueState, observe};
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

fn recorded(name: &str) -> (PullRequest, Vec<CheckRun>, Vec<Comment>) {
    (
        serde_json::from_value(fixture(name, "pull")).unwrap(),
        serde_json::from_value(fixture(name, "checks")).unwrap(),
        serde_json::from_value(fixture(name, "comments")).unwrap(),
    )
}

#[test]
fn recorded_checks_and_comments_recognize_queue_facts() {
    for (name, expected) in [
        ("merged", QueueState::Merged),
        ("waiting", QueueState::Waiting),
        ("running", QueueState::ChecksRunning),
        ("failed", QueueState::FailedCi),
        ("conflict", QueueState::Conflict),
    ] {
        let (pr, checks, comments) = recorded(name);
        let result = observe(pr, &checks, &comments);
        assert_eq!(result.state, expected, "{name}");
        if name != "waiting" {
            assert!(result.events.iter().any(|e| e.state == expected), "{name}");
        }
        if name == "failed" {
            assert!(
                result
                    .events
                    .iter()
                    .any(|e| e.state == QueueState::Dequeued)
            );
        }
    }
}

#[test]
fn documented_queue_title_distinguishes_waiting_from_running() {
    let (pr, mut checks, _) = recorded("running");
    let check = checks
        .iter_mut()
        .find(|c| c.name == tl3_mergify::QUEUE_CHECK)
        .unwrap();
    check.output.as_mut().unwrap().title = "In merge queue".into();
    assert_eq!(observe(pr, &checks, &[]).state, QueueState::Queued);
}

#[test]
fn ignores_unrelated_spoofed_and_old_head_checks() {
    let (pr, checks, comments) = recorded("running");
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
            observe(pr.clone(), &[check], &comments).state,
            QueueState::Unknown
        );
    }
}

#[test]
fn latest_check_wins_and_historical_comments_never_override_it() {
    let (pr, mut checks, _) = recorded("waiting");
    let (_, _, comments) = recorded("failed");
    let mut older = checks
        .iter()
        .find(|c| c.name == tl3_mergify::QUEUE_CHECK)
        .unwrap()
        .clone();
    older.id = 1;
    older.output.as_mut().unwrap().title = "Dequeued — checks failed".into();
    older.output.as_mut().unwrap().summary = None;
    checks.push(older);
    checks.reverse();
    let result = observe(pr, &checks, &comments);
    assert_eq!(result.state, QueueState::Waiting);
    assert!(
        result
            .events
            .iter()
            .any(|e| e.state == QueueState::FailedCi)
    );
}

#[test]
fn only_github_confirms_merge_even_when_queue_check_claims_success() {
    let (mut pr, checks, comments) = recorded("merged");
    pr.merged_at = None;
    pr.state = "open".into();
    assert_eq!(
        observe(pr.clone(), &checks, &comments).state,
        QueueState::Unknown
    );
    pr.merged_at = Some(chrono::Utc::now());
    assert_eq!(observe(pr, &[], &[]).state, QueueState::Merged);
}

#[test]
fn invalid_or_future_metadata_stays_unknown() {
    let (pr, checks, _) = recorded("running");
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
            observe(pr.clone(), &[check.clone()], &[]).state,
            QueueState::Unknown
        );
    }
    check.output.as_mut().unwrap().summary = Some("<span itemprop=\"mergify.v1.!\"></span>".into());
    assert_eq!(
        observe(pr.clone(), &[check.clone()], &[]).state,
        QueueState::Unknown
    );
    check.output = None;
    assert_eq!(observe(pr, &[check], &[]).state, QueueState::Unknown);
}

#[test]
fn machine_metadata_does_not_confuse_interruption_or_deferred_checks_with_failure() {
    let (pr, checks, _) = recorded("running");
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
        (
            json!({"version":1,"state":"dequeued","dequeue_reason":{"code":"CHECKS_INTERRUPTED"}}),
            QueueState::Dequeued,
        ),
    ] {
        check.output.as_mut().unwrap().summary = Some(format!(
            "<span itemprop=\"mergify.v1.{}\"></span>",
            STANDARD.encode(payload.to_string())
        ));
        assert_eq!(observe(pr.clone(), &[check.clone()], &[]).state, expected);
    }
}

#[test]
fn comment_identity_and_payload_are_required() {
    let (pr, _, mut comments) = recorded("conflict");
    assert_eq!(observe(pr.clone(), &[], &comments).events.len(), 1);
    for change in [0, 1, 2] {
        let mut comment = comments[0].clone();
        match change {
            0 => comment.user.id = 1,
            1 => comment.user.login = "someone".into(),
            2 => comment.body = "In merge queue. Checks failed. Merged!".into(),
            _ => unreachable!(),
        }
        assert!(observe(pr.clone(), &[], &[comment]).events.is_empty());
    }
    comments[0].body = comments[0]
        .body
        .replace("\"version\": 1", "\"version\": 99");
    assert_eq!(
        observe(pr, &[], &comments).events[0].state,
        QueueState::Unknown
    );
}

static PEM: LazyLock<String> = LazyLock::new(|| {
    RsaPrivateKey::new(&mut OsRng, 2048)
        .unwrap()
        .to_pkcs8_pem(LineEnding::LF)
        .unwrap()
        .to_string()
});

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
            pem: PEM.clone(),
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
        let prefix = "/repos/test/repo";
        get(&server, &format!("{prefix}/pulls/{number}"), pr.clone(), 2).await;
        get(
            &server,
            &format!("{prefix}/issues/{number}/comments"),
            fixture(name, "comments"),
            1,
        )
        .await;
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
        assert_eq!(
            client.read("test/repo", number).await.unwrap().state,
            expected
        );
    }
}

#[tokio::test]
async fn commands_post_once_and_reconcile_receipts_by_operation() {
    let server = MockServer::start().await;
    let client = client(&server).await;
    let pr = fixture("waiting", "pull");
    get(&server, "/repos/test/repo/pulls/2425", pr.clone(), 2).await;
    let mut receipts = vec![];
    for command in ["queue", "dequeue"] {
        let key = format!("mergify:{command}:operation-1");
        let body = format!("{}\n\n@mergifyio {command}", GitHub::marker(&key));
        let mut receipt = fixture("merged", "comments")[0].clone();
        receipt["user"] = json!({"id":1,"login":"factory[bot]"});
        receipt["body"] = json!(body);
        receipt["id"] = json!(if command == "queue" { 1 } else { 2 });
        Mock::given(method("POST"))
            .and(path("/repos/test/repo/issues/2425/comments"))
            .and(body_json(json!({"body":body})))
            .respond_with(ResponseTemplate::new(201).set_body_json(receipt.clone()))
            .expect(1)
            .mount(&server)
            .await;
        receipts.push(receipt);
    }
    let calls = AtomicUsize::new(0);
    let served = receipts.clone();
    Mock::given(method("GET"))
        .and(path("/repos/test/repo/issues/2425/comments"))
        .respond_with(move |_: &wiremock::Request| {
            let count = calls.fetch_add(1, Ordering::SeqCst);
            let body = match count {
                0 => json!([]),
                1 | 2 => json!([served[0]]),
                _ => json!(served),
            };
            ResponseTemplate::new(200).set_body_json(body)
        })
        .expect(4)
        .mount(&server)
        .await;
    let head = pr["head"]["sha"].as_str().unwrap();
    assert_eq!(
        client
            .queue("test/repo", 2425, head, "operation-1")
            .await
            .unwrap()
            .id,
        1
    );
    assert_eq!(
        client
            .queue("test/repo", 2425, head, "operation-1")
            .await
            .unwrap()
            .id,
        1
    );
    assert_eq!(
        client
            .dequeue("test/repo", 2425, "operation-1")
            .await
            .unwrap()
            .id,
        2
    );
    assert_eq!(
        client
            .dequeue("test/repo", 2425, "operation-1")
            .await
            .unwrap()
            .id,
        2
    );
}

#[tokio::test]
async fn refuses_unapproved_or_closed_heads_before_posting() {
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
        let result = client.queue("test/repo", 2425, approved, "effect").await;
        assert!(matches!(
            (closed, result),
            (false, Err(Error::HeadChanged)) | (true, Err(Error::PullRequestClosed))
        ));
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
    get(
        &server,
        "/repos/test/repo/issues/2425/comments",
        json!([]),
        1,
    )
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
        client.read("test/repo", 2425).await,
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
        client.read("test/repo", 7).await,
        Err(Error::GitHub(tl3_github::Error::Http { status: 403 }))
    ));
}
