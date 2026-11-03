use super::*;
use serde_json::{Value, json};
use std::time::Duration;
use tl3_github::{AppConfig, ClientOptions};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const PARENT: &str = "9cfcf69b-2bd7-46a5-9060-4163a1448bba";
const CHILD: &str = "97d18911-0000-4000-8000-000000000000";
const TARGET: &str = "//tests:test_checkout_recovery_1465";
const KEY: &str = "synthetic-internal-material";

async fn evidence(mode: &str) -> (QueueCiEvidence, Vec<Request>) {
    let server = MockServer::start().await;
    let two_environments = mode == "two-environments";
    let mode = mode.to_owned();
    Mock::given(wiremock::matchers::any())
        .respond_with(move |request: &Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
            let answer = match request.url.path() {
                "/app/installations/3/access_tokens" => json!({
                    "token":"synthetic-github-token", "expires_at":"2099-01-01T00:00:00Z"
                }),
                "/repos/owner/project/actions/jobs/7/logs" => {
                    return ResponseTemplate::new(200).set_body_string(format!(
                        "Remote Bazel run: https://app.buildbuddy.io/invocation/{PARENT}\nFAILED: {TARGET}\n{}",
                        "cleanup\n".repeat(10000)
                    ));
                }
                "/api/v1/GetInvocation" => {
                    let id = body["selector"]["invocation_id"].as_str().unwrap();
                    let returned_id = if mode == "wrong-invocation" { "wrong" } else { id };
                    let sha = if mode == "stale-sha" { "old-head" } else { HEAD };
                    let repo = if mode == "wrong-repo" { "https://github.com/other/repo" } else { "https://github.com/owner/project.git" };
                    json!({"invocation":[{
                        "id":{"invocationId":returned_id}, "repoUrl":repo,
                        "commitSha":sha, "invocationStatus":"COMPLETE_INVOCATION_STATUS",
                        "command":if id == PARENT {"remote run"} else {"test"},
                        "childInvocations":if id == PARENT {json!([{"invocationId":CHILD}])} else {json!([])}
                    }]})
                }
                "/api/v1/GetTarget" => json!({"target":[{
                    "id":{"invocationId":CHILD,"targetId":"target-id"},
                    "label":TARGET, "status":"FAILED"
                }]}),
                "/api/v1/GetAction" => {
                    if mode == "unavailable" {
                        return ResponseTemplate::new(503).set_body_string(format!("x-buildbuddy-api-key: {KEY}"));
                    }
                    if mode == "missing-artifacts" { return ResponseTemplate::new(200).set_body_json(json!({"action":[]})); }
                    json!({"action":(0..4).map(|shard| json!({
                        "id":{"invocationId":CHILD,"targetId":"target-id","configurationId":"config", "actionId":format!("action-{shard}")},
                        "targetLabel":if mode == "wrong-target" {"//tests:other"} else {TARGET},
                        "shard":shard,"run":1,"attempt":1,
                        "file":[{"name":"test.xml","uri":format!("bytestream://remote.buildbuddy.io/blobs/xml-{shard}/123")},
                                {"name":"test.log","uri":format!("bytestream://remote.buildbuddy.io/blobs/log-{shard}/123")}]
                    })).collect::<Vec<_>>()})
                }
                "/api/v1/GetFile" => {
                    let uri = body["uri"].as_str().unwrap();
                    if uri.contains("xml") {
                        let xml: String = if mode == "malformed-xml" {"<testsuite><testcase".into()}
                        else if uri.contains("xml-0") {
                            "<testsuite tests='4' failures='1' errors='1' skipped='1'><testcase classname='tests.test_checkout_recovery_1465' name='test_lost'><error message='FileNotFoundError'>FileNotFoundError: /home/rob/pbt17/pytest-of-buildbuddy/pytest-130 vanished</error></testcase><testcase classname='tests.test_checkout_recovery_1465' name='test_assertion'><failure message='assert exists'>assert checkout.exists()</failure></testcase><testcase classname='tests.test_checkout_recovery_1465' name='test_capability'><skipped message='io_uring prerequisite unavailable'>capability prerequisite not met</skipped></testcase><testcase classname='tests.test_checkout_recovery_1465' name='test_ok'/></testsuite>".into()
                        } else {"<testsuite tests='1'><testcase classname='tests.test_checkout_recovery_1465' name='test_ok'/></testsuite>".into()};
                        return ResponseTemplate::new(200).set_body_string(xml);
                    }
                    if mode == "oversized" { return ResponseTemplate::new(200).set_body_string("x".repeat(2 * 1024 * 1024)); }
                    return ResponseTemplate::new(200).set_body_string(format!(
                        "FileNotFoundError: remote basetemp vanished\n{KEY}\nx-buildbuddy-api-key: {KEY}\nhttps://blob.example/test.log?X-Amz-Signature=private-download\n{}",
                        "cleanup\n".repeat(200)
                    ));
                }
                _ => return ResponseTemplate::new(404),
            };
            ResponseTemplate::new(200).set_body_json(answer)
        }).mount(&server).await;
    let github = GitHub::new(
        AppConfig {
            id: 123,
            slug: "factory".into(),
            pem: include_str!("../../../../../mergify/tests/fixtures/throwaway-key.pem").into(),
            webhook_secret: "synthetic".into(),
        },
        3,
        ClientOptions {
            api_url: format!("{}/", server.uri()).parse().unwrap(),
            max_retries: 0,
            mutation_interval: Duration::ZERO,
            ..ClientOptions::default()
        },
    )
    .unwrap();
    let reproduction = if two_environments {
        "PB reproduction: EPERM: io_uring denied on base and head"
    } else {
        ""
    };
    let checks: Vec<CheckRun> = serde_json::from_value(json!([{
        "id":7,"name":"Bazel","head_sha":HEAD,"status":"completed","conclusion":"failure",
        "app":{"id":15368,"slug":"github-actions"},
        "details_url":"https://github.com/owner/project/actions/runs/6/job/7",
        "output":{"summary":format!("FAILED: {TARGET}\n{KEY}\n{reproduction}\nhttps://blob.example/log?X-Amz-Signature=private-download")}
    }])).unwrap();
    let client = remote::Client {
        api: format!("{}/api/v1/", server.uri()),
        key: KEY.into(),
    };
    let details = read_checks_with(&github, "owner/project", HEAD, &checks, &client).await;
    (details, server.received_requests().await.unwrap())
}

#[tokio::test]
async fn resolves_target_logs_bound_to_head_parent_check_and_all_shards() {
    let (details, _) = evidence("observed").await;
    assert!(
        details.complete,
        "target evidence must replace the truncated job summary"
    );
    let value = json!(details);
    let results = value["target_results"]
        .as_array()
        .expect("retain target results");
    assert_eq!(results.len(), 4);
    for (shard, result) in results.iter().enumerate() {
        assert_eq!(result["repository"], "owner/project");
        assert_eq!(result["check"], 7);
        assert_eq!(result["sha"], HEAD);
        assert_eq!(result["parent_invocation"], PARENT);
        assert_eq!(result["invocation"], CHILD);
        assert_eq!(result["target"], TARGET);
        assert_eq!(result["shard"], shard);
        assert_eq!(result["attempt"], 1);
    }
    assert!(value["failed_tests"].to_string().contains("test_lost"));
    assert!(
        value["failed_tests"]
            .to_string()
            .contains("FileNotFoundError")
    );
}

#[tokio::test]
async fn trusted_access_never_exports_credentials_headers_or_signed_urls() {
    let (details, requests) = evidence("observed").await;
    let text = json!(details).to_string();
    for private in [
        KEY,
        "x-buildbuddy-api-key",
        "private-download",
        "https://blob.example",
    ] {
        assert!(
            !text.contains(private),
            "private transport data escaped: {private}"
        );
    }
    assert!(
        requests
            .iter()
            .any(|request| request.url.path() == "/api/v1/GetFile"
                && request.headers.get("x-buildbuddy-api-key").is_some())
    );
}

#[tokio::test]
async fn preserves_remote_node_outcomes_and_prerequisites_separately_from_local_eperm() {
    let (details, _) = evidence("two-environments").await;
    let value = json!(details);
    let nodes = value["target_results"][0]["nodes"]
        .as_array()
        .expect("retain all remote nodes");
    let statuses: BTreeSet<_> = nodes
        .iter()
        .map(|node| node["status"].as_str().unwrap())
        .collect();
    assert_eq!(
        statuses,
        BTreeSet::from(["failed", "errored", "passed", "skipped"])
    );
    assert!(nodes.iter().any(|node| node["status"] == "skipped"
        && node["message"].as_str().unwrap().contains("prerequisite")));
    assert!(
        !value["failed_tests"]
            .to_string()
            .contains("test_capability")
    );
    let local_reproduction = "EPERM: io_uring denied on base and head";
    assert!(!json!(details).to_string().contains(local_reproduction));
    assert!(!value["failed_tests"].to_string().contains("EPERM"));
    assert!(json!(details).to_string().contains("pytest-130"));
}

#[tokio::test]
async fn exhausted_or_unbound_retrieval_is_indeterminate_with_precise_gaps() {
    for mode in [
        "wrong-invocation",
        "stale-sha",
        "wrong-repo",
        "wrong-target",
        "missing-artifacts",
        "unavailable",
        "malformed-xml",
        "oversized",
    ] {
        let (details, requests) = evidence(mode).await;
        assert!(!details.complete, "{mode}");
        let value = json!(details);
        assert!(
            !value["gaps"]
                .as_array()
                .expect("typed unresolved reason")
                .is_empty(),
            "{mode}"
        );
        assert!(requests.len() <= 20, "retrieval must be bounded: {mode}");
        assert_eq!(value["checks"][0]["name"], "Bazel");
    }
}

#[tokio::test]
async fn complete_target_evidence_retains_repair_diagnostics_without_claiming_causality() {
    let (details, _) = evidence("observed").await;
    assert!(
        details.complete,
        "complete failure keeps ordinary repair available"
    );
    assert_eq!(
        details.failed_tests.len(),
        2,
        "passed and skipped nodes are not maker failures"
    );
    assert!(
        details
            .failed_tests
            .iter()
            .any(|node| node.message.contains("assert checkout.exists()"))
    );
    assert!(
        details.decision.is_none(),
        "the evidence adapter does not decide scope or queue operations"
    );
    let maker: Value = serde_json::from_str(include_str!(
        "../../../../../../workers/src/tl3_workers/schemas/maker-input.json"
    ))
    .unwrap();
    let schema = json!({"$defs":maker["$defs"],"type":"object","properties":{"feedback":maker["properties"]["feedback"]}});
    let input = json!({"feedback":[{"kind":"failed_check","report":"Remote CI failed","goal_ids":[],"ci_target_results":details.target_results}]});
    assert!(
        jsonschema::validator_for(&schema).unwrap().is_valid(&input),
        "complete evidence must reach a valid maker input"
    );
}

#[tokio::test]
async fn diagnostic_reads_are_bounded_recorded_and_never_submit_or_change_ci() {
    let (details, requests) = evidence("unavailable").await;
    let value = json!(details);
    assert!(
        !value["retrievals"]
            .as_array()
            .expect("record retrieval attempts")
            .is_empty()
    );
    assert!(value["gaps"].to_string().contains("unavailable"));
    assert!(requests.len() <= 20);
    for request in requests
        .iter()
        .filter(|request| request.url.path().starts_with("/api/v1/"))
    {
        assert!(
            ["GetInvocation", "GetTarget", "GetAction", "GetFile"]
                .iter()
                .any(|method| request.url.path().ends_with(method))
        );
    }
    assert!(
        value.to_string().len() <= 32000,
        "model-facing evidence stays bounded"
    );
}
