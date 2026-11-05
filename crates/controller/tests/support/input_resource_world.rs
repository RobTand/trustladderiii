#![allow(dead_code)]
use super::controller_under_test as tl3_controller;
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
use tl3_kernel::{Event, NodeId, Reply};
use tl3_store::Store;
use wiremock::{Mock, MockServer, Request, ResponseTemplate, matchers::any};

struct Hold(Arc<Mutex<Vec<Delivery>>>);
impl Executor for Hold {
    fn execute(&mut self, delivery: &Delivery) -> tl3_controller::Result<Completion> {
        self.0.lock().unwrap().push(delivery.clone());
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

pub(super) struct World {
    pub controller: Controller,
    pub server: MockServer,
    pub runtime: tokio::runtime::Runtime,
    pub facts: Arc<Mutex<Vec<WriteRequest>>>,
    pub root: tempfile::TempDir,
    pub settings: tl3_config::Store,
    pub launches: Arc<Mutex<Vec<Delivery>>>,
    _db: super::Database,
}

impl World {
    pub fn new() -> Self {
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
            if path == "/repos/owner/sandbox/issues/2715" {
                return ResponseTemplate::new(200).set_body_json(json!({"number":2715,"title":"A prerequisite PR","body":null,"state":"open","pull_request":{"url":"https://api.github.com/repos/owner/sandbox/pulls/2715"},"updated_at":"2026-11-05T00:00:00Z"}));
            }
            if path == "/repos/owner/sandbox/pulls/2715" {
                return ResponseTemplate::new(200).set_body_json(json!({"number":2715,"user":{"id":42,"login":"owner"},"state":"open","merged_at":null,"head":{"ref":"topic","sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","repo":null},"base":{"ref":"main","sha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","repo":null},"updated_at":"2026-11-05T00:00:00Z"}));
            }
            if matches!(path, "/repos/owner/sandbox/issues/12" | "/repos/owner/sandbox/issues/17" | "/repos/owner/sandbox/issues/2713" | "/repos/owner/sandbox/issues/2703") {
                let number: u64 = path.rsplit('/').next().unwrap().parse().unwrap();
                return ResponseTemplate::new(200).set_body_json(json!({"number":number,"title":"Cover scanned sources","body":"Depends on #2713 merging first; after #2703 merges; also #4040, #4041 and #2715.","state":"open","pull_request":null,"updated_at":"2026-11-05T00:00:00Z"}));
            }
            ResponseTemplate::new(404)
        }).mount(&server));
        let root = tempfile::tempdir().unwrap();
        let mut db = super::Database::start();
        let settings = db.create("input_resources");
        let mut store = Store::open(&settings).unwrap();
        store.put_worker_materials("0", &json!({"issue":{"number":12,"author":"requester","comments":[],"title":"Cover scanned sources","body":"Depends on #2713 merging first; after #2703 merges; also #4040, #4041 and #2715."},"repo":{"readme":"Fixture repository","file_list":[],"materials":[]}})).unwrap();
        let repo = Repository {
            owner: "owner".into(),
            name: "sandbox".into(),
            installation: 3,
        };
        let github = GitHub::new(
            AppConfig {
                id: 123,
                slug: "factory".into(),
                pem: include_str!("../../../mergify/tests/fixtures/throwaway-key.pem").into(),
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
        let launches = Arc::new(Mutex::new(vec![]));
        registry.register(Upstream::Workers, Hold(launches.clone()));
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
            root,
            settings,
            launches,
            _db: db,
        }
    }

    pub fn drain(controller: &mut Controller) {
        for _ in 0..80 {
            if !controller.drive_once().unwrap() {
                return;
            }
        }
        panic!("input-resource fixture did not settle");
    }

    pub fn reply(&mut self, kind: &str, number: u64) {
        let generation = self.controller.world().unwrap().nodes[0].generation;
        self.controller.observe(&format!("reply-{generation}-{number}"), Observation::WorkerReply {
            node:NodeId(0), generation,role:"ingest".into(),model:None,
            reply:json!({"kind":"brief","brief":"Cover scanned sources","goals":[{"description":"Cover sources","verification":"test"}],"memory_ids":[],"files":[],"forecast":{"tokenizer":"o200k_base","brief_tokens":1,"file_tokens":{},"overhead_tokens":0,"tokens":1,"fit_tokens":100,"context_tokens":150,"fits":true},"merge_prerequisites":[{"kind":kind,"number":number,"source_quote":format!("#{number}")}]}),
        }).unwrap();
        Self::drain(&mut self.controller);
    }

    pub fn next_generation(&mut self) {
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

    pub fn incidents(&mut self) -> Vec<Value> {
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
