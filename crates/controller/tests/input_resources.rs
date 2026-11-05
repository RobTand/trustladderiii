mod support;

use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tl3_controller::{
    Bootstrap, Controller, LabelledIssue, Repository, adapters,
    adapters::workers::writer::{FakeWriter, WriteRequest, Writer, WriterReply},
    executor::{Completion, Delivery, Executor, Observation, Reconciliation, Upstream},
};
use tl3_github::{AppConfig, ClientOptions, GitHub, message::Policy};
use tl3_kernel::{Event, NodeId, Phase, Reply};
use tl3_store::Store;
use wiremock::{Mock, MockServer, Request, ResponseTemplate, matchers::any};

struct Hold;
impl Executor for Hold {
    fn execute(&mut self, _: &Delivery) -> tl3_controller::Result<Completion> {
        Ok(Completion::confirmed(vec![]))
    }
    fn reconcile(&mut self, _: &Delivery) -> tl3_controller::Result<Reconciliation> {
        Ok(Reconciliation::Found(Box::new(Completion::confirmed(
            vec![],
        ))))
    }
}

struct Capture(Arc<Mutex<Vec<WriteRequest>>>);
impl Writer for Capture {
    fn draft(&mut self, key: &str, input: &WriteRequest) -> tl3_controller::Result<WriterReply> {
        self.0.lock().unwrap().push(input.clone());
        FakeWriter.draft(key, input)
    }
}

struct World {
    controller: Controller,
    server: MockServer,
    runtime: tokio::runtime::Runtime,
    facts: Arc<Mutex<Vec<WriteRequest>>>,
    _root: tempfile::TempDir,
    _db: support::Database,
}

impl World {
    fn new() -> Self {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let server = runtime.block_on(MockServer::start());
        runtime.block_on(Mock::given(any()).respond_with(|request: &Request| {
            let path = request.url.path();
            if path == "/app/installations/3/access_tokens" {
                return ResponseTemplate::new(201).set_body_json(json!({"token":"fixture-token","expires_at":"2099-01-01T00:00:00Z"}));
            }
            if path.ends_with("/comments") {
                if request.method.as_str() == "GET" {
                    return ResponseTemplate::new(200).set_body_json(json!([]));
                }
                let body: Value = serde_json::from_slice(&request.body).unwrap();
                return ResponseTemplate::new(201).set_body_json(json!({"id":99,"body":body["body"],"user":{"id":123,"login":"factory[bot]"},"created_at":"2026-11-05T00:00:00Z","updated_at":"2026-11-05T00:00:00Z","issue_url":"https://api.github.com/repos/owner/sandbox/issues/12"}));
            }
            if matches!(path, "/repos/owner/sandbox/issues/12" | "/repos/owner/sandbox/issues/17" | "/repos/owner/sandbox/issues/2713" | "/repos/owner/sandbox/issues/2703") {
                let number: u64 = path.rsplit('/').next().unwrap().parse().unwrap();
                return ResponseTemplate::new(200).set_body_json(json!({"number":number,"title":"Cover scanned sources","body":"Depends on #2713 merging first; after #2703 merges; also #4040 and #4041.","state":"open","pull_request":null,"updated_at":"2026-11-05T00:00:00Z"}));
            }
            ResponseTemplate::new(404)
        }).mount(&server));
        let root = tempfile::tempdir().unwrap();
        let mut db = support::Database::start();
        let settings = db.create("input_resources");
        let mut store = Store::open(&settings).unwrap();
        store.put_worker_materials("0", &json!({"issue":{"title":"Cover scanned sources","body":"Depends on #2713 merging first; after #2703 merges; also #4040 and #4041."}})).unwrap();
        let repo = Repository {
            owner: "owner".into(),
            name: "sandbox".into(),
            installation: 3,
        };
        let github = GitHub::new(
            AppConfig {
                id: 123,
                slug: "factory".into(),
                pem: include_str!("../../mergify/tests/fixtures/throwaway-key.pem").into(),
                webhook_secret: "fixture".into(),
            },
            3,
            ClientOptions {
                api_url: format!("{}/", server.uri()).parse().unwrap(),
                max_retries: 0,
                mutation_interval: std::time::Duration::ZERO,
                ..ClientOptions::default()
            },
        )
        .unwrap();
        let operations = Some(tl3_config::OperationsIssue {
            owner: "owner".into(),
            name: "sandbox".into(),
            installation_id: 3,
            number: 17,
        });
        let facts = Arc::new(Mutex::new(vec![]));
        let mut registry = adapters::simulation(root.path()).unwrap();
        registry.register(Upstream::Workers, Hold);
        registry.register(
            Upstream::GitHub,
            adapters::github::Adapter::new(
                BTreeMap::from([(3, github)]),
                Capture(facts.clone()),
                0,
                Policy::default(),
                root.path(),
                operations.clone(),
            )
            .unwrap(),
        );
        let mut controller = Controller::open(
            store,
            registry,
            Bootstrap {
                repos: vec![repo],
                operations,
                rob: 42,
                slots: 1,
                pool: 1,
                fail_fast: false,
            },
        )
        .unwrap();
        controller
            .intake(LabelledIssue {
                delivery: "label".into(),
                owner: "owner".into(),
                repo: "sandbox".into(),
                installation: 3,
                number: 12,
                title: "Cover scanned sources".into(),
                actor: 42,
                label: "tl3".into(),
                labels: vec![],
            })
            .unwrap();
        Self::drain(&mut controller);
        Self {
            controller,
            server,
            runtime,
            facts,
            _root: root,
            _db: db,
        }
    }

    fn drain(controller: &mut Controller) {
        for _ in 0..80 {
            if !controller.drive_once().unwrap() {
                return;
            }
        }
        panic!("input-resource fixture did not settle");
    }

    fn reply(&mut self, kind: &str, number: u64) {
        let generation = self.controller.world().unwrap().nodes[0].generation;
        self.controller.observe(&format!("reply-{generation}-{number}"), Observation::WorkerReply {
            node:NodeId(0), generation,role:"ingest".into(),model:None,
            reply:json!({"kind":"brief","brief":"Cover scanned sources","goals":[{"description":"Cover sources","verification":"test"}],"memory_ids":[],"files":[],"forecast":{"tokenizer":"o200k_base","brief_tokens":1,"file_tokens":{},"overhead_tokens":0,"tokens":1,"fit_tokens":100,"context_tokens":150,"fits":true},"merge_prerequisites":[{"kind":kind,"number":number,"source_quote":format!("#{number}")}]}),
        }).unwrap();
        Self::drain(&mut self.controller);
    }

    fn next_generation(&mut self) {
        let generation = self.controller.world().unwrap().nodes[0].generation;
        self.controller
            .observe(
                &format!("failed-{generation}"),
                Observation::Kernel(Event::Reply {
                    node: NodeId(0),
                    generation,
                    reply: Reply::RunFailed,
                }),
            )
            .unwrap();
        Self::drain(&mut self.controller);
    }

    fn incidents(&mut self) -> Vec<Value> {
        self.controller
            .snapshot()
            .unwrap()
            .rows
            .into_iter()
            .filter(|row| row.table == "incidents" && row.value["open"] == true)
            .map(|row| row.value)
            .collect()
    }
}

/// Q59/#833: an issue number must never be read as a PR merely because ingest says so.
#[test]
fn mismatch_is_validated_before_the_pull_endpoint_and_uses_bounded_run_failures() {
    let mut w = World::new();
    w.reply("pull_request", 2713);
    let first = w.controller.world().unwrap().nodes[0];
    assert_eq!(
        first.generation, 1,
        "the existing invalid-reply path must rerun ingest"
    );
    w.reply("pull_request", 2713);
    assert!(
        w.controller.world().unwrap().nodes[0].waiting_incident,
        "Q7 must bound repeated validation failures"
    );
    let requests = w.runtime.block_on(w.server.received_requests()).unwrap();
    assert!(
        !requests
            .iter()
            .any(|r| r.url.path().contains("/pulls/2713")),
        "a known issue must not reach the 404 PR endpoint"
    );
    let incidents = w.incidents();
    assert_eq!(incidents.len(), 1);
    assert_eq!(
        incidents[0]["signature"]["class"],
        "prerequisite_kind_mismatch"
    );
    assert_eq!(incidents[0]["cause"], "tl3");
    let facts = &incidents[0]["input_failure"]["resource_failure"];
    assert_eq!(
        facts["requested"],
        json!({"kind":"github_pull_request","repository":"owner/sandbox","number":2713})
    );
    assert_eq!(facts["problem"]["observed"]["kind"], "github_issue");
    let count = requests.len();
    World::drain(&mut w.controller);
    assert_eq!(
        w.runtime
            .block_on(w.server.received_requests())
            .unwrap()
            .len(),
        count,
        "an open incident must halt this operation"
    );
}

/// Q59/#833: the #2713-shaped valid issue prerequisite still reaches independent audit.
#[test]
fn correct_issue_prerequisite_reaches_audit() {
    let mut w = World::new();
    w.reply("issue", 2703);
    assert_eq!(
        w.controller.world().unwrap().nodes[0].phase,
        Phase::Auditing
    );
    assert!(w.incidents().is_empty());
    let snapshot = w.controller.snapshot().unwrap();
    let declarations = &snapshot
        .rows
        .iter()
        .find(|r| r.table == "merge_prerequisites")
        .unwrap()
        .value;
    assert_eq!(declarations[0]["target"]["kind"], "Issue");
}

/// Q7/#833: changing generation and observation content must not create another cause lineage.
#[test]
fn not_found_dedupes_across_generations_for_the_same_request_and_resource() {
    let mut w = World::new();
    w.reply("issue", 4040);
    w.next_generation();
    w.reply("issue", 4040);
    assert_eq!(
        w.incidents().len(),
        1,
        "one open incident per request, cause, and typed resource"
    );
}

/// Q7/#833: distinct missing resources must remain distinct incident causes.
#[test]
fn different_missing_resources_are_not_deduped() {
    let mut w = World::new();
    w.reply("issue", 4040);
    w.next_generation();
    w.reply("issue", 4041);
    assert_eq!(w.incidents().len(), 2);
}

/// Q7/#833: the adapter identity must reach stored diagnostics, writer facts, and the note.
#[test]
fn missing_resource_is_named_in_the_incident_writer_facts_and_posted_note() {
    let mut w = World::new();
    w.reply("issue", 4040);
    let incidents = w.incidents();
    assert_eq!(incidents.len(), 1);
    let failure = &incidents[0]["input_failure"]["resource_failure"];
    assert_eq!(
        failure["requested"],
        json!({"kind":"github_issue","repository":"owner/sandbox","number":4040})
    );
    assert!(
        incidents[0]["summary"]
            .as_str()
            .unwrap()
            .contains("GitHub issue owner/sandbox#4040")
    );
    assert!(
        w.facts
            .lock()
            .unwrap()
            .iter()
            .any(|input| input.facts["input_resource_failure"] == *failure),
        "typed identity must reach the writer"
    );
    let requests = w.runtime.block_on(w.server.received_requests()).unwrap();
    assert!(
        requests
            .iter()
            .filter(|r| r.method.as_str() == "POST" && r.url.path().ends_with("/comments"))
            .any(
                |r| serde_json::from_slice::<Value>(&r.body).unwrap()["body"]
                    .as_str()
                    .unwrap()
                    .contains("GitHub issue owner/sandbox#4040")
            ),
        "the posted note must identify the resource"
    );
}

/// Q7/#833: successful later translation is not an authorized incident-clear event.
#[test]
fn a_corrected_declaration_does_not_clear_an_unprobed_input_incident() {
    let mut w = World::new();
    w.reply("issue", 4040);
    w.next_generation();
    w.reply("issue", 2713);
    assert_eq!(
        w.controller.world().unwrap().nodes[0].phase,
        Phase::Auditing
    );
    assert_eq!(w.incidents().len(), 1);
}
