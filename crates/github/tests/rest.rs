use chrono::{Duration as ChronoDuration, Utc};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};
use rand::rngs::OsRng;
use rsa::{
    RsaPrivateKey,
    pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, LazyLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tl3_github::*;
use wiremock::{
    Mock, MockServer, Request, Respond, ResponseTemplate,
    matchers::{body_json, header, method, path, query_param},
};

static KEYS: LazyLock<(String, String)> = LazyLock::new(|| {
    let key = RsaPrivateKey::new(&mut OsRng, 2048).unwrap();
    (
        key.to_pkcs8_pem(LineEnding::LF).unwrap().to_string(),
        key.to_public_key()
            .to_public_key_pem(LineEnding::LF)
            .unwrap(),
    )
});

fn config() -> AppConfig {
    AppConfig {
        id: 123,
        slug: "factory".into(),
        pem: KEYS.0.clone(),
        webhook_secret: "unused-fake".into(),
    }
}
fn options(server: &MockServer) -> ClientOptions {
    ClientOptions {
        api_url: format!("{}/", server.uri()).parse().unwrap(),
        mutation_interval: Duration::ZERO,
        max_retries: 1,
        max_wait: Duration::from_secs(2),
        ..ClientOptions::default()
    }
}
async fn client(server: &MockServer) -> GitHub {
    client_with_options(server, options(server)).await
}
async fn client_with_options(server: &MockServer, opts: ClientOptions) -> GitHub {
    Mock::given(method("POST")).and(path("/app/installations/3/access_tokens"))
        .and(header("x-github-api-version",API_VERSION))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"token":"fake-token","expires_at":(Utc::now()+ChronoDuration::hours(1)).to_rfc3339()})))
        .expect(1).mount(server).await;
    GitHub::new(config(), 3, opts).unwrap()
}
fn fixture(name: &str) -> Value {
    let bytes: &[u8] = match name {
        "issue" => include_bytes!("fixtures/issues-labeled.json"),
        "comment" => include_bytes!("fixtures/issue-comment-created.json"),
        "pull_request" => include_bytes!("fixtures/pull-request-closed.json"),
        "check_run" => include_bytes!("fixtures/check-run-completed.json"),
        _ => panic!("unknown fixture"),
    };
    let v: Value = serde_json::from_slice(bytes).unwrap();
    v[name].clone()
}
fn bot_comment(id: u64, key: &str) -> Value {
    let mut v = fixture("comment");
    v["id"] = json!(id);
    v["user"] = json!({"id":99,"login":"factory[bot]"});
    v["body"] = json!(format!("{}\n\nDone", GitHub::marker(key)));
    v
}
async fn get(server: &MockServer, endpoint: &str, response: Value) {
    Mock::given(method("GET"))
        .and(path(endpoint))
        .and(header("authorization", "Bearer fake-token"))
        .and(header("x-github-api-version", API_VERSION))
        .respond_with(ResponseTemplate::new(200).set_body_json(response))
        .expect(1)
        .mount(server)
        .await;
}

#[tokio::test]
async fn signs_rs256_jwt_and_caches_the_installation_token_across_clones() {
    let server = MockServer::start().await;
    let github = client(&server).await;
    Mock::given(method("GET"))
        .and(path("/repos/owner/sandbox/issues/12"))
        .and(header("authorization", "Bearer fake-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("issue")))
        .expect(2)
        .mount(&server)
        .await;
    let clone = github.clone();
    let (a, b) = tokio::join!(
        github.issue("owner/sandbox", 12),
        clone.issue("owner/sandbox", 12)
    );
    assert_eq!(a.unwrap().number, 12);
    b.unwrap();
    let requests = server.received_requests().await.unwrap();
    let auth = requests
        .iter()
        .find(|r| r.url.path().contains("access_tokens"))
        .unwrap();
    let bearer = auth
        .headers
        .get("authorization")
        .unwrap()
        .to_str()
        .unwrap()
        .strip_prefix("Bearer ")
        .unwrap();
    #[derive(Debug, Deserialize)]
    struct Claims {
        iss: String,
        iat: i64,
        exp: i64,
    }
    let data = decode::<Claims>(
        bearer,
        &DecodingKey::from_rsa_pem(KEYS.1.as_bytes()).unwrap(),
        &Validation::new(Algorithm::RS256),
    )
    .unwrap();
    assert_eq!(data.claims.iss, "123");
    assert!((Utc::now().timestamp() - data.claims.iat - 60).abs() < 10);
    assert_eq!(data.claims.exp - data.claims.iat, 600);
}

#[tokio::test]
async fn refreshes_a_token_near_expiry() {
    let server = MockServer::start().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    Mock::given(method("POST")).and(path("/app/installations/3/access_tokens")).respond_with(move |_: &Request| {
        let n=seen.fetch_add(1,Ordering::SeqCst);
        ResponseTemplate::new(201).set_body_json(json!({"token":format!("token-{n}"),"expires_at":(Utc::now()+ChronoDuration::seconds(if n==0 {30} else {3600})).to_rfc3339()}))
    }).expect(2).mount(&server).await;
    Mock::given(method("GET"))
        .and(path("/repos/owner/sandbox/issues/12"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("issue")))
        .expect(2)
        .mount(&server)
        .await;
    let github = GitHub::new(config(), 3, options(&server)).unwrap();
    github.issue("owner/sandbox", 12).await.unwrap();
    github.issue("owner/sandbox", 12).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn comments_keep_markers_and_repeated_posts_return_the_original() {
    let server = MockServer::start().await;
    let github = client(&server).await;
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    Mock::given(method("GET"))
        .and(path("/repos/owner/sandbox/issues/12/comments"))
        .respond_with(move |_: &Request| {
            let found = seen.fetch_add(1, Ordering::SeqCst) > 0;
            ResponseTemplate::new(200).set_body_json(if found {
                json!([bot_comment(9, "key")])
            } else {
                json!([])
            })
        })
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/repos/owner/sandbox/issues/12/comments"))
        .and(body_json(
            json!({"body":format!("{}\n\nDone",GitHub::marker("key"))}),
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(bot_comment(9, "key")))
        .expect(1)
        .mount(&server)
        .await;
    assert_eq!(
        github
            .post_comment("owner/sandbox", 12, "key", "Done")
            .await
            .unwrap()
            .id,
        9
    );
    assert_eq!(
        github
            .post_comment("owner/sandbox", 12, "key", "Done")
            .await
            .unwrap()
            .id,
        9
    );
    Mock::given(method("PATCH"))
        .and(path("/repos/owner/sandbox/issues/comments/9"))
        .and(body_json(
            json!({"body":format!("{}\n\nUpdated",GitHub::marker("key"))}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(bot_comment(9, "key")))
        .expect(1)
        .mount(&server)
        .await;
    github
        .edit_comment("owner/sandbox", 9, "key", "Updated")
        .await
        .unwrap();
    assert!(matches!(
        github.post_comment("owner/sandbox", 12, "", "bad").await,
        Err(Error::Config(_))
    ));
}

#[tokio::test]
async fn reconciliation_deletes_only_duplicate_app_comments_and_checks_all_pages() {
    let server = MockServer::start().await;
    let github = client(&server).await;
    let mut forged = bot_comment(1, "key");
    forged["user"]["login"] = json!("someone");
    let first: Vec<_> = (0..100).map(|_| forged.clone()).collect();
    Mock::given(method("GET"))
        .and(path("/repos/owner/sandbox/issues/12/comments"))
        .and(query_param("page", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(first))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/owner/sandbox/issues/12/comments"))
        .and(query_param("page", "2"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!([bot_comment(9, "key"), bot_comment(10, "key")])),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/repos/owner/sandbox/issues/comments/10"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    assert_eq!(
        github
            .reconcile_comments("owner/sandbox", 12, "key")
            .await
            .unwrap()
            .unwrap()
            .id,
        9
    );
}

struct OnceThen {
    calls: AtomicUsize,
    first: ResponseTemplate,
    then: ResponseTemplate,
}
impl Respond for OnceThen {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            self.first.clone()
        } else {
            self.then.clone()
        }
    }
}

#[tokio::test]
async fn retries_transient_reads_but_not_permissions_or_validation_errors() {
    for (status, retries) in [(503, true), (403, false), (422, false)] {
        let server = MockServer::start().await;
        let github = client(&server).await;
        Mock::given(method("GET"))
            .and(path("/repos/owner/sandbox/issues/12"))
            .respond_with(OnceThen {
                calls: AtomicUsize::new(0),
                first: ResponseTemplate::new(status),
                then: ResponseTemplate::new(200).set_body_json(fixture("issue")),
            })
            .expect(if retries { 2 } else { 1 })
            .mount(&server)
            .await;
        let result = github.issue("owner/sandbox", 12).await;
        if retries {
            result.unwrap();
        } else {
            assert!(matches!(result,Err(Error::Http {status:s}) if s==status));
        }
    }
}

#[tokio::test]
async fn observes_retry_after_reset_and_secondary_rate_limits() {
    for (status, headers, body) in [
        (429, vec![("retry-after", "0")], json!({})),
        (
            403,
            vec![("x-ratelimit-remaining", "0"), ("x-ratelimit-reset", "1")],
            json!({}),
        ),
    ] {
        let server = MockServer::start().await;
        let github = client(&server).await;
        let mut first = ResponseTemplate::new(status).set_body_json(body);
        for (key, value) in headers {
            first = first.insert_header(key, value);
        }
        Mock::given(method("GET"))
            .and(path("/repos/owner/sandbox/issues/12"))
            .respond_with(OnceThen {
                calls: AtomicUsize::new(0),
                first,
                then: ResponseTemplate::new(200).set_body_json(fixture("issue")),
            })
            .expect(2)
            .mount(&server)
            .await;
        github.issue("owner/sandbox", 12).await.unwrap();
    }
    let server = MockServer::start().await;
    let github = client(&server).await;
    Mock::given(method("GET"))
        .and(path("/repos/owner/sandbox/issues/12"))
        .respond_with(
            ResponseTemplate::new(403)
                .set_body_json(json!({"message":"You have exceeded a secondary rate limit."})),
        )
        .expect(1)
        .mount(&server)
        .await;
    assert!(
        matches!(github.issue("owner/sandbox",12).await,Err(Error::RateLimited {retry_after}) if retry_after==Duration::from_secs(60))
    );
    assert!(matches!(
        github.issue("owner/sandbox", 12).await,
        Err(Error::RateLimited { .. })
    ));
}

#[tokio::test]
async fn uncertain_comment_write_is_not_retried_until_reconciliation() {
    let server = MockServer::start().await;
    let github = client(&server).await;
    let seen = Arc::new(AtomicUsize::new(0));
    let calls = seen.clone();
    Mock::given(method("GET"))
        .and(path("/repos/owner/sandbox/issues/12/comments"))
        .respond_with(move |_: &Request| {
            ResponseTemplate::new(200).set_body_json(if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                json!([])
            } else {
                json!([bot_comment(9, "key")])
            })
        })
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/repos/owner/sandbox/issues/12/comments"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&server)
        .await;
    assert!(matches!(
        github
            .post_comment("owner/sandbox", 12, "key", "Done")
            .await,
        Err(Error::Http { status: 503 })
    ));
    assert_eq!(
        github
            .post_comment("owner/sandbox", 12, "key", "Done")
            .await
            .unwrap()
            .id,
        9
    );
}

#[tokio::test]
async fn all_slice_one_rest_operations_use_the_documented_routes() {
    let server = MockServer::start().await;
    let github = client(&server).await;
    get(&server, "/repos/owner/sandbox/issues/12", fixture("issue")).await;
    github.issue("owner/sandbox", 12).await.unwrap();
    Mock::given(method("PATCH"))
        .and(path("/repos/owner/sandbox/issues/12"))
        .and(body_json(
            json!({"state":"closed","state_reason":"not_planned"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("issue")))
        .expect(1)
        .mount(&server)
        .await;
    github
        .close_issue("owner/sandbox", 12, CloseReason::NotPlanned)
        .await
        .unwrap();
    get(
        &server,
        "/repos/owner/sandbox/commits/abc1234/check-runs",
        json!({"total_count":0,"check_runs":[]}),
    )
    .await;
    Mock::given(method("POST")).and(path("/repos/owner/sandbox/check-runs"))
        .and(body_json(json!({"name":"approval","head_sha":"abc1234","external_id":"key","status":"completed","conclusion":"success"})))
        .respond_with(ResponseTemplate::new(201).set_body_json(fixture("check_run"))).expect(1).mount(&server).await;
    github
        .create_check_run(
            "owner/sandbox",
            &NewCheckRun {
                name: "approval".into(),
                head_sha: "abc1234".into(),
                external_id: "key".into(),
                status: CheckStatus::Completed,
                conclusion: Some(CheckConclusion::Success),
            },
        )
        .await
        .unwrap();
    get(
        &server,
        "/repos/owner/sandbox/pulls/13",
        fixture("pull_request"),
    )
    .await;
    github.pull_request("owner/sandbox", 13).await.unwrap();
    Mock::given(method("GET"))
        .and(path("/repos/owner/sandbox/pulls"))
        .and(query_param("state", "all"))
        .and(query_param("head", "owner:tl3/change"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([fixture("pull_request")])))
        .expect(1)
        .mount(&server)
        .await;
    assert_eq!(
        github
            .pull_requests_for_branch("owner/sandbox", "owner", "tl3/change")
            .await
            .unwrap()
            .len(),
        1
    );
    Mock::given(method("POST"))
        .and(path("/repos/owner/sandbox/pulls"))
        .and(body_json(
            json!({"title":"A change","head":"tl3/change","base":"main","body":"Done"}),
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(fixture("pull_request")))
        .expect(1)
        .mount(&server)
        .await;
    github
        .open_pull_request(
            "owner/sandbox",
            &NewPullRequest {
                title: "A change".into(),
                head: "tl3/change".into(),
                base: "main".into(),
                body: "Done".into(),
            },
        )
        .await
        .unwrap();
    Mock::given(method("PATCH"))
        .and(path("/repos/owner/sandbox/pulls/13"))
        .and(body_json(json!({"state":"closed"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("pull_request")))
        .expect(1)
        .mount(&server)
        .await;
    github
        .close_pull_request("owner/sandbox", 13)
        .await
        .unwrap();
    // GitHub branch names occupy one encoded path segment.
    get(
        &server,
        "/repos/owner/sandbox/branches/tl3%2Fchange",
        json!({"name":"tl3/change","commit":{"sha":"abc1234"}}),
    )
    .await;
    assert_eq!(
        github
            .branch("owner/sandbox", "tl3/change")
            .await
            .unwrap()
            .unwrap()
            .commit
            .sha,
        "abc1234"
    );
    Mock::given(method("DELETE"))
        .and(path("/repos/owner/sandbox/git/refs/heads/tl3%2Fchange"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    github
        .delete_branch("owner/sandbox", "tl3/change")
        .await
        .unwrap();
}

#[tokio::test]
async fn missing_branches_converge_but_permission_errors_do_not() {
    for status in [404, 422, 403] {
        let server = MockServer::start().await;
        let github = client(&server).await;
        Mock::given(method("DELETE"))
            .and(path("/repos/owner/sandbox/git/refs/heads/gone"))
            .respond_with(ResponseTemplate::new(status))
            .expect(1)
            .mount(&server)
            .await;
        if status == 422 {
            Mock::given(method("GET"))
                .and(path("/repos/owner/sandbox/branches/gone"))
                .respond_with(ResponseTemplate::new(404))
                .expect(1)
                .mount(&server)
                .await;
        }
        let result = github.delete_branch("owner/sandbox", "gone").await;
        assert_eq!(result.is_ok(), status != 403);
    }
}

#[tokio::test]
async fn check_external_id_reconciles_only_the_apps_own_check() {
    let server = MockServer::start().await;
    let github = client(&server).await;
    let mut foreign = fixture("check_run");
    foreign["app"]["id"] = json!(999);
    get(
        &server,
        "/repos/owner/sandbox/commits/abc1234/check-runs",
        json!({"check_runs":[{
            "id": 21, "name": "approval", "head_sha": "abc1234",
            "external_id": "approval-key", "status": "completed", "conclusion": "success", "app": null
        }, foreign,fixture("check_run")]}),
    )
    .await;
    assert_eq!(
        github
            .create_check_run(
                "owner/sandbox",
                &NewCheckRun {
                    name: "approval".into(),
                    head_sha: "abc1234".into(),
                    external_id: "approval-key".into(),
                    status: CheckStatus::Completed,
                    conclusion: Some(CheckConclusion::Success),
                }
            )
            .await
            .unwrap()
            .id,
        22
    );
}

#[tokio::test]
async fn catch_up_keeps_label_actors_and_rereads_checks_for_unchanged_heads() {
    let server = MockServer::start().await;
    let github = client(&server).await;
    let since = "2026-10-29T00:00:00Z".parse().unwrap();
    Mock::given(method("GET"))
        .and(path("/repos/owner/sandbox/issues"))
        .and(query_param("state", "all"))
        .and(query_param("since", "2026-10-28T23:59:59+00:00"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([fixture("issue")])))
        .expect(1)
        .mount(&server)
        .await;
    get(
        &server,
        "/repos/owner/sandbox/issues/comments",
        json!([fixture("comment")]),
    )
    .await;
    get(&server,"/repos/owner/sandbox/issues/events",json!([
        {"id":1,"event":"labeled","actor":{"id":42,"login":"owner"},"label":{"name":"tl3"},"created_at":"2026-10-30T01:00:00Z","issue":{"number":12}},
        {"id":2,"event":"closed","created_at":"2026-10-30T01:00:00Z"}
    ])).await;
    get(
        &server,
        "/repos/owner/sandbox/commits/abc1234/check-runs",
        json!({"check_runs":[fixture("check_run")]}),
    )
    .await;
    get(&server,"/repos/owner/sandbox/commits/abc1234/check-suites",json!({"check_suites":[{"id":23,"head_sha":"abc1234","status":"completed","conclusion":"success"}]})).await;
    get(
        &server,
        "/repos/owner/sandbox/pulls",
        json!([fixture("pull_request")]),
    )
    .await;
    let changes = github
        .catch_up(
            &[ServedRepo {
                full_name: "owner/sandbox".into(),
                tracked_heads: vec!["abc1234".into()],
            }],
            since,
        )
        .await
        .unwrap();
    assert!(changes.next_cursor > since);
    let changes = &changes.repositories[0];
    assert_eq!(changes.labels[0].1.actor.id, 42);
    assert_eq!(changes.comments.len(), 1);
    assert_eq!(changes.pull_requests[0].number, 13);
    assert_eq!(changes.check_runs[0].id, 22);
    assert_eq!(changes.check_suites[0].id, 23);
}

#[tokio::test]
async fn rejects_unsafe_urls_and_does_not_follow_cross_origin_redirects() {
    let server = MockServer::start().await;
    let mut opts = options(&server);
    opts.api_url = "http://example.com/".parse().unwrap();
    assert!(matches!(
        GitHub::new(config(), 3, opts),
        Err(Error::Config(_))
    ));
    let github = client(&server).await;
    Mock::given(method("GET"))
        .and(path("/repos/owner/sandbox/issues/12"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("location", "https://api.github.com/"),
        )
        .expect(1)
        .mount(&server)
        .await;
    assert!(matches!(
        github.issue("owner/sandbox", 12).await,
        Err(Error::Http { status: 302 })
    ));
    assert!(matches!(
        github.issue("../sandbox", 12).await,
        Err(Error::Config(_))
    ));
}

#[tokio::test]
async fn timed_out_post_reconciles_a_committed_comment_before_another_write() {
    let server = MockServer::start().await;
    let mut opts = options(&server);
    opts.request_timeout = Duration::from_millis(500);
    let github = client_with_options(&server, opts).await;
    let posted = Arc::new(AtomicUsize::new(0));
    let committed = posted.clone();
    Mock::given(method("GET"))
        .and(path("/repos/owner/sandbox/issues/12/comments"))
        .respond_with(move |_: &Request| {
            ResponseTemplate::new(200).set_body_json(if committed.load(Ordering::SeqCst) == 0 {
                json!([])
            } else {
                json!([bot_comment(9, "key")])
            })
        })
        .expect(2)
        .mount(&server)
        .await;
    let committed = posted.clone();
    Mock::given(method("POST"))
        .and(path("/repos/owner/sandbox/issues/12/comments"))
        .respond_with(move |_: &Request| {
            committed.fetch_add(1, Ordering::SeqCst);
            ResponseTemplate::new(201)
                .set_body_json(bot_comment(9, "key"))
                .set_delay(Duration::from_secs(2))
        })
        .expect(1)
        .mount(&server)
        .await;
    assert!(matches!(
        github
            .post_comment("owner/sandbox", 12, "key", "Done")
            .await,
        Err(Error::Transport)
    ));
    assert_eq!(posted.load(Ordering::SeqCst), 1);
    assert_eq!(
        github
            .post_comment("owner/sandbox", 12, "key", "Done")
            .await
            .unwrap()
            .id,
        9
    );
    assert_eq!(posted.load(Ordering::SeqCst), 1);
    let requests = server.received_requests().await.unwrap();
    let methods: Vec<_> = requests.iter().map(|r| r.method.as_str()).collect();
    assert_eq!(methods, ["POST", "GET", "POST", "GET"]);
}

#[tokio::test]
async fn retries_patch_and_delete_without_changing_the_mutation() {
    let server = MockServer::start().await;
    let github = client(&server).await;
    Mock::given(method("PATCH"))
        .and(path("/repos/owner/sandbox/issues/comments/9"))
        .and(body_json(
            json!({"body":format!("{}\n\nUpdated", GitHub::marker("key"))}),
        ))
        .respond_with(OnceThen {
            calls: AtomicUsize::new(0),
            first: ResponseTemplate::new(503),
            then: ResponseTemplate::new(200).set_body_json(bot_comment(9, "key")),
        })
        .expect(2)
        .mount(&server)
        .await;
    assert_eq!(
        github
            .edit_comment("owner/sandbox", 9, "key", "Updated")
            .await
            .unwrap()
            .id,
        9
    );
    Mock::given(method("DELETE"))
        .and(path("/repos/owner/sandbox/issues/comments/9"))
        .respond_with(OnceThen {
            calls: AtomicUsize::new(0),
            first: ResponseTemplate::new(502),
            then: ResponseTemplate::new(404),
        })
        .expect(2)
        .mount(&server)
        .await;
    github.delete_comment("owner/sandbox", 9).await.unwrap();
}

#[tokio::test]
async fn unauthorized_token_is_invalidated_for_the_next_call_across_clones() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/app/installations/3/access_tokens"))
        .respond_with(OnceThen {
            calls: AtomicUsize::new(0),
            first: ResponseTemplate::new(201).set_body_json(json!({
                "token":"expired-token", "expires_at":(Utc::now()+ChronoDuration::hours(1)).to_rfc3339()
            })),
            then: ResponseTemplate::new(201).set_body_json(json!({
                "token":"fresh-token", "expires_at":(Utc::now()+ChronoDuration::hours(1)).to_rfc3339()
            })),
        })
        .expect(2).mount(&server).await;
    Mock::given(method("GET"))
        .and(path("/repos/owner/sandbox/issues/12"))
        .and(header("authorization", "Bearer expired-token"))
        .respond_with(ResponseTemplate::new(401))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/owner/sandbox/issues/12"))
        .and(header("authorization", "Bearer fresh-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("issue")))
        .expect(2)
        .mount(&server)
        .await;
    let github = GitHub::new(config(), 3, options(&server)).unwrap();
    assert!(matches!(
        github.issue("owner/sandbox", 12).await,
        Err(Error::Http { status: 401 })
    ));
    github.clone().issue("owner/sandbox", 12).await.unwrap();
    github.issue("owner/sandbox", 12).await.unwrap();
}

#[tokio::test]
async fn rate_limited_post_waits_and_reconciles_before_trying_again() {
    for status in [429, 403] {
        let server = MockServer::start().await;
        let github = client(&server).await;
        Mock::given(method("GET"))
            .and(path("/repos/owner/sandbox/issues/12/comments"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .expect(2)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/repos/owner/sandbox/issues/12/comments"))
            .respond_with(OnceThen {
                calls: AtomicUsize::new(0),
                first: ResponseTemplate::new(status).insert_header("retry-after", "1"),
                then: ResponseTemplate::new(201).set_body_json(bot_comment(9, "key")),
            })
            .expect(2)
            .mount(&server)
            .await;
        assert!(matches!(
            github.post_comment("owner/sandbox", 12, "key", "Done").await,
            Err(Error::RateLimited { retry_after }) if retry_after == Duration::from_secs(1)
        ));
        let retry_started = std::time::Instant::now();
        github
            .clone()
            .post_comment("owner/sandbox", 12, "key", "Done")
            .await
            .unwrap();
        assert!(retry_started.elapsed() >= Duration::from_millis(900));
        let requests = server.received_requests().await.unwrap();
        let methods: Vec<_> = requests.iter().map(|r| r.method.as_str()).collect();
        assert_eq!(methods, ["POST", "GET", "POST", "GET", "POST"]);
    }
}

#[tokio::test]
async fn catch_up_stops_pr_pages_at_the_cursor_and_reads_repository_label_events() {
    for recent_count in [0, 101] {
        let server = MockServer::start().await;
        let github = client(&server).await;
        let since = "2026-10-29T00:00:00Z".parse().unwrap();
        get(
            &server,
            "/repos/owner/sandbox/issues",
            json!([fixture("issue"), fixture("issue")]),
        )
        .await;
        get(&server, "/repos/owner/sandbox/issues/comments", json!([])).await;
        let mut recent = fixture("pull_request");
        recent["updated_at"] = json!("2026-10-29T00:00:00Z");
        let mut old = fixture("pull_request");
        old["updated_at"] = json!("2026-10-28T23:59:59Z");
        let rows: Vec<_> = std::iter::repeat_n(recent, recent_count)
            .chain(std::iter::repeat_n(old, 100))
            .collect();
        let page_count = recent_count / 100 + 1;
        for (page, chunk) in rows.chunks(100).take(page_count).enumerate() {
            Mock::given(method("GET"))
                .and(path("/repos/owner/sandbox/pulls"))
                .and(query_param("sort", "updated"))
                .and(query_param("direction", "desc"))
                .and(query_param("state", "all"))
                .and(query_param("per_page", "100"))
                .and(query_param("page", (page + 1).to_string()))
                .respond_with(ResponseTemplate::new(200).set_body_json(chunk))
                .expect(1)
                .mount(&server)
                .await;
        }
        Mock::given(method("GET"))
            .and(path("/repos/owner/sandbox/pulls"))
            .and(query_param("page", (page_count + 1).to_string()))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&server)
            .await;
        let old_event = json!({"id":1,"event":"labeled","actor":{"id":99,"login":"other"},
            "label":{"name":"tl3"},"created_at":"2026-10-28T23:59:59Z","issue":{"number":12}});
        Mock::given(method("GET"))
            .and(path("/repos/owner/sandbox/issues/events"))
            .and(query_param("page", "1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(vec![old_event; 100]))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/owner/sandbox/issues/events"))
            .and(query_param("page", "2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"id":2,"event":"labeled","actor":{"id":42,"login":"owner"},
                    "label":{"name":"tl3"},"created_at":"2026-10-29T00:00:00Z","issue":{"number":14}},
                {"id":3,"event":"unlabeled","actor":{"id":99,"login":"other"},
                    "label":{"name":"tl3"},"created_at":"2026-10-30T00:00:00Z","issue":{"number":15}},
                {"id":4,"event":"closed"}
            ])))
            .expect(1).mount(&server).await;
        get(
            &server,
            "/repos/owner/sandbox/commits/abc1234/check-runs",
            json!({"check_runs":[{
                "id":22,"name":"approval","head_sha":"abc1234","external_id":null,
                "status":"queued","conclusion":null,"app":null
            }]}),
        )
        .await;
        get(
            &server,
            "/repos/owner/sandbox/commits/abc1234/check-suites",
            json!({"check_suites":[]}),
        )
        .await;
        let result = github
            .catch_up(
                &[ServedRepo {
                    full_name: "owner/sandbox".into(),
                    tracked_heads: vec!["abc1234".into()],
                }],
                since,
            )
            .await
            .unwrap();
        let changes = &result.repositories[0];
        assert_eq!(changes.pull_requests.len(), recent_count);
        assert_eq!(changes.check_runs.len(), 1);
        assert!(changes.check_runs[0].app.is_none());
        assert_eq!(changes.labels.len(), 2);
        assert_eq!(changes.labels[0].0, 14);
        assert_eq!(changes.labels[0].1.actor.id, 42);
        assert_eq!(changes.labels[1].0, 15);
        assert_eq!(changes.labels[1].1.event, "unlabeled");
        assert_eq!(changes.labels[1].1.actor.id, 99);
    }
}

#[tokio::test]
async fn unfinished_check_requests_omit_the_conclusion_field() {
    for (status, status_text) in [
        (CheckStatus::Queued, "queued"),
        (CheckStatus::InProgress, "in_progress"),
    ] {
        let server = MockServer::start().await;
        let github = client(&server).await;
        get(
            &server,
            "/repos/owner/sandbox/commits/abc1234/check-runs",
            json!({"check_runs":[]}),
        )
        .await;
        Mock::given(method("POST"))
            .and(path("/repos/owner/sandbox/check-runs"))
            .and(body_json(json!({
                "name":"approval", "head_sha":"abc1234", "external_id":"approval-key", "status":status_text
            })))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "id":22, "name":"approval", "head_sha":"abc1234", "external_id":"approval-key",
                "status":status_text, "conclusion":null, "app":null
            })))
            .expect(1).mount(&server).await;
        let check = github
            .create_check_run(
                "owner/sandbox",
                &NewCheckRun {
                    name: "approval".into(),
                    head_sha: "abc1234".into(),
                    external_id: "approval-key".into(),
                    status,
                    conclusion: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(check.status, status_text);
        assert!(check.app.is_none());
    }
}
