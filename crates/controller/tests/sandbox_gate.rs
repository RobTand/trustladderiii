//! Replay sandbox REST evidence through the production queue adapter.
use std::{collections::BTreeMap, path::PathBuf, time::Duration};
use tl3_controller::{
    Repository,
    adapters::mergify::{Adapter, Evidence},
    executor::{Delivery, Executor, Observation, Reconciliation},
};
use tl3_github::{AppConfig, ClientOptions, GitHub};
use tl3_kernel::{EffectKind, NodeId, Out};
use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

fn replay(name: &str) -> Vec<bool> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../deploy/sandbox/evidence")
        .join(name);
    let load = |part: &str| -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(root.join(format!("{part}.json"))).unwrap()).unwrap()
    };
    let pull = load("pull");
    let head = pull["head"]["sha"].as_str().unwrap().to_owned();
    let merge = pull["merge_commit_sha"].as_str().unwrap();
    let number = pull["number"].as_u64().unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let server = runtime.block_on(async {
        let server = MockServer::start().await;
        for (method, path, body) in [
            (
                "POST",
                "/app/installations/3/access_tokens".to_owned(),
                serde_json::json!({"token":"disposable", "expires_at":"2099-01-01T00:00:00Z"}),
            ),
            (
                "GET",
                format!("/repos/RobTand/tl3-sandbox/pulls/{number}"),
                pull.clone(),
            ),
            (
                "GET",
                format!("/repos/RobTand/tl3-sandbox/commits/{head}/check-runs"),
                serde_json::json!({"check_runs":load("head-checks")}),
            ),
            (
                "GET",
                format!("/repos/RobTand/tl3-sandbox/commits/{merge}/check-runs"),
                serde_json::json!({"check_runs":load("merge-checks")}),
            ),
        ] {
            Mock::given(matchers::method(method))
                .and(matchers::path(path))
                .respond_with(
                    ResponseTemplate::new(if method == "POST" { 201 } else { 200 })
                        .set_body_json(body),
                )
                .mount(&server)
                .await;
        }
        server
    });
    let github = GitHub::new(
        AppConfig {
            id: 123,
            slug: "factory".into(),
            pem: include_str!("../../mergify/tests/fixtures/throwaway-key.pem").into(),
            webhook_secret: "test-secret".into(),
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
    let records = tempfile::tempdir().unwrap();
    let mut adapter = Adapter::new(BTreeMap::from([(3, github)]), 1, records.path()).unwrap();
    let delivery = Delivery {
        key: format!("sandbox-{name}"),
        epoch: 1,
        output: Out::Effect {
            node: NodeId(0),
            seq: 1,
            kind: EffectKind::Enqueue,
            head: None,
        }
        .into(),
        generation: 1,
        head_sha: Some(head),
        repository: Repository {
            owner: "RobTand".into(),
            name: "tl3-sandbox".into(),
            installation: 3,
        },
        issue: 17,
        pull_request: Some(number),
        paths: vec![],
        attempt: 1,
        round: 0,
        replanned: false,
        question: None,
        marker_key: None,
        note: None,
        question_context: None,
        checks: None,
        job_generation: None,
        steering: None,
        request_text: None,
    };
    let Reconciliation::Found(completion) = adapter.reconcile(&delivery).unwrap() else {
        panic!("recorded merged PR must produce merge evidence");
    };
    let merges: Vec<_> = completion
        .observations
        .iter()
        .filter_map(|observation| match observation {
            Observation::Queue(fact) => match &fact.evidence {
                Evidence::Merged { checks_passed, .. } => Some(*checks_passed),
                _ => None,
            },
            _ => None,
        })
        .collect();
    merges
}

#[test]
fn bootstrap_merge_has_no_queue_ci_evidence() {
    assert_eq!(replay("bootstrap"), [false]);
}

#[test]
fn approved_queue_merge_has_exact_head_and_queue_ci_evidence() {
    assert_eq!(replay("queue-trials/approved/after"), [true]);
}
