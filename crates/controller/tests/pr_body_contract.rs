use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tl3_controller::{
    adapters::{github::Adapter, workers::writer::FakeWriter},
    executor::{Delivery, Executor, Reconciliation},
};
use tl3_github::{AppConfig, ClientOptions, GitHub, message::Policy};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const REQUEST: &str = "## PR rules\nUse Change type: test for test-only changes. Use Change type: fix only if a production seam changes.\n";

#[derive(Clone, Default)]
struct Api(Arc<Mutex<(Option<Value>, usize, usize)>>);
impl Respond for Api {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let mut state = self.0.lock().unwrap();
        let path = request.url.path();
        match (request.method.as_str(), path) {
            ("POST", "/app/installations/1/access_tokens") => ResponseTemplate::new(201).set_body_json(json!({"token":"fixture", "expires_at":"2099-01-01T00:00:00Z"})),
            ("GET", "/repos/fixture/project/issues/2640") => ResponseTemplate::new(200).set_body_json(json!({"number":2640,"title":"Repair the test", "body":REQUEST,"state":"open","updated_at":"2026-11-03T00:00:00Z"})),
            ("GET", "/repos/fixture/project/pulls") => ResponseTemplate::new(200).set_body_json(json!(state.0.iter().collect::<Vec<_>>())),
            ("POST", "/repos/fixture/project/pulls") => {
                state.1 += 1;
                let body: Value = serde_json::from_slice(&request.body).unwrap();
                let pull = json!({"number":100,"user":{"id":99,"login":"factory[bot]"},"body":body["body"],"state":"open","draft":false,"merged_at":null,
                    "head":{"ref":body["head"],"sha":"b".repeat(40),"repo":{"id":1,"name":"project","owner":{"id":1,"login":"fixture"}}},
                    "base":{"ref":"main","sha":"a".repeat(40),"repo":null},"updated_at":"2026-11-03T00:00:00Z"});
                state.0 = Some(pull.clone());
                ResponseTemplate::new(201).set_body_json(pull)
            }
            ("PATCH", "/repos/fixture/project/pulls/100") => {
                state.2 += 1;
                let body: Value = serde_json::from_slice(&request.body).unwrap();
                let pull = state.0.as_mut().unwrap();
                pull["body"] = body["body"].clone();
                ResponseTemplate::new(200).set_body_json(pull.clone())
            }
            _ => panic!("unexpected fixture request: {} {path}", request.method),
        }
    }
}

struct Fixture {
    // The adapter owns a separate runtime; keep the server alive across synchronous calls.
    _runtime: tokio::runtime::Runtime,
    _server: MockServer,
    root: tempfile::TempDir,
    api: Api,
    adapter: Adapter,
    delivery: Delivery,
}
impl Fixture {
    fn new(label: &str, values: &[&str], default: &str, obligations: &[&str]) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let api = Api::default();
        let server = runtime.block_on(async {
            let server = MockServer::start().await;
            Mock::given(|_: &Request| true)
                .respond_with(api.clone())
                .mount(&server)
                .await;
            server
        });
        let client = GitHub::new(
            AppConfig {
                id: 123,
                slug: "fixture".into(),
                pem: include_str!("../../mergify/tests/fixtures/throwaway-key.pem").into(),
                webhook_secret: "fixture".into(),
            },
            1,
            ClientOptions {
                api_url: format!("{}/", server.uri()).parse().unwrap(),
                max_retries: 0,
                mutation_interval: Duration::ZERO,
                max_wait: Duration::ZERO,
                ..Default::default()
            },
        )
        .unwrap();
        let root = tempfile::tempdir().unwrap();
        let repository = serde_json::from_value(json!({"owner":"fixture","name":"project","installation_id":1,
            "branch_template":"prismabuild-{issue}-tl3-{node}","generated_files":["BUILD.bazel","src/prismabuild/BUILD.bazel","tools/BUILD.bazel"],
            "pr_body_lines":[{"label":label,"values":values,"default":default}]})).unwrap();
        let adapter = Adapter::new(
            BTreeMap::from([(1, client)]),
            FakeWriter,
            1,
            Policy::default(),
            root.path(),
            Some(tl3_config::OperationsIssue {
                owner: "fixture".into(),
                name: "operations".into(),
                installation_id: 1,
                number: 81,
            }),
        )
        .unwrap()
        .with_repositories(&[repository])
        .unwrap();
        let delivery = serde_json::from_value(json!({"key":"effect:214:11","epoch":1,
            "output":{"Effect":{"node":0,"seq":11,"kind":"OpenPr","head":null}},"generation":11,
            "head_sha":"b".repeat(40),"repository":{"owner":"fixture","name":"project","installation":1},"issue":2640,"pull_request":null,
            "paths":["tests/test_pool.py","tests/BUILD.bazel","BUILD.bazel","src/prismabuild/BUILD.bazel","tools/BUILD.bazel"],"attempt":1,"round":0,
            "request_text":REQUEST,"evidence":{"goals":[{"id":"publish","stage":"publishing","publication":{"body_lines":obligations,"proof":true,"non_draft":true}}],
            "checks":[{"head_sha":"b".repeat(40),"base_sha":"a".repeat(40),"action_key":"c".repeat(64),"report":true,"pass":true,
                "results":[{"name":"deep","kind":"demonstrating","verdict":"pass","forcing":{"patches":[{"target":"example.RESIDENCY","value":"private-forcing-value"}]},
                "tests":[{"test":"tests/test_pool.py::test_pool","base":"assertion","head":"passed","head_forced":"passed","passed":true}]}]}],
            "reviews":[{"head_sha":"b".repeat(40),"verdict":"approve"}]}})).unwrap();
        Self {
            _runtime: runtime,
            _server: server,
            root,
            api,
            adapter,
            delivery,
        }
    }
    fn pb(lines: &[&str]) -> Self {
        Self::new("Change type", &["test", "fix", "chore"], "chore", lines)
    }
    fn publish(&mut self) -> tl3_controller::Result<tl3_controller::executor::Completion> {
        self.adapter.execute(&self.delivery)
    }
    fn body(&self) -> String {
        self.api.0.lock().unwrap().0.as_ref().unwrap()["body"]
            .as_str()
            .unwrap()
            .to_owned()
    }
    fn fresh_key(&mut self) {
        self.delivery.key.push_str("-recovery");
    }
}

#[test]
fn t629_pb_composition_preserves_obligation_proof_and_head() {
    let mut f = Fixture::pb(&["Refs #2640", "Change type: test"]);
    f.publish()
        .expect("compatible typed test obligation must win over BUILD ownership reservations");
    let body = f.body();
    assert_eq!(
        body.lines()
            .filter(|l| l.starts_with("Change type:"))
            .collect::<Vec<_>>(),
        ["Change type: test"]
    );
    assert_eq!(body.lines().filter(|l| *l == "Refs #2640").count(), 1);
    assert!(body.contains("<!-- tl3:recorded-evidence:start -->"));
    assert!(body.contains(&"b".repeat(40)) && body.contains(&"c".repeat(64)));
    assert!(body.contains(&GitHub::marker(&f.delivery.key)));
    assert!(!body.contains("private-forcing-value"));
    assert_eq!(f.api.0.lock().unwrap().1, 1);
}

#[test]
fn t629_explicit_values_and_other_repository_policy() {
    for value in ["test", "fix", "chore"] {
        let line = format!("Change type: {value}");
        let mut f = Fixture::pb(&[&line]);
        f.publish()
            .expect("one compatible explicit value takes priority");
        assert_eq!(
            f.body()
                .lines()
                .filter(|l| l.starts_with("Change type:"))
                .collect::<Vec<_>>(),
            [line.as_str()]
        );
    }
    let mut f = Fixture::new("Risk", &["low", "high"], "high", &["Risk: low"]);
    f.publish()
        .expect("policy reconciliation must be repository independent");
    assert_eq!(
        f.body()
            .lines()
            .filter(|l| l.starts_with("Risk:"))
            .collect::<Vec<_>>(),
        ["Risk: low"]
    );
}

#[test]
fn t629_generated_reservations_do_not_hide_true_production_edits() {
    for (production, expected) in [(false, "test"), (true, "fix")] {
        let mut f = Fixture::pb(&[]);
        if production {
            f.delivery.paths.push("src/prismabuild/pool.py".into());
        }
        f.publish().unwrap();
        assert!(
            f.body()
                .lines()
                .any(|l| l == format!("Change type: {expected}")),
            "generated ownership alone must not classify a production change"
        );
    }
}

#[test]
fn t629_conflicting_or_invalid_contract_never_mutates_github() {
    for lines in [
        vec!["Change type: test", "Change type: fix"],
        vec!["Change type: unknown-secret-value"],
    ] {
        let mut f = Fixture::pb(&lines);
        let error = f
            .publish()
            .expect_err("incompatible source contract must be refused");
        assert!(matches!(
            error,
            tl3_controller::Error::Permanent {
                class: "pr_policy",
                ..
            }
        ));
        assert!(
            format!("{error:?}").contains("rules:"),
            "refusal must name the incompatible configured rule"
        );
        let state = f.api.0.lock().unwrap();
        assert_eq!(
            (state.1, state.2),
            (0, 0),
            "no write before both contracts pass"
        );
    }
}

#[test]
fn t629_one_repair_then_validate_final_body_and_reconcile() {
    let mut f = Fixture::pb(&[]);
    f.delivery.paths = vec!["tests/test_pool.py".into()];
    f.publish().unwrap();
    f.delivery = {
        let mut value = serde_json::to_value(&f.delivery).unwrap();
        value["evidence"]["goals"][0]["publication"]["body_lines"] = json!(["Change type: test"]);
        serde_json::from_value(value).unwrap()
    };
    f.delivery.paths.extend(
        [
            "BUILD.bazel",
            "src/prismabuild/BUILD.bazel",
            "tools/BUILD.bazel",
        ]
        .map(String::from),
    );
    let original = f.body();
    f.api.0.lock().unwrap().0.as_mut().unwrap()["body"] = json!(format!(
        "Human context stays.\n{original}\nChange type: fix\nChange type: test\n"
    ));
    f.fresh_key();
    f.publish()
        .expect("one deterministic repair must preserve the explicit test obligation");
    let body = f.body();
    assert_eq!(
        body.lines()
            .filter(|l| l.starts_with("Change type:"))
            .collect::<Vec<_>>(),
        ["Change type: test"]
    );
    assert!(body.starts_with("Human context stays."));
    assert!(body.contains(&GitHub::marker("effect:214:11")));
    assert!(!body.contains("private-forcing-value"));
    assert_eq!(
        f.api.0.lock().unwrap().2,
        1,
        "repair uses one bounded mutation"
    );
    assert!(matches!(
        f.adapter.reconcile(&f.delivery).unwrap(),
        Reconciliation::Found(_)
    ));
    f.fresh_key();
    f.publish().unwrap();
    assert_eq!(f.body(), body, "already compliant body must be unchanged");
    assert_eq!(
        f.api.0.lock().unwrap().1,
        1,
        "crash/reconcile must not duplicate PRs"
    );
    assert_eq!(f.api.0.lock().unwrap().2, 1);
    assert!(f.root.path().is_dir());
}

#[test]
fn t629_redacted_obligation_is_refused_after_final_composition() {
    let mut f = Fixture::pb(&["Change type: test", "password=synthetic-secret"]);
    assert!(
        f.publish().is_err(),
        "redaction cannot silently discard a required exact obligation"
    );
    assert_eq!(f.api.0.lock().unwrap().1, 0);
}
