#[path = "../../store/tests/support/mod.rs"]
mod support;

use serde_json::json;
use std::sync::{Arc, Mutex};
use tl3_controller::{
    Bootstrap, Controller, LabelledIssue, adapters,
    executor::{Completion, Delivery, Executor, Observation, Output, Reconciliation, Upstream},
};
use tl3_kernel::{EffectKind, Notice, Outcome, Phase, Role};
use tl3_store::Store;

struct Workers {
    inner: adapters::workers::Fake,
    mode: &'static str,
    seen: Arc<Mutex<Vec<Delivery>>>,
}
impl Executor for Workers {
    fn execute(&mut self, d: &Delivery) -> tl3_controller::Result<Completion> {
        self.seen.lock().unwrap().push(d.clone());
        let forecast = json!({"tokenizer":"o200k_base", "brief_tokens":1, "file_tokens":{}, "overhead_tokens":0,
            "tokens":1,"fit_tokens":100,"context_tokens":150,"fits":true});
        let reply = match d.output {
            Output::Effect {
                kind: EffectKind::StartRun(Role::Ingest),
                ..
            } => Some((
                "ingest",
                json!({
                "kind":"brief", "brief":"Complete the requested change.", "goals":[{"description":"Complete the requested change.","verification":"test"}],
                "memory_ids":[], "files":[], "forecast":forecast}),
            )),
            Output::Effect {
                kind: EffectKind::StartRun(Role::Audit),
                ..
            } if self.mode == "audit" => Some((
                "audit",
                json!({"verdict":"agree", "objections":[], "protected_paths":["tools/ci/bazel_gate.py"]}),
            )),
            Output::Effect {
                kind: EffectKind::StartRun(Role::Plan),
                ..
            } => {
                let path = if self.mode == "nonmatching" {
                    "tests/test_gate.py"
                } else {
                    "tools/ci/bazel_gate.py"
                };
                Some((
                    "planner",
                    json!({"kind":"cant_meet","report":"All goals require the gate change.",
                    "files":[],"protected_paths":[path],"forecast":forecast,"violations":[]}),
                ))
            }
            _ => None,
        };
        if let Some((role, reply)) = reply {
            Ok(Completion::confirmed(vec![Observation::WorkerReply {
                node: d.node(),
                generation: d.generation,
                role: role.into(),
                reply,
                model: None,
            }]))
        } else {
            self.inner.execute(d)
        }
    }
    fn reconcile(&mut self, d: &Delivery) -> tl3_controller::Result<Reconciliation> {
        self.inner.reconcile(d)
    }
}

fn run(
    body: &str,
    mode: &'static str,
    should_close: bool,
    expected_planners: usize,
) -> Vec<Delivery> {
    let mut db = support::Database::start();
    let settings = db.create("protected_request");
    let root = tempfile::tempdir().unwrap();
    let mut config = tl3_config::Config::parse(include_str!("../../config/example.toml")).unwrap();
    config.planning.protected_paths.push("global/**".into());
    config.github.repos[0].protected_paths = vec!["tools/ci/**".into()];
    let repo = &config.github.repos[0];
    config.github.operations = Some(tl3_config::OperationsIssue {
        owner: repo.owner.clone(),
        name: repo.name.clone(),
        installation_id: repo.installation_id,
        number: 81,
    });
    let runs = Arc::new(Mutex::new(Vec::new()));
    let mut registry = adapters::simulation(root.path()).unwrap();
    registry.register(
        Upstream::Workers,
        Workers {
            inner: adapters::workers::Fake::new(root.path()),
            mode,
            seen: runs.clone(),
        },
    );
    let mut store = Store::open(&settings).unwrap();
    store
        .put_worker_materials("0", &json!({"issue":{"body":body}}))
        .unwrap();
    let mut c = Controller::open(
        Store::open(&settings).unwrap(),
        registry,
        Bootstrap::from_config(&config).unwrap(),
    )
    .unwrap();
    c.set_request_protection(&config);
    c.intake(LabelledIssue {
        delivery: "protected".into(),
        owner: repo.owner.to_ascii_uppercase(),
        repo: repo.name.to_ascii_uppercase(),
        installation: repo.installation_id,
        number: 2636,
        title: "Retry budget-only gate failures".into(),
        actor: config.github.rob.id,
        label: "tl3".into(),
        labels: vec![],
    })
    .unwrap();
    for _ in 0..500 {
        if !c.drive_once().unwrap() {
            break;
        }
    }
    let n = c.world().unwrap().nodes[0];
    if should_close {
        assert_eq!(
            n.phase,
            Phase::Closed(Outcome::NotActionable),
            "protected requests must close without a Rob question"
        );
        assert!(n.question.is_none());
        assert_eq!(n.credit.spent, 0);
        let snapshot = c.snapshot().unwrap();
        let reason = snapshot
            .rows
            .iter()
            .find(|r| r.table == "close_reasons")
            .expect("durable close reason");
        assert_eq!(reason.value["reason"], "protected_paths");
        assert!(
            reason.value["paths"]
                .as_array()
                .unwrap()
                .contains(&json!("tools/ci/bazel_gate.py"))
        );
        let effects = store.effects().unwrap();
        let summary = effects
            .iter()
            .map(|e| serde_json::from_value::<Delivery>(e.payload.clone()).unwrap())
            .find(|d| {
                matches!(
                    d.output,
                    Output::Notice {
                        notice: Notice::Summary(_),
                        ..
                    }
                )
            })
            .expect("summary");
        let text = serde_json::to_value(&summary).unwrap()["steering"]["ConsoleNote"]["text"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(
            text.contains("tools/ci/bazel_gate.py")
                && text.contains("outside TL3")
                && text.contains("restart")
        );
        assert!(effects.iter().any(|e| {
            e.payload["issue"] == 81
                && e.payload["steering"]["ConsoleNote"]["text"]
                    .as_str()
                    .is_some_and(|s| s.contains("2636") && s.contains("tools/ci/bazel_gate.py"))
        }));
        drop(c);
        let reopened = Controller::open(
            Store::open(&settings).unwrap(),
            adapters::simulation(root.path()).unwrap(),
            Bootstrap::from_config(&config).unwrap(),
        )
        .unwrap();
        drop(reopened);
    } else {
        assert_ne!(n.phase, Phase::Closed(Outcome::NotActionable));
    }
    assert_eq!(
        runs.lock()
            .unwrap()
            .iter()
            .filter(|d| matches!(
                d.output,
                Output::Effect {
                    kind: EffectKind::StartRun(Role::Plan),
                    ..
                }
            ))
            .count(),
        expected_planners
    );
    let notes = store
        .effects()
        .unwrap()
        .into_iter()
        .map(|effect| serde_json::from_value::<Delivery>(effect.payload).unwrap())
        .filter(|delivery| {
            matches!(
                delivery.steering,
                Some(tl3_controller::steering::Task::ConsoleNote { .. })
            )
        })
        .collect();
    db.stop();
    notes
}

#[test]
fn pb_2636_footprint_closes_before_audit_or_planning() {
    run(
        "## Behaviour wanted\n1. In `tools/ci/bazel_gate.py`, add budget-only reruns.\n2. Write a runner marker that the gate reads.\n\n## Expected footprint\n- `tools/bazel/python_runner.py`\n- `tools/ci/bazel_gate.py`\n- `tests/test_bazel_gate.py`\n\n## Out of scope\n- Changing budgets.",
        "planner",
        true,
        0,
    );
}
#[test]
fn planner_protected_cant_meet_closes_on_first_reply() {
    run("Implement budget-only reruns.", "planner", true, 1);
}
#[test]
fn audit_can_discover_a_required_protected_change() {
    run("Implement budget-only reruns.", "audit", true, 0);
}
#[test]
fn mentions_negated_edits_and_unprotected_tests_keep_normal_flow() {
    for body in [
        "We use `tools/ci/bazel_gate.py`.",
        "## Goals\n- This doesn't touch `tools/ci/bazel_gate.py`.\n- Add tests in `tests/test_gate.py`.",
        "## Expected footprint\n- `tests/test_gate.py`\n\n## Out of scope\n- Edit `tools/ci/bazel_gate.py`.",
    ] {
        run(body, "nonmatching", false, 2);
    }
}

const PATH_TOKEN: &str = "ghp_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8";

fn post_protected_refusal(operations: bool, oversized: bool) {
    use std::{collections::BTreeMap, time::Duration};
    use tl3_github::{AppConfig, ClientOptions, GitHub, message::Policy};
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    let notes = run(
        &format!(
            "## Expected footprint\n- `tools/ci/bazel_gate.py`\n- `.github/workflows/{PATH_TOKEN}.yml`"
        ),
        "planner",
        true,
        0,
    );
    assert_eq!(notes.len(), 2);
    let mut note = notes
        .into_iter()
        .find(|note| (note.issue == 81) == operations)
        .unwrap();
    let Some(tl3_controller::steering::Task::ConsoleNote { text }) = &mut note.steering else {
        panic!("protected refusal must use ConsoleNote");
    };
    assert!(
        text.contains(PATH_TOKEN),
        "fixture must retain the requested path before publication"
    );
    if oversized {
        text.push_str(&"x".repeat(1024 * 1024 + 1));
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let server = runtime.block_on(MockServer::start());
    let comments = format!(
        "/repos/{}/{}/issues/{}/comments",
        note.repository.owner, note.repository.name, note.issue
    );
    runtime.block_on(async {
        Mock::given(method("POST"))
            .and(path(format!(
                "/app/installations/{}/access_tokens",
                note.repository.installation
            )))
            .respond_with(ResponseTemplate::new(201).set_body_json(
                json!({"token":"fixture-access", "expires_at":"2099-01-01T00:00:00Z"}),
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(&comments))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .mount(&server)
            .await;
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../github/tests/fixtures/issue-comment-created.json"
        ))
        .unwrap();
        Mock::given(method("POST"))
            .and(path(&comments))
            .respond_with(ResponseTemplate::new(201).set_body_json(fixture["comment"].clone()))
            .mount(&server)
            .await;
    });
    let client = GitHub::new(
        AppConfig {
            id: 123,
            slug: "factory".into(),
            pem: include_str!("../../mergify/tests/fixtures/throwaway-key.pem").into(),
            webhook_secret: "fixture-secret".into(),
        },
        note.repository.installation,
        ClientOptions {
            api_url: format!("{}/", server.uri()).parse().unwrap(),
            max_retries: 0,
            mutation_interval: Duration::ZERO,
            max_wait: Duration::ZERO,
            ..ClientOptions::default()
        },
    )
    .unwrap();
    let root = tempfile::tempdir().unwrap();
    let mut adapter = adapters::github::Adapter::new(
        BTreeMap::from([(note.repository.installation, client)]),
        adapters::workers::writer::FakeWriter,
        note.epoch,
        Policy::default(),
        root.path(),
        Some(tl3_config::OperationsIssue {
            owner: note.repository.owner.clone(),
            name: note.repository.name.clone(),
            installation_id: note.repository.installation,
            number: 81,
        }),
    )
    .unwrap();
    let result = adapter.execute(&note);
    let posted = runtime
        .block_on(server.received_requests())
        .unwrap()
        .into_iter()
        .filter(|request| request.method == "POST" && request.url.path() == comments)
        .collect::<Vec<_>>();
    if oversized {
        assert!(
            matches!(result, Err(tl3_controller::Error::RedactionInfra(_))),
            "ConsoleNote must propagate typed redaction infrastructure errors: {result:?}"
        );
        assert!(
            posted.is_empty(),
            "failed redaction must prevent publication"
        );
    } else {
        result.unwrap();
        assert_eq!(posted.len(), 1);
        let payload: serde_json::Value = serde_json::from_slice(&posted[0].body).unwrap();
        let body = payload["body"].as_str().unwrap();
        assert!(
            !body.contains(PATH_TOKEN),
            "protected refusal leaked a requested path in POST {comments}: {body}"
        );
        assert!(body.contains("[REDACTED]"));
        assert!(body.contains("outside TL3"));
    }
}

#[test]
fn protected_refusal_summary_redacts_requested_paths_before_posting() {
    post_protected_refusal(false, false);
}

#[test]
fn protected_refusal_operations_redacts_requested_paths_before_posting() {
    post_protected_refusal(true, false);
}

#[test]
fn protected_refusal_redaction_failure_prevents_both_posts() {
    for operations in [false, true] {
        post_protected_refusal(operations, true);
    }
}
