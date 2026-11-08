use super::*;
use crate as controller_under_test;
use crate::git_tests::{Repo, documents, git, support::Database};
use tl3_kernel::{Command as KernelCommand, Fact, NodeId, Notice, Phase, Priority, RepoId, World};

#[test]
fn flux_worker_test_input_needs_no_pb_tools() {
    let value = json!({"image": "sha256:".to_owned() + &"1".repeat(64),
        "execution": {"backend": "flux", "profile": "/fixture/attempt/execution.json"}});
    typed_round_trip::<tl3_contracts::roles::maker_input::Tests>(&value);
}

fn typed_round_trip<T: serde::de::DeserializeOwned + serde::Serialize>(value: &serde_json::Value) {
    let parsed: T = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(parsed).unwrap(), *value);
}

/// #819: exercise each real producer through the shared contract and sorted Value path.
pub(super) fn assert_typed_input(role: &str, value: &serde_json::Value) {
    fn check<T: serde::de::DeserializeOwned + serde::Serialize>(value: &serde_json::Value) {
        let typed: T = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(
            serde_json::to_vec(&serde_json::to_value(&typed).unwrap()).unwrap(),
            serde_json::to_vec(value).unwrap()
        );
        assert_ne!(
            serde_json::to_vec(&typed).unwrap(),
            serde_json::to_vec(value).unwrap()
        );
    }
    use tl3_contracts::roles::*;
    match role {
        "ingest" => check::<ingest_input::IngestInput>(value),
        "audit" => check::<audit_input::AuditInput>(value),
        "planner" => check::<planner_input::PlannerInput>(value),
        "maker" => check::<maker_input::MakerInput>(value),
        "writer" => check::<writer_input::WriterInput>(value),
        "classifier" => check::<classifier_input::ClassifierInput>(value),
        _ => {}
    }
}

fn materials(
    f: &Repo,
    config: &tl3_config::Config,
) -> (wiremock::MockServer, crate::production::Materials) {
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };
    let runtime = crate::executor::runtime().unwrap();
    let server = runtime.block_on(MockServer::start());
    for (verb, route, status, body) in [
        (
            "POST",
            "/app/installations/1/access_tokens",
            201,
            json!({"token":"fixture-installation-token","expires_at":"2099-01-01T00:00:00Z"}),
        ),
        (
            "GET",
            "/repos/RobTand/tl3-sandbox/issues/42",
            200,
            json!({"number":42,"title":"Production Materials issue","body":"Preserve evidence",
                "state":"open","updated_at":"2026-01-01T00:00:00Z"}),
        ),
        (
            "GET",
            "/repos/RobTand/tl3-sandbox/issues/42/comments",
            200,
            json!([]),
        ),
    ] {
        runtime.block_on(
            Mock::given(method(verb))
                .and(path(route))
                .respond_with(ResponseTemplate::new(status).set_body_json(body))
                .mount(&server),
        );
    }
    let client = tl3_github::GitHub::new(
        tl3_github::AppConfig {
            id: 123,
            slug: "fixture".into(),
            webhook_secret: "unused".into(),
            pem: include_str!("../../../../mergify/tests/fixtures/throwaway-key.pem").into(),
        },
        1,
        tl3_github::ClientOptions {
            api_url: format!("{}/", server.uri()).parse().unwrap(),
            max_retries: 0,
            mutation_interval: std::time::Duration::ZERO,
            ..Default::default()
        },
    )
    .unwrap();
    let mirror = config
        .worktrees_root
        .join(".repositories/robtand/tl3-sandbox.git");
    std::fs::create_dir_all(mirror.parent().unwrap()).unwrap();
    git(
        f.root.path(),
        &[
            "clone",
            "--bare",
            "--quiet",
            f.path.to_str().unwrap(),
            mirror.to_str().unwrap(),
        ],
    );
    // Only this fixture repository rewrites the production fetch to local Git.
    git(
        &mirror,
        &[
            "config",
            &format!("url.{}.insteadOf", f.path.display()),
            "https://github.com/RobTand/tl3-sandbox.git",
        ],
    );
    (
        server,
        crate::production::Materials::fixture_client(config, client),
    )
}

// These worlds are reached through kernel events, including repair, re-ingest,
// replan, rebase and pause/resume. No fixture invents an invalid kernel phase.
fn worlds() -> Vec<(World, Vec<tl3_kernel::Out>)> {
    let mut cases = Vec::new();
    for scenario in 0..8 {
        let tl3_kernel::StepResult {
            mut world,
            mut outputs,
            ..
        } = tl3_kernel::step(
            World::new(&[1], 1),
            Event::NewRequest {
                repo: RepoId(0),
                priority: Priority::Unranked,
            },
        );
        let mut changed = false;
        let mut paused_audit_reply = false;
        let mut paused_maker_reply = false;
        for _ in 0..120 {
            assert!(world.valid());
            cases.push((world.clone(), outputs));
            let Some(mut event) = world.witness() else {
                break;
            };
            let n = world.nodes[0];
            if paused_audit_reply {
                event = Event::Reply {
                    node: NodeId(0),
                    generation: n.generation,
                    reply: Reply::AuditAgrees,
                };
                paused_audit_reply = false;
            }
            if paused_maker_reply {
                event = Event::Reply {
                    node: NodeId(0),
                    generation: n.generation,
                    reply: Reply::MakerDone {
                        head: n.head.unwrap_or(tl3_kernel::HeadId(0)),
                    },
                };
                paused_maker_reply = false;
            }
            if !changed {
                let reply = match (scenario, &event) {
                    (
                        1,
                        Event::Reply {
                            reply: Reply::AuditAgrees,
                            ..
                        },
                    ) => Some(Reply::AuditDisagrees),
                    (
                        2,
                        Event::Reply {
                            reply: Reply::Approved { .. },
                            ..
                        },
                    ) => Some(Reply::ChangesRequested),
                    (
                        3,
                        Event::Reply {
                            reply: Reply::MakerDone { .. },
                            ..
                        },
                    ) => Some(Reply::MakerOutOfRoom),
                    (
                        4,
                        Event::Reply {
                            reply: Reply::Approved { .. },
                            ..
                        },
                    ) => Some(Reply::ReviewerOutOfRoom),
                    (
                        5,
                        Event::Reply {
                            reply: Reply::ChecksDone { .. },
                            ..
                        },
                    ) => Some(Reply::ChecksDone {
                        report: true,
                        pass: false,
                        in_footprint: true,
                    }),
                    _ => None,
                };
                if let Some(reply) = reply {
                    event = Event::Reply {
                        node: NodeId(0),
                        generation: n.generation,
                        reply,
                    };
                    changed = true;
                } else if scenario == 7
                    && matches!(
                        event,
                        Event::Reply {
                            reply: Reply::AuditAgrees,
                            ..
                        }
                    )
                {
                    event = Event::Command {
                        node: NodeId(0),
                        id: n.next_command_id().expect("fresh test command ID"),
                        command: KernelCommand::Pause,
                    };
                    paused_audit_reply = true;
                    changed = true;
                } else if scenario == 6 && n.phase == Phase::Queued {
                    event = Event::Fact {
                        node: NodeId(0),
                        fact: Fact::Conflict,
                    };
                    changed = true;
                } else if scenario == 0 && n.phase == Phase::Making && n.effect.is_none() {
                    paused_maker_reply = true;
                    event = Event::Command {
                        node: NodeId(0),
                        id: n.next_command_id().expect("fresh test command ID"),
                        command: KernelCommand::Pause,
                    };
                    changed = true;
                }
            }
            tl3_kernel::StepResult { world, outputs, .. } = tl3_kernel::step(world, event);
        }
    }
    cases
}

fn in_world(mut snapshot: Snapshot, world: &World) -> Snapshot {
    snapshot.rows.retain(|r| {
        !matches!(
            r.table.as_str(),
            "kernel" | "nodes" | "gates" | "file_sets" | "comment_jobs" | "unclassified_comments"
        )
    });
    snapshot
        .rows
        .extend(
            crate::snapshot::encode(world)
                .unwrap()
                .into_iter()
                .map(|((table, id), value)| tl3_store::WorldRow {
                    table,
                    id,
                    value,
                    version: snapshot.revision,
                    arrival: 0,
                }),
        );
    snapshot
}

#[test]
fn t653_real_builders_satisfy_every_role_and_dispatch_phase() {
    real_builders(false);
}

#[test]
fn selected_flux_real_builders_need_no_pb_config_in_both_input_paths() {
    real_builders(true);
}

fn real_builders(flux: bool) {
    let f = Repo::new();
    let mut db = Database::start();
    let mut config =
        tl3_config::Config::parse(include_str!("../../../../config/example.toml")).unwrap();
    config.store = db.create("all_input_contracts");
    config.workers.runs = f.root.path().join("runs");
    config.worktrees_root = f.root.path().join("worktrees");
    if flux {
        config.execution_backend = tl3_config::ExecutionBackend::Flux;
        config.flux = Some(tl3_config::Flux {
            instance_generation: "fixture/00000000000000000000000000000000".into(),
            executable: "/fixture/flux".into(),
            backend_cli: "/fixture/tl3-run-backend".into(),
            container_mode: tl3_config::ContainerMode::OwnScope,
            runtime_bundles: vec![],
            uri: "local:///fixture/flux".into(),
            environment: BTreeMap::new(),
            staging_root: f.root.path().join("staging"),
            output_root: f.root.path().join("outputs"),
            container_image: config.prismabuild.container_image.clone(),
        });
        config.prismabuild.tools_dir = PathBuf::new();
        config.prismabuild.container_image.clear();
    }
    let (_upstream, provider) = materials(&f, &config);
    let registry = crate::adapters::simulation(&f.root.path().join("fakes")).unwrap();
    let mut controller = crate::Controller::open(
        Store::open(&config.store).unwrap(),
        registry,
        crate::Bootstrap::from_config(&config).unwrap(),
    )
    .unwrap();
    controller
        .intake(crate::LabelledIssue {
            delivery: "contracts".into(),
            owner: "RobTand".into(),
            repo: "tl3-sandbox".into(),
            installation: 1,
            number: 42,
            title: "Preserve evidence".into(),
            actor: config.github.rob.id,
            label: "tl3".into(),
            labels: vec![],
        })
        .unwrap();
    // #839 / Q27: build the evidence through the controller, real store and
    // mock GitHub bounded read, then pass the retained row to both builders.
    let target = tl3_kernel::DependencyTarget {
        kind: tl3_kernel::PrerequisiteKind::Issue,
        number: 2763,
        request: None,
    };
    controller
        .observe(
            "t839-contract-declaration",
            Observation::Kernel(Event::Fact {
                node: NodeId(0),
                fact: Fact::Dependency {
                    target,
                    status: tl3_kernel::DependencyStatus::Pending,
                },
            }),
        )
        .unwrap();
    assert!(controller.drive_once().unwrap());
    crate::adapters::github::set_fake_issue(&f.root.path().join("fakes"), &serde_json::from_value(json!({"number":2763,"title":"Closed prerequisite","body":null,"state":"closed","state_reason":"not_planned","updated_at":"2026-11-03T23:00:00Z"})).unwrap());
    controller
        .observe(
            "t839-contract-catch-up",
            Observation::PrerequisiteRead {
                node: NodeId(0),
                repository: controller.bootstrap.repos[0].clone(),
                kind: target.kind,
                number: target.number,
            },
        )
        .unwrap();
    assert!(controller.drive_once().unwrap());
    assert_eq!(
        controller.world().unwrap().dependencies()[0].status,
        tl3_kernel::DependencyStatus::Delivered
    );
    let mut store = Store::open(&config.store).unwrap();
    let brief = json!({"kind":"brief","brief":"Preserve evidence","memory_ids":[],"goals":[{"description":"Preserve evidence","verification":"test"}]});
    let plan = json!({"kind":"leaf","files":["README.md"],"checks":[{"name":"evidence","kind":"regression","node_ids":["test_value.py::test_value"],"goal_ids":[0]}]});
    documents(
        &mut store,
        &[
            ("briefs", brief),
            ("plans", plan),
            (
                "feedback",
                json!({"generation":0,"maker":[{"kind":"failed_check","goal_ids":["goal-1"],"report":"Independent checking requested a repair"}],"planner":[],"conflict":null}),
            ),
            (
                "goals",
                json!([{"description":"Preserve evidence","verification":"test"}]),
            ),
        ],
    );
    let supervisor = Supervisor::new(
        &config,
        &f.root.path().join("records"),
        &mut crate::store_support::ownership(),
    )
    .unwrap()
    .with_materials(provider);
    let snapshot = store.load().unwrap();
    let mut commit = tl3_store::Commit::new("contract-postmortem", json!(null), &snapshot);
    commit.postmortems.push(
        crate::postmortems::owed(
            &snapshot,
            NodeId(0),
            "contract".into(),
            crate::postmortems::Reason::RobAsked,
            None,
        )
        .unwrap(),
    );
    store.commit(&commit).unwrap();
    let postmortem = store.owed_postmortem().unwrap().unwrap().id;
    let mut coverage = std::collections::BTreeSet::new();
    let mut phases = std::collections::BTreeSet::new();
    let mut variants = std::collections::BTreeSet::new();
    let mut request_cases = std::collections::BTreeSet::new();
    let mut auxiliary_phases = std::collections::BTreeSet::new();
    for (index, (world, outputs)) in worlds().into_iter().enumerate() {
        let n = world.nodes[0];
        phases.insert(n.phase.name());
        let mut snapshot = in_world(store.load().unwrap(), &world);
        snapshot.rows.push(tl3_store::WorldRow {
            table: "check_reports".into(),
            id: format!("0:{}", f.head),
            value: json!({"base_sha":f.base,"results":[]}),
            version: snapshot.revision,
            arrival: 0,
        });
        let mut delivery = f.delivery();
        delivery.generation = n.generation;
        delivery.round = n.round;
        delivery.attempt = n.attempt;
        delivery.replanned = n.replanned;
        delivery.question = n.question;
        for output in outputs {
            match output {
                tl3_kernel::Out::Effect {
                    node,
                    seq,
                    kind: EffectKind::StartRun(kernel_role),
                    head,
                } => {
                    let role = protocol::role(kernel_role).unwrap();
                    if !request_cases.insert(format!(
                        "{role}:{:?}:{}:{}:{}",
                        n.phase, n.round, n.replanned, n.review_restarted
                    )) {
                        continue;
                    }
                    delivery.output = Out::Effect {
                        node,
                        seq,
                        kind: EffectKind::StartRun(kernel_role),
                        head,
                    };
                    let request = supervisor
                        .request(&delivery, role, &mut store, &snapshot)
                        .unwrap_or_else(|error| panic!("{role} {:?}: {error}", n.phase));
                    protocol::validate_inputs(role, &request["inputs"]).unwrap();
                    if flux && matches!(role, "maker" | "planner") {
                        assert!(request["inputs"]["tests"].get("tools_path").is_none());
                        assert_eq!(request["inputs"]["tests"]["execution"]["backend"], "flux");
                    }
                    if flux && role == "planner" {
                        assert_eq!(
                            request["inputs"]["tool_capabilities"][0]["executor"],
                            "flux_pytest"
                        );
                    }
                    assert!(
                        crate::input_check::request(&request).unwrap().is_empty(),
                        "{role}: envelope rejected"
                    );
                    typed_round_trip::<tl3_contracts::request::WorkerRequest>(&request);
                    if role == "planner" {
                        assert_eq!(
                            request["inputs"]["prerequisite_closures"],
                            json!([{"kind":"issue","number":2763,"reason":"not_planned"}])
                        );
                    }
                    if role == "reviewer" {
                        typed_round_trip::<tl3_contracts::roles::reviewer_input::ReviewerInput>(
                            &request["inputs"],
                        );
                        let typed: tl3_contracts::roles::reviewer_input::ReviewerInput =
                            serde_json::from_value(request["inputs"].clone()).unwrap();
                        assert_eq!(
                            serde_json::to_string(&serde_json::to_value(&typed).unwrap()).unwrap(),
                            serde_json::to_string(&request["inputs"]).unwrap()
                        );
                        assert_ne!(
                            serde_json::to_string(&typed).unwrap(),
                            serde_json::to_string(&request["inputs"]).unwrap()
                        );
                    }
                    if matches!(role, "ingest" | "audit") {
                        assert_eq!(
                            request["inputs"]["issue"]["title"],
                            "Production Materials issue"
                        );
                    }
                    if role == "maker" {
                        assert_eq!(
                            store.worker_materials("0").unwrap().unwrap()["check_run"]["plan"][0]["name"],
                            "check-1"
                        );
                    }
                    coverage.insert(role);
                    variants.insert(format!("{role}:{:?}:{}", n.phase, n.round));
                }
                tl3_kernel::Out::Notice {
                    notice:
                        notice @ (Notice::Ask { .. }
                        | Notice::Summary(_)
                        | Notice::MeasuresPosted
                        | Notice::Note(_)),
                    node,
                } => {
                    if !request_cases
                        .insert(format!("writer:{notice:?}:{:?}:{:?}", n.phase, n.question))
                    {
                        continue;
                    }
                    delivery.output = Out::Notice { node, notice };
                    let input = crate::adapters::github::writer_input(
                        &delivery,
                        Some(notice),
                        "Preserve evidence",
                        "Preserve evidence",
                    )
                    .unwrap();
                    protocol::validate_inputs("writer", &serde_json::to_value(&input).unwrap())
                        .unwrap();
                    assert_typed_input("writer", &serde_json::to_value(&input).unwrap());
                    coverage.insert("writer");
                }
                _ => {}
            }
        }
        // Classifier and requested postmortems are controller-side jobs, so their
        // contracts apply in every reachable phase, including paused and closed.
        if !auxiliary_phases.insert(format!("{:?}:{}", n.phase, n.paused)) {
            continue;
        }
        let comment = json!({"id":index+100,"body":"Continue","issue_url":"https://api.github.com/repos/RobTand/tl3-sandbox/issues/42","user":{"id":config.github.rob.id,"login":"owner"},"created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z"});
        snapshot.rows.push(tl3_store::WorldRow {
            table: "unclassified_comments".into(),
            id: format!("contract-{index}"),
            value: json!({"repository":delivery.repository,"issue":42,"comment":comment}),
            version: snapshot.revision,
            arrival: 0,
        });
        assert!(controller.schedule_comment(&snapshot).unwrap());
        let effect = store
            .effects()
            .unwrap()
            .into_iter()
            .find(|effect| effect.key == format!("classify:contract-{index}"))
            .unwrap();
        let classify: Delivery = serde_json::from_value(effect.payload).unwrap();
        let request = supervisor
            .request(&classify, "classifier", &mut store, &snapshot)
            .unwrap();
        protocol::validate_inputs("classifier", &request["inputs"]).unwrap();
        typed_round_trip::<tl3_contracts::request::WorkerRequest>(&request);
        coverage.insert("classifier");
        delivery.output = Out::Comment {
            node: NodeId(0),
            kind: CommentKind::Postmortem,
        };
        delivery.steering = Some(crate::steering::Task::Postmortem {
            id: postmortem,
            run_key: "contract".into(),
        });
        let request = supervisor
            .request(&delivery, "postmortem", &mut store, &snapshot)
            .unwrap();
        assert_eq!(
            request["inputs"]["case"]["prerequisite_closures"],
            json!([{"kind":"issue","number":2763,"reason":"not_planned"}])
        );
        protocol::validate_inputs("postmortem", &request["inputs"]).unwrap();
        typed_round_trip::<tl3_contracts::request::WorkerRequest>(&request);
        typed_round_trip::<tl3_contracts::roles::postmortem_input::PostmortemInput>(
            &request["inputs"],
        );
        coverage.insert("postmortem");
    }
    assert_eq!(
        coverage,
        [
            "ingest",
            "audit",
            "planner",
            "maker",
            "reviewer",
            "classifier",
            "writer",
            "postmortem"
        ]
        .into_iter()
        .collect()
    );
    for phase in [
        "Ingesting",
        "Auditing",
        "Unplanned",
        "Planning",
        "Ready",
        "Making",
        "Checking",
        "Reviewing",
        "Publishing",
        "Queued",
        "Rebasing",
        "AwaitingRob",
        "Stopping",
        "Closed",
    ] {
        assert!(phases.contains(phase), "missing {phase}");
    }
    assert!(variants.contains("planner:Planning { must_split: true }:0"));
    assert!(variants.iter().any(|v| v.starts_with("maker:Making:1")));
    assert!(
        variants
            .iter()
            .any(|v| v.starts_with("reviewer:Reviewing:1"))
    );
}

#[path = "../../../tests/support/input_resource_world.rs"]
mod resource_world;

/// Auditing / #832-A: the real next-generation producer preserves every
/// PB #2781 objection as its own stable audit_objections[index] item.
#[test]
fn audit_repair_receives_separately_indexed_real_objections() {
    let golden: Value =
        serde_json::from_str(include_str!("../../../tests/fail_fast/pb-2781.json")).unwrap();
    let objections = &golden["records"][1]["reply"]["objections"];
    let mut w = resource_world::World::new();
    w.reply("issue", 2713);
    let generation = w.controller.world().unwrap().nodes[0].generation;
    w.controller
        .observe(
            "audit-disagrees",
            Observation::WorkerReply {
                node: NodeId(0),
                generation,
                role: "audit".into(),
                reply: golden["records"][1]["reply"].clone(),
                model: None,
                plan_check: None,
            },
        )
        .unwrap();
    resource_world::World::drain(&mut w.controller);
    let retry = w.launches.lock().unwrap().last().unwrap().clone();
    assert_eq!(
        w.controller.world().unwrap().nodes[0].phase,
        Phase::Ingesting
    );
    assert!(retry.generation > generation);
    let mut config =
        tl3_config::Config::parse(include_str!("../../../../config/example.toml")).unwrap();
    config.github.repos[0].owner = "owner".into();
    config.github.repos[0].name = "sandbox".into();
    config.github.repos[0].installation_id = 3;
    config.store = w.settings.clone();
    config.workers.runs = w.root.path().join("runs");
    config.models.key_file = w.root.path().join("fixture-key");
    config.hindsight.endpoint = "http://127.0.0.1:9".parse().unwrap();
    let supervisor = Supervisor::new(
        &config,
        &w.root.path().join("records"),
        &mut crate::store_support::ownership(),
    )
    .unwrap();
    let mut store = Store::open(&w.settings).unwrap();
    let snapshot = store.load().unwrap();
    let request = supervisor
        .request(&retry, "ingest", &mut store, &snapshot)
        .unwrap();
    assert_eq!(&request["inputs"]["audit_objections"], objections);
    let items = request["inputs"]["audit_objections"].as_array().unwrap();
    assert_eq!(items.len(), objections.as_array().unwrap().len());
    typed_round_trip::<tl3_contracts::roles::ingest_input::IngestInput>(&request["inputs"]);
}

/// Q7/Q51/#833: the retry sees the rejected declaration before producing its next reply.
#[test]
fn ingest_validation_retry_receives_the_safe_kind_diagnostic() {
    let mut w = resource_world::World::new();
    w.reply("pull_request", 2713);
    let retry = w.launches.lock().unwrap().last().unwrap().clone();
    assert_eq!(retry.generation, 1);
    let mut config =
        tl3_config::Config::parse(include_str!("../../../../config/example.toml")).unwrap();
    config.github.repos[0].owner = "owner".into();
    config.github.repos[0].name = "sandbox".into();
    config.github.repos[0].installation_id = 3;
    config.store = w.settings.clone();
    config.workers.runs = w.root.path().join("runs");
    config.models.key_file = w.root.path().join("fixture-key");
    config.hindsight.endpoint = "http://127.0.0.1:9".parse().unwrap();
    let supervisor = Supervisor::new(
        &config,
        &w.root.path().join("records"),
        &mut crate::store_support::ownership(),
    )
    .unwrap();
    let mut store = Store::open(&w.settings).unwrap();
    let snapshot = store.load().unwrap();
    let request = supervisor
        .request(&retry, "ingest", &mut store, &snapshot)
        .unwrap();
    assert_eq!(
        request["inputs"]["audit_objections"],
        json!([
            "Requested GitHub pull request owner/sandbox#2713; observed GitHub issue owner/sandbox#2713. The prerequisite declaration must be corrected."
        ]),
        "the actual next ingest input must explain its rejected kind"
    );
    // No corrected reply exists until the input above has been inspected.
    w.reply("issue", 2713);
    assert_eq!(
        w.controller.world().unwrap().nodes[0].phase,
        Phase::Auditing
    );
    let accepted = w.controller.snapshot().unwrap();
    assert!(
        !accepted
            .rows
            .iter()
            .any(|row| row.table == "input_resource_contexts")
    );
}
