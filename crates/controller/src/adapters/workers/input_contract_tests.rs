use super::*;
use crate::git_tests::{Repo, documents, support::Database};
use tl3_kernel::{Command as KernelCommand, Fact, NodeId, Notice, Phase, Priority, RepoId, World};

// These worlds are reached through kernel events, including repair, re-ingest,
// replan, rebase and pause/resume. No fixture invents an invalid kernel phase.
fn worlds() -> Vec<(World, Vec<tl3_kernel::Out>)> {
    let mut cases = Vec::new();
    for scenario in 0..7 {
        let (mut world, mut outputs) = tl3_kernel::step(
            World::new(&[1], 1),
            Event::NewRequest {
                repo: RepoId(0),
                priority: Priority::Unranked,
            },
        );
        let mut changed = false;
        for _ in 0..120 {
            assert!(world.valid());
            cases.push((world.clone(), outputs));
            let Some(mut event) = world.witness() else {
                break;
            };
            let n = world.nodes[0];
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
                } else if scenario == 6 && n.phase == Phase::Queued {
                    event = Event::Fact {
                        node: NodeId(0),
                        fact: Fact::Conflict,
                    };
                    changed = true;
                } else if scenario == 0 && n.phase == Phase::Making && n.effect.is_none() {
                    event = Event::Command {
                        node: NodeId(0),
                        id: n.last_cmd + 1,
                        command: KernelCommand::Pause,
                    };
                    changed = true;
                }
            }
            (world, outputs) = tl3_kernel::step(world, event);
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
    let f = Repo::new();
    let mut db = Database::start();
    let mut config =
        tl3_config::Config::parse(include_str!("../../../../config/example.toml")).unwrap();
    config.store = db.create("all_input_contracts");
    config.workers.runs = f.root.path().join("runs");
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
    let mut store = Store::open(&config.store).unwrap();
    let brief = json!({"kind":"brief","brief":"Preserve evidence","memory_ids":[],"goals":[{"description":"Preserve evidence","verification":"test"}]});
    let plan = json!({"kind":"leaf","files":["README.md"],"checks":[{"name":"evidence","kind":"regression","node_ids":["test_value.py::test_value"],"goal_ids":[0]}]});
    let repo = crate::production::repository_materials(&f.path, &f.base).unwrap();
    store.put_worker_materials("0", &json!({"issue":{"number":42,"title":"Preserve evidence","body":"Preserve evidence","author":"owner","comments":[]},"repo":repo,"maker":crate::production::maker_inputs(&brief,&plan).unwrap(),"base_checkout":{"repository":f.path,"commit":f.base}})).unwrap();
    documents(
        &mut store,
        &[
            ("briefs", brief),
            ("plans", plan),
            (
                "goals",
                json!([{"description":"Preserve evidence","verification":"test"}]),
            ),
        ],
    );
    let supervisor = Supervisor::new(&config, &f.root.path().join("records")).unwrap();
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
                    kind: EffectKind::StartRun(role),
                    head,
                } => {
                    let role = protocol::role(role).unwrap();
                    delivery.output = Out::Effect {
                        node,
                        seq,
                        kind: EffectKind::StartRun(match role {
                            "ingest" => tl3_kernel::Role::Ingest,
                            "audit" => tl3_kernel::Role::Audit,
                            "planner" => tl3_kernel::Role::Plan,
                            "maker" => tl3_kernel::Role::Make,
                            "reviewer" => tl3_kernel::Role::Review,
                            _ => unreachable!(),
                        }),
                        head,
                    };
                    let request = supervisor
                        .request(&delivery, role, &mut store, &snapshot)
                        .unwrap_or_else(|error| panic!("{role} {:?}: {error}", n.phase));
                    protocol::validate_inputs(role, &request["inputs"]).unwrap();
                    coverage.insert(role);
                    variants.insert(format!("{role}:{:?}:{}", n.phase, n.round));
                }
                tl3_kernel::Out::Notice {
                    notice:
                        notice @ (Notice::Ask { .. }
                        | Notice::Summary(_)
                        | Notice::MeasuresPosted
                        | Notice::Note(_)),
                    ..
                } => {
                    let input = crate::adapters::github::writer_input(
                        &delivery,
                        Some(notice),
                        "Preserve evidence",
                        "Preserve evidence",
                    )
                    .unwrap();
                    protocol::validate_inputs("writer", &serde_json::to_value(input).unwrap())
                        .unwrap();
                    coverage.insert("writer");
                }
                _ => {}
            }
        }
        // Classifier and requested postmortems are controller-side jobs, so their
        // contracts apply in every reachable phase, including paused and closed.
        let comment = json!({"id":index+100,"body":"Continue","user":{"id":config.github.rob.id,"login":"owner"},"created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z"});
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
        coverage.insert("classifier");
        delivery.steering = Some(crate::steering::Task::Postmortem {
            id: postmortem,
            run_key: "contract".into(),
        });
        let request = supervisor
            .request(&delivery, "postmortem", &mut store, &snapshot)
            .unwrap();
        protocol::validate_inputs("postmortem", &request["inputs"]).unwrap();
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
        "Planning",
        "Making",
        "Checking",
        "Reviewing",
        "Publishing",
        "Queued",
        "Rebasing",
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
