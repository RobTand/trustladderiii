use super::*;
use crate::{Bootstrap, Controller, LabelledIssue, git_tests::Repo, store_support::Database};
use tl3_kernel::{NodeId, Phase};
use tl3_store::Commit;

struct Fixture {
    controller: Controller,
    supervisor: Supervisor,
    config: tl3_config::Config,
    repo: Repo,
    _db: Database,
}

impl Fixture {
    fn new() -> Self {
        Self::with_materials(true)
    }

    fn with_materials(include_materials: bool) -> Self {
        let repo = Repo::new();
        let mut db = Database::start();
        let mut config =
            tl3_config::Config::parse(include_str!("../../../../config/example.toml")).unwrap();
        config.store = db.create("step_feedback");
        config.workers.runs = repo.root.path().join("runs");
        let supervisor = Supervisor::new(
            &config,
            &repo.root.path().join("records"),
            &mut crate::store_support::ownership(),
        )
        .unwrap();
        let mut controller = Controller::open(
            Store::open(&config.store).unwrap(),
            crate::adapters::simulation(&repo.root.path().join("fakes")).unwrap(),
            Bootstrap::from_config(&config).unwrap(),
        )
        .unwrap();
        controller
            .intake(LabelledIssue {
                delivery: "feedback".into(),
                owner: "RobTand".into(),
                repo: "tl3-sandbox".into(),
                installation: 1,
                number: 42,
                title: "Repair the failing check".into(),
                actor: config.github.rob.id,
                label: "tl3".into(),
                labels: vec![],
            })
            .unwrap();
        if include_materials {
            controller.store.put_worker_materials("0", &json!({"base_checkout":{"repository":repo.path,"commit":repo.base}, "maker": {
                "brief":"Return hello.", "goals":[{"id":"greeting","description":"Return hello.","verification":"test"}],
                "plan":{"files":["README.md"],"checks":[{"name":"hello","kind":"demonstrating","tests":["test_greeting.py::test_hello"],"goal_ids":[0]}]}
            }})).unwrap();
        }
        crate::git_tests::documents(
            &mut controller.store,
            &[
                (
                    "briefs",
                    json!({"kind":"brief", "brief":"Return hello.", "goals":[{"description":"Return hello.","verification":"test"}],"memory_ids":[]}),
                ),
                (
                    "goals",
                    json!([{"id":"greeting","description":"Return hello.","verification":"test"}]),
                ),
            ],
        );
        Self {
            controller,
            supervisor,
            config,
            repo,
            _db: db,
        }
    }

    fn seed(&mut self, phase: Phase, pos: u8) {
        let snapshot = self.controller.snapshot().unwrap();
        let mut world = self.controller.world().unwrap();
        let repo = self
            .controller
            .store
            .intern_repo("RobTand", "tl3-sandbox")
            .unwrap();
        let path = self
            .controller
            .store
            .intern_path(repo, "README.md")
            .unwrap();
        let footprint = world.intern_footprint([tl3_kernel::FileId(path.get() as u64)]);
        let n = &mut world.nodes[0];
        n.phase = phase;
        n.pos = pos;
        n.head = Some(self.controller.head(NodeId(0), &self.repo.head).unwrap());
        n.generation = 10;
        n.run = matches!(
            phase,
            Phase::Making | Phase::Reviewing | Phase::Planning { .. } | Phase::Rebasing
        )
        .then_some(10);
        n.job = (phase == Phase::Checking).then_some(10);
        n.effect = (phase == Phase::Publishing).then_some(tl3_kernel::InFlight {
            kind: tl3_kernel::EffectKind::PostApproval,
            seq: 1,
            cancelling: true,
        });
        n.next_seq = 2;
        n.pool = n.run.is_some() && phase != Phase::Rebasing;
        n.holds = !matches!(phase, Phase::Planning { .. });
        n.footprint = footprint;
        n.checked = n.head;
        n.approved = n.head;
        n.pr_open = matches!(phase, Phase::Publishing | Phase::Queued);
        n.had_pr = n.pr_open;
        n.paused = true;
        n.ticket = None;
        n.round = 0;
        n.attempt = 1;
        n.credit.start = 100;
        n.credit.ready = true;
        assert!(
            world.valid(),
            "seed must be a valid settled world: {:?}",
            world.check()
        );
        let mut commit = Commit::new(
            format!("feedback-seed-{}", snapshot.revision.get()),
            json!(null),
            &snapshot,
        );
        commit.rows.extend(crate::snapshot::changes(
            &crate::snapshot::encode(&self.controller.world().unwrap()).unwrap(),
            crate::snapshot::encode(&world).unwrap(),
        ));
        self.controller.store.commit(&commit).unwrap();
    }

    fn observe(&mut self, observation: Observation) {
        let key = format!(
            "failure-{}",
            self.controller.snapshot().unwrap().revision.get()
        );
        self.controller.observe(&key, observation).unwrap();
        for _ in 0..32 {
            assert!(self.controller.drive_once().unwrap());
            if !crate::input_errors::kernel_backlog(&self.controller.snapshot().unwrap()) {
                return;
            }
        }
        panic!("the observation must drain through the controller");
    }

    fn change(&mut self, change: impl FnOnce(&mut tl3_kernel::World)) {
        let snapshot = self.controller.snapshot().unwrap();
        let mut world = self.controller.world().unwrap();
        let before = crate::snapshot::encode(&world).unwrap();
        change(&mut world);
        assert!(
            world.valid(),
            "changed fixture must stay settled: {:?}",
            world.check()
        );
        let mut commit = Commit::new(
            format!("fixture-change-{}", snapshot.revision.get()),
            json!(null),
            &snapshot,
        );
        commit.rows.extend(crate::snapshot::changes(
            &before,
            crate::snapshot::encode(&world).unwrap(),
        ));
        self.controller.store.commit(&commit).unwrap();
    }

    fn planner(&mut self) -> Value {
        let snapshot = self.controller.snapshot().unwrap();
        self.supervisor
            .build_request(
                &self.repo.delivery(),
                "planner",
                &mut Store::open(&self.config.store).unwrap(),
                &snapshot,
            )
            .unwrap()["inputs"]
            .clone()
    }

    fn worker(&mut self, role: &str, reply: Value) {
        self.observe(Observation::WorkerReply {
            node: NodeId(0),
            generation: 10,
            role: role.into(),
            reply,
            model: None,
            plan_check: None,
        });
    }

    fn maker(&mut self) -> Value {
        let snapshot = self.controller.snapshot().unwrap();
        let n = self.controller.world().unwrap().nodes[0];
        let mut delivery = self.repo.delivery();
        delivery.generation = n.generation;
        delivery.attempt = n.attempt;
        delivery.round = n.round;
        self.supervisor
            .build_request(
                &delivery,
                "maker",
                &mut Store::open(&self.config.store).unwrap(),
                &snapshot,
            )
            .unwrap()["inputs"]
            .clone()
    }
}

#[test]
fn publishing_enqueue_without_inflight_enqueue_delivers_ci_feedback() {
    let mut f = Fixture::new();
    // #862: the agenda is at Enqueue while an earlier effect is retiring.
    f.seed(Phase::Publishing, 3);
    f.observe(Observation::CiFailed {
        node: NodeId(0),
        infra: false,
        checks: vec![crate::findings::CiFailure {
            name: "test_greeting".into(),
            summary: "expected hello, got goodbye".into(),
            url: Some("https://ci.invalid/check/42".into()),
        }],
    });
    assert_eq!(f.controller.world().unwrap().nodes[0].round, 1);
    let inputs = f.maker();
    assert!(
        inputs["feedback"]
            .to_string()
            .contains("expected hello, got goodbye"),
        "the D2 repair maker needs the failing check: {inputs}"
    );
    assert!(
        f.controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "feedback")
    );
}

#[test]
fn failed_run_details_reach_the_retry_maker() {
    let mut f = Fixture::new();
    f.seed(Phase::Making, 2);
    f.observe(Observation::WorkerFailed {
        node: NodeId(0),
        generation: 10,
        failure_class: "worker_exit".into(),
        failure_message: "maker exited during reproduction".into(),
    });
    f.observe(Observation::Steering {
        node: NodeId(0),
        command: tl3_kernel::Command::Resume,
    });
    let snapshot = f.controller.snapshot().unwrap();
    assert!(
        snapshot.pending_effects.iter().any(|effect| {
            let delivery: Delivery = serde_json::from_value(effect.payload.clone()).unwrap();
            matches!(
                delivery.output,
                crate::executor::Output::Effect {
                    kind: tl3_kernel::EffectKind::PrepareWorktree,
                    ..
                }
            )
        }),
        "resuming the retry must prepare its worktree"
    );
    let inputs = f.maker();
    assert!(
        inputs["feedback"]
            .to_string()
            .contains("maker exited during reproduction"),
        "retry needs its failure evidence: {inputs}"
    );
}

#[test]
fn failed_checks_reach_the_fix_maker() {
    let mut f = Fixture::new();
    f.seed(Phase::Checking, 1);
    f.observe(Observation::Checks {
        node: NodeId(0),
        generation: 10,
        action_key: "a".repeat(64),
        base_sha: f.repo.base.clone(),
        sha: f.repo.head.clone(),
        changed_paths: vec![],
        failed_tests: vec![crate::findings::FailedTest {
            test: "test_hello.py::test_greeting".into(),
            message: "hello is missing".into(),
        }],
        report: true,
        pass: false,
        results: vec![],
    });
    assert_eq!(f.controller.world().unwrap().nodes[0].round, 1);
    assert_eq!(
        f.maker()["feedback"][0]["failed_tests"][0]["message"],
        "hello is missing"
    );
    assert!(
        f.controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "check_reports")
    );
}

#[test]
fn failed_round_goal_materials_preserve_unbound_findings_and_nested_precedence() {
    for (materials, expected) in [
        (None, json!([])),
        (Some(json!({})), json!([])),
        (Some(json!({"goals":[{"id":"flat"}]})), json!(["flat"])),
        (
            Some(json!({"goals":[{"id":"flat"}], "maker":{}})),
            json!([]),
        ),
        (
            Some(json!({"goals":[{"id":"flat"}], "maker":{"goals":[
                {"id":" nested "}, {"id":"nested"}
            ]}})),
            json!([" nested ", "nested"]),
        ),
    ] {
        let mut f = Fixture::with_materials(false);
        if let Some(materials) = &materials {
            f.controller
                .store
                .put_worker_materials("0", materials)
                .unwrap();
        }
        f.seed(Phase::Publishing, 3);
        f.observe(Observation::CiFailed {
            node: NodeId(0),
            infra: false,
            checks: vec![],
        });
        let snapshot = f.controller.snapshot().unwrap();
        let document = crate::feedback::read(&snapshot, NodeId(0)).unwrap();
        assert_eq!(document.maker.len(), 1, "materials: {materials:?}");
        assert_eq!(
            json!(document.maker[0])["goal_ids"],
            expected,
            "materials: {materials:?}"
        );
        assert!(!document.maker[0].report.as_str().is_empty());
    }
}

#[test]
fn malformed_goal_materials_reject_failed_round_without_feedback() {
    for (goals, message) in [
        (json!([{"id":" "}]), "maker goal needs a string id"),
        (json!([{"id":7}]), "maker goal needs a string id"),
        (json!([{}]), "maker goal needs a string id"),
        (json!({"id":"goal"}), "maker goals must be an array"),
    ] {
        let mut f = Fixture::new();
        f.controller
            .store
            .put_worker_materials("0", &json!({"maker":{"goals":goals}}))
            .unwrap();
        f.seed(Phase::Publishing, 3);
        let before = f.controller.world().unwrap();
        let observation = Observation::CiFailed {
            node: NodeId(0),
            infra: false,
            checks: vec![],
        };
        let value = json!(observation);
        f.controller.observe("invalid-goals", observation).unwrap();
        let snapshot = f.controller.snapshot().unwrap();
        let row = snapshot
            .rows
            .iter()
            .find(|row| row.table == "inbox" && row.value == value)
            .unwrap();
        let error = f
            .controller
            .consume_kernel_input(&snapshot, row)
            .unwrap_err();
        assert!(
            matches!(error, crate::Error::Invalid(actual) if actual == message),
            "{error:?}"
        );
        assert_eq!(f.controller.world().unwrap(), before);
        assert_eq!(f.controller.snapshot().unwrap().revision, snapshot.revision);
        assert!(
            !f.controller
                .snapshot()
                .unwrap()
                .rows
                .iter()
                .any(|row| row.table == "feedback"),
            "invalid goal materials must not persist partial feedback"
        );
    }
}

#[test]
fn review_blockers_reach_the_fix_maker_once() {
    let mut f = Fixture::new();
    f.seed(Phase::Reviewing, 1);
    f.worker("reviewer", json!({"verdict":"changes", "blocking_findings":[{"citation":{"kind":"goal","id":"greeting"},"problem":"Missing greeting", "evidence":"Returns goodbye"}], "findings":[]}));
    let inputs = f.maker();
    assert_eq!(inputs["feedback"].as_array().unwrap().len(), 1);
    assert_eq!(
        inputs["feedback"][0]["report"],
        "Missing greeting: Returns goodbye"
    );
    assert_eq!(inputs["current_review_findings"][0]["id"], "review-10-0");
    let snapshot = f.controller.snapshot().unwrap();
    let stored = snapshot
        .rows
        .iter()
        .find(|r| r.table == "feedback")
        .unwrap();
    assert_eq!(
        stored.value["generation"],
        f.controller.world().unwrap().nodes[0].generation
    );
}

#[test]
fn maker_cant_meet_report_reaches_the_next_maker() {
    let mut f = Fixture::new();
    f.seed(Phase::Making, 2);
    f.worker(
        "maker",
        json!({"kind":"cant_meet", "report":"The public API cannot represent the goal"}),
    );
    assert_eq!(f.controller.world().unwrap().nodes[0].attempt, 2);
    assert!(
        f.maker()["feedback"][0]["report"]
            .as_str()
            .unwrap()
            .contains("The public API cannot represent the goal")
    );
}

#[test]
fn stale_kernel_and_worker_replies_create_no_feedback() {
    for observation in [
        Observation::Kernel(Event::Reply {
            node: NodeId(0),
            generation: 9,
            reply: Reply::MakerCantMeet,
        }),
        Observation::WorkerReply {
            node: NodeId(0),
            generation: 9,
            role: "maker".into(),
            reply: json!({"kind":"cant_meet","report":"stale"}),
            model: None,
            plan_check: None,
        },
        Observation::Kernel(Event::Reply {
            node: NodeId(0),
            generation: 9,
            reply: Reply::ChangesRequested,
        }),
    ] {
        let mut f = Fixture::new();
        f.seed(Phase::Making, 2);
        f.observe(observation);
        assert!(!f.controller.snapshot().unwrap().rows.iter().any(|r| {
            ["feedback", "maker_findings", "findings", "rebase_conflicts"]
                .contains(&r.table.as_str())
        }));
        assert!(f.maker().get("feedback").is_none());
    }
}

#[test]
fn replan_carries_the_current_failure_to_the_planner() {
    let mut f = Fixture::new();
    f.seed(Phase::Making, 2);
    f.change(|w| {
        w.nodes[0].attempt = 3;
        w.nodes[0].round = 1;
    });
    f.worker(
        "maker",
        json!({"kind":"cant_meet", "report":"Need a different plan"}),
    );
    assert!(matches!(
        f.controller.world().unwrap().nodes[0].phase,
        Phase::Planning { .. }
    ));
    assert!(
        f.planner()["findings"]
            .to_string()
            .contains("Need a different plan")
    );
}

#[test]
fn planner_violations_and_report_reach_its_retry() {
    let mut f = Fixture::new();
    f.seed(Phase::Planning { must_split: false }, 1);
    f.worker("planner", json!({"kind":"cant_meet", "report":"Need a smaller request", "files":["README.md"], "violations":["Bad selector", "Bad selector"],
        "forecast":{"tokenizer":"o200k_base","brief_tokens":1,"file_tokens":{"README.md":0},"overhead_tokens":0,"tokens":1,"fit_tokens":100,"context_tokens":150,"fits":true}}));
    assert_eq!(
        f.planner()["findings"],
        json!(["Bad selector", "Need a smaller request"])
    );
}

#[test]
fn rebase_conflict_is_head_scoped_and_cleared_on_rebased() {
    let mut f = Fixture::new();
    f.seed(Phase::Rebasing, 1);
    f.observe(Observation::RebaseConflict {
        node: NodeId(0),
        generation: 10,
        head: f.repo.head.clone(),
        target: f.repo.base.clone(),
        files: vec!["README.md".into()],
    });
    assert!(
        f.maker()["feedback"]
            .to_string()
            .contains("Resolve the merge")
    );
    let snapshot = f.controller.snapshot().unwrap();
    let mut delivery = f.repo.delivery();
    delivery.head_sha = Some("b".repeat(40));
    let other = f
        .supervisor
        .build_request(
            &delivery,
            "maker",
            &mut Store::open(&f.config.store).unwrap(),
            &snapshot,
        )
        .unwrap();
    assert!(
        !other["inputs"]["feedback"]
            .to_string()
            .contains("Resolve the merge")
    );
    f.seed(Phase::Rebasing, 1);
    f.observe(Observation::Rebased {
        node: NodeId(0),
        generation: 10,
        sha: f.repo.head.clone(),
    });
    assert!(
        !f.maker()["feedback"]
            .to_string()
            .contains("Resolve the merge")
    );
    assert!(
        f.controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .find(|row| row.table == "feedback")
            .unwrap()
            .value["conflict"]
            .is_null()
    );
}

#[test]
fn try_again_guidance_reaches_the_planner_through_console_intake() {
    let mut f = Fixture::new();
    f.seed(Phase::Making, 2);
    f.change(|w| {
        w.nodes[0].phase = Phase::AwaitingRob;
        w.nodes[0].pos = 0;
        w.nodes[0].run = None;
        w.nodes[0].pool = false;
        w.nodes[0].holds = false;
        w.nodes[0].question = Some(tl3_kernel::Question {
            id: 1,
            kind: tl3_kernel::QKind::CantMeet,
            cont: tl3_kernel::Cont::Planning { must_split: false },
            cost: 1,
            clarified: false,
        });
        w.next_question = 2;
    });
    tl3_store::ConsoleCommands::open(&f.config.store).unwrap().submit("00000000-0000-4000-8000-000000000921", 0,
        &json!({"kind":"answer", "question":1, "choice":{"answer":"try_again", "guidance":"Use the public API"}})).unwrap();
    crate::ingress::drain(&mut f.controller).unwrap();
    for _ in 0..8 {
        if f.controller.snapshot().unwrap().rows.iter().any(|r| {
            r.table == "feedback"
                && r.value["planner"]
                    .as_array()
                    .is_some_and(|items| items.contains(&json!("Use the public API")))
        }) {
            break;
        }
        assert!(f.controller.drive_once().unwrap());
    }
    assert!(
        f.planner()["findings"]
            .as_array()
            .unwrap()
            .contains(&json!("Use the public API"))
    );
}

#[test]
fn missing_canonical_feedback_starts_an_empty_document() {
    let mut f = Fixture::new();
    f.seed(Phase::Making, 2);
    let document = crate::feedback::read(&f.controller.snapshot().unwrap(), NodeId(0)).unwrap();
    assert_eq!(document, crate::feedback::Document::default());
    assert!(f.maker().get("feedback").is_none());
    f.observe(Observation::WorkerFailed {
        node: NodeId(0),
        generation: 10,
        failure_class: "worker_exit".into(),
        failure_message: "first failure".into(),
    });
    let document = crate::feedback::read(&f.controller.snapshot().unwrap(), NodeId(0)).unwrap();
    assert_eq!(document.maker.len(), 1);
    assert_eq!(document.maker[0].report.as_str(), "first failure");
    assert!(document.planner.is_empty());
}

#[test]
fn infrastructure_ci_failure_creates_no_maker_feedback() {
    let mut f = Fixture::new();
    f.seed(Phase::Queued, 0);
    f.observe(Observation::CiFailed {
        node: NodeId(0),
        infra: true,
        checks: vec![],
    });
    assert!(
        !f.controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|row| row.table == "feedback" || row.table == "maker_findings")
    );
    assert!(f.maker().get("feedback").is_none());
}

#[test]
fn closing_a_request_clears_rebase_conflict_context() {
    let mut f = Fixture::new();
    f.seed(Phase::Rebasing, 1);
    f.observe(Observation::RebaseConflict {
        node: NodeId(0),
        generation: 10,
        head: f.repo.head.clone(),
        target: f.repo.base.clone(),
        files: vec!["README.md".into()],
    });
    f.observe(Observation::Steering {
        node: NodeId(0),
        command: tl3_kernel::Command::Stop,
    });
    assert!(f.controller.world().unwrap().nodes[0].phase.is_closed());
    assert!(
        f.controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .find(|row| row.table == "feedback")
            .unwrap()
            .value["conflict"]
            .is_null()
    );
}

#[test]
fn a_failed_step_persists_only_the_canonical_feedback_document() {
    let mut f = Fixture::new();
    f.seed(Phase::Making, 2);
    f.observe(Observation::WorkerFailed {
        node: NodeId(0),
        generation: 10,
        failure_class: "worker_exit".into(),
        failure_message: "failed while reproducing".into(),
    });
    let snapshot = f.controller.snapshot().unwrap();
    let document = snapshot
        .rows
        .iter()
        .find(|row| row.table == "feedback")
        .unwrap();
    assert_eq!(
        document.value["maker"][0]["report"],
        "failed while reproducing"
    );
    assert!(
        !snapshot.rows.iter().any(|row| {
            ["maker_findings", "findings", "rebase_conflicts"].contains(&row.table.as_str())
        }),
        "a step must store one representation of feedback"
    );
}

#[test]
fn malformed_feedback_on_closed_nodes_does_not_poison_later_steps() {
    {
        let mut f = Fixture::new();
        f.seed(Phase::Rebasing, 1);
        f.observe(Observation::RebaseConflict {
            node: NodeId(0),
            generation: 10,
            head: f.repo.head.clone(),
            target: f.repo.base.clone(),
            files: vec!["README.md".into()],
        });
        let snapshot = f.controller.snapshot().unwrap();
        let mut commit = Commit::new("malformed-feedback", json!(null), &snapshot);
        commit.rows.push(tl3_store::RowChange::Delete {
            table: "feedback".into(),
            id: "0".into(),
        });
        commit.rows.push(crate::put(
            "feedback",
            "0".into(),
            json!({"maker":"malformed", "conflict":{"head":"old"}}),
        ));
        f.controller.store.commit(&commit).unwrap();
        f.observe(Observation::Steering {
            node: NodeId(0),
            command: tl3_kernel::Command::Stop,
        });
        assert!(f.controller.world().unwrap().nodes[0].phase.is_closed());
        assert!(
            f.controller
                .snapshot()
                .unwrap()
                .rows
                .iter()
                .find(|row| row.table == "feedback")
                .unwrap()
                .value["conflict"]
                .is_null()
        );
        {
            let snapshot = f.controller.snapshot().unwrap();
            let document = snapshot
                .rows
                .iter()
                .find(|row| row.table == "feedback")
                .unwrap();
            assert!(document.value["conflict"].is_null());
            assert_eq!(document.value["maker"], "malformed");
        }
        for _ in 0..2 {
            f.observe(Observation::Steering {
                node: NodeId(0),
                command: tl3_kernel::Command::Resume,
            });
        }
    }
}

#[test]
fn postmortem_reads_canonical_planner_feedback() {
    let mut f = Fixture::new();
    f.seed(Phase::Planning { must_split: false }, 1);
    f.worker("planner", json!({"kind":"cant_meet", "report":"Need a smaller request", "files":["README.md"], "violations":[],
        "forecast":{"tokenizer":"o200k_base","brief_tokens":1,"file_tokens":{"README.md":0},"overhead_tokens":0,"tokens":1,"fit_tokens":100,"context_tokens":150,"fits":true}}));
    let snapshot = f.controller.snapshot().unwrap();
    let mut commit = Commit::new("feedback-postmortem", json!(null), &snapshot);
    commit.postmortems.push(
        crate::postmortems::owed(
            &snapshot,
            NodeId(0),
            "feedback".into(),
            crate::postmortems::Reason::RobAsked,
            None,
        )
        .unwrap(),
    );
    f.controller.store.commit(&commit).unwrap();
    let id = f.controller.store.owed_postmortem().unwrap().unwrap().id;
    let mut delivery = f.repo.delivery();
    delivery.output = crate::executor::Output::Comment {
        node: NodeId(0),
        kind: crate::executor::CommentKind::Postmortem,
    };
    delivery.steering = Some(crate::steering::Task::Postmortem {
        id,
        run_key: "feedback".into(),
    });
    let inputs = f
        .supervisor
        .build_request(
            &delivery,
            "postmortem",
            &mut Store::open(&f.config.store).unwrap(),
            &f.controller.snapshot().unwrap(),
        )
        .unwrap();
    assert_eq!(
        inputs["inputs"]["case"]["findings"],
        json!(["Need a smaller request"])
    );
}
