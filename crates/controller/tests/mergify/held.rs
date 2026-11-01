use super::*;
use tl3_controller::executor::Observation;

struct Fixture {
    api: Server,
    root: tempfile::TempDir,
    clock: head_checks::Clock,
}
impl Fixture {
    fn open(&self, settings: &tl3_config::Store) -> Controller {
        let mut registry = adapters::simulation(self.root.path()).unwrap();
        registry.register(
            Upstream::Mergify,
            self.api.adapter(&self.root.path().join("mergify"), 0),
        );
        let mut controller = Controller::open_with_clock(
            Store::open(settings).unwrap(),
            registry,
            bootstrap(),
            self.clock.clone(),
        )
        .unwrap();
        let mut allowances = tl3_config::Allowances::default();
        allowances.github_mergify.silence_secs = 1;
        allowances.github_mergify.no_progress_secs = 3;
        controller.set_allowances(allowances);
        controller
    }
    fn advance(&self) {
        self.clock
            .0
            .set(self.clock.0.get() + Duration::from_secs(1));
    }
}

fn request(number: u64) -> LabelledIssue {
    LabelledIssue {
        delivery: format!("held-{number}"),
        owner: "owner".into(),
        repo: "sandbox".into(),
        installation: 3,
        number,
        title: "change".into(),
        actor: 42,
        label: "tl3".into(),
    }
}

fn setup(settings: &tl3_config::Store, started: bool) -> (Fixture, Controller, String, String) {
    let fixture = Fixture {
        api: Server::new(),
        root: tempfile::tempdir().unwrap(),
        clock: head_checks::Clock(std::rc::Rc::new(std::cell::Cell::new(
            std::time::SystemTime::now(),
        ))),
    };
    {
        let mut state = fixture.api.state.lock().unwrap();
        state.lag = true;
        state.fail_reads = !started;
    }
    let mut controller = fixture.open(settings);
    controller.intake(request(12)).unwrap();
    let mut key = None;
    for _ in 0..300 {
        if !controller
            .drive_once_with(|boundary, effect_key| {
                if boundary == Boundary::BeforeDispatch {
                    let effect = Store::open(settings)
                        .unwrap()
                        .effect(effect_key)
                        .unwrap()
                        .unwrap();
                    let delivery: Delivery = serde_json::from_value(effect.payload).unwrap();
                    if matches!(
                        delivery.output,
                        Out::Effect {
                            kind: EffectKind::Enqueue,
                            ..
                        }
                    ) {
                        fixture.api.state.lock().unwrap().pull["head"]["sha"] =
                            json!(delivery.head_sha);
                        key = Some(effect_key.to_owned());
                    }
                }
            })
            .unwrap()
        {
            break;
        }
    }
    assert_eq!(
        controller.world().unwrap().nodes[0].phase,
        Phase::Publishing
    );
    assert_eq!(
        fixture.api.state.lock().unwrap().posts,
        usize::from(started)
    );
    fixture.api.state.lock().unwrap().fail_reads = true;
    for _ in 0..2 {
        fixture.advance();
        controller.look_due().unwrap();
        drain(&mut controller);
    }
    let key = key.unwrap();
    let snapshot = controller.snapshot().unwrap();
    let incident = snapshot
        .rows
        .iter()
        .find(|row| {
            row.table == "incidents" && row.value["effect"] == key && row.value["open"] == true
        })
        .expect("first read incident holds the enqueue");
    assert_eq!(incident.value["needs_rob"], false);
    let id = incident.id.clone();
    let reads = fixture.api.state.lock().unwrap().reads;
    drain(&mut controller);
    assert_eq!(
        fixture.api.state.lock().unwrap().reads,
        reads,
        "a hold waits for its scheduled look"
    );
    (fixture, controller, key, id)
}

fn resolved(controller: &mut Controller, id: &str) {
    assert!(
        controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|row| row.table == "incidents" && row.id == id && row.value["open"] == false),
        "recovery incident {id} must be resolved"
    );
}

pub(super) fn scenario(settings: &tl3_config::Store, mode: &str) {
    let (fixture, mut controller, key, id) = setup(settings, mode != "quiet");
    match mode {
        "quiet" => {
            drop(controller);
            controller = fixture.open(settings);
            drain(&mut controller);
            assert_eq!(fixture.api.state.lock().unwrap().posts, 0);
            let reads = fixture.api.state.lock().unwrap().reads;
            {
                let mut state = fixture.api.state.lock().unwrap();
                state.fail_reads = false;
                state.lag = false;
            }
            fixture.advance();
            controller.look_due().unwrap();
            assert_eq!(
                fixture.api.state.lock().unwrap().reads,
                reads + 1,
                "the incident look must probe the quiet upstream with one GET"
            );
            drain(&mut controller);
            assert_eq!(
                fixture.api.state.lock().unwrap().posts,
                1,
                "recovery retries the held enqueue"
            );
            assert_eq!(controller.world().unwrap().nodes.len(), 1);
            assert_eq!(controller.world().unwrap().nodes[0].phase, Phase::Queued);
            resolved(&mut controller, &id);
        }
        "success" => {
            let head = {
                let mut state = fixture.api.state.lock().unwrap();
                state.fail_reads = false;
                state.checks = super::fixture("running", "checks");
                state.pull["head"]["sha"].as_str().unwrap().to_owned()
            };
            controller
                .observe(
                    "late-queue-news",
                    Observation::Queue(tl3_controller::adapters::mergify::QueueFact {
                        repository: bootstrap().repos[0].clone(),
                        number: 100,
                        evidence: tl3_controller::adapters::mergify::Evidence::CiFailed {
                            check: 900,
                            completed_at: None,
                            head,
                            infra: true,
                        },
                    }),
                )
                .unwrap();
            assert!(controller.drive_once().unwrap());
            assert_eq!(
                controller.effect(&key).unwrap().unwrap().state,
                tl3_store::EffectState::Confirmed
            );
            resolved(&mut controller, &id);
        }
        "merged" | "closed-restart" => {
            let head = {
                let mut state = fixture.api.state.lock().unwrap();
                state.fail_reads = false;
                state.merged(false);
                state.merge_checks = json!([{"id":901,"name":"Sandbox tests",
                    "head_sha":state.pull["merge_commit_sha"],"status":"completed","conclusion":"success",
                    "app":{"id":15368,"slug":"github-actions"}}]);
                state.pull["head"]["sha"].as_str().unwrap().to_owned()
            };
            controller
                .observe(
                    "merge",
                    Observation::Merged {
                        node: NodeId(0),
                        sha: head,
                        checks_passed: true,
                    },
                )
                .unwrap();
            drain(&mut controller);
            assert_eq!(
                controller.world().unwrap().nodes[0].phase,
                Phase::Closed(Outcome::Merged)
            );
            if mode == "closed-restart" {
                let snapshot = controller.snapshot().unwrap();
                let mut value = snapshot
                    .rows
                    .iter()
                    .find(|row| row.table == "incidents" && row.id == id)
                    .unwrap()
                    .value
                    .clone();
                value["open"] = json!(true);
                let mut commit =
                    tl3_store::Commit::new("legacy-open-incident", json!({}), &snapshot);
                commit.rows.push(tl3_store::RowChange::Put {
                    table: "incidents".into(),
                    id: id.clone(),
                    value,
                });
                drop(controller);
                Store::open(settings).unwrap().commit(&commit).unwrap();
                let reads = fixture.api.state.lock().unwrap().reads;
                controller = fixture.open(settings);
                drain(&mut controller);
                assert_eq!(
                    fixture.api.state.lock().unwrap().reads,
                    reads,
                    "closed requests need no recovery GET"
                );
            }
            resolved(&mut controller, &id);
        }
        "stop" | "stop-restart" => {
            controller.intake(request(13)).unwrap();
            drain(&mut controller);
            assert!(!controller.world().unwrap().nodes[1].holds);
            let reads = fixture.api.state.lock().unwrap().reads;
            controller
                .observe(
                    "stop",
                    Observation::Kernel(tl3_kernel::Event::Command {
                        node: NodeId(0),
                        id: 1,
                        command: tl3_kernel::Command::Stop,
                    }),
                )
                .unwrap();
            if mode == "stop-restart" {
                for _ in 0..300 {
                    if controller.effect(&key).unwrap().unwrap().state
                        == tl3_store::EffectState::Cancelling
                    {
                        break;
                    }
                    assert!(controller.drive_once().unwrap());
                }
                assert_eq!(
                    controller.effect(&key).unwrap().unwrap().state,
                    tl3_store::EffectState::Cancelling
                );
                drop(controller);
                controller = fixture.open(settings);
            }
            for _ in 0..300 {
                if matches!(controller.world().unwrap().nodes[0].phase, Phase::Closed(_)) {
                    break;
                }
                if !controller.drive_once().unwrap() {
                    break;
                }
            }
            let world = controller.world().unwrap();
            assert_eq!(
                world.nodes[0].phase,
                Phase::Closed(Outcome::Stopped),
                "stop must not wait on held queue admission"
            );
            assert!(!world.nodes[0].holds);
            assert!(
                world.nodes[1].holds,
                "the next leaf must take the gate slot"
            );
            assert_eq!(
                fixture.api.state.lock().unwrap().reads,
                reads,
                "stop must finish without a Mergify read"
            );
            assert!(matches!(
                controller.effect(&key).unwrap().unwrap().state,
                tl3_store::EffectState::Confirmed | tl3_store::EffectState::Cancelled
            ));
            resolved(&mut controller, &id);
        }
        _ => panic!("unknown held-effect scenario: {mode}"),
    }
    assert!(controller.world().unwrap().valid());
    println!("test held_{mode} ... ok");
}
