//! Q54 (#440): `limits.fail_fast` reaches the stored world at every open, so
//! an open request stops for TL3 review at its next retry. The stop spends no
//! credit, asks Rob nothing, posts the summary with the failing step and its
//! evidence, and leaves a one-line note on the operations issue.
mod support;

use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tl3_controller::{
    Bootstrap, Controller, LabelledIssue, Repository, adapters,
    executor::{
        CommentKind, Completion, Delivery, Executor, Observation, Output, Reconciliation, Upstream,
    },
    findings::{CiFailure, FailedTest},
};
use tl3_kernel::{EffectKind, Event, Notice, Outcome, Phase, Reply, ReviewStop, Role};
use tl3_store::{Commit, RowChange, Store};

const OPERATIONS: u64 = 81;

fn bootstrap(fail_fast: bool) -> Bootstrap {
    Bootstrap {
        operations: Some(tl3_config::OperationsIssue {
            owner: "RobTand".into(),
            name: "trustladderiii".into(),
            installation_id: 1,
            number: OPERATIONS,
        }),
        repos: vec![Repository {
            owner: "RobTand".into(),
            name: "tl3-sandbox".into(),
            installation: 1,
        }],
        rob: 1,
        slots: 1,
        pool: 1,
        fail_fast,
    }
}

fn labelled() -> LabelledIssue {
    LabelledIssue {
        delivery: "fail-fast".into(),
        owner: "RobTand".into(),
        repo: "tl3-sandbox".into(),
        installation: 1,
        number: 42,
        title: "Fix the sandbox build".into(),
        actor: 1,
        label: "tl3".into(),
        labels: Vec::new(),
    }
}

type Posted = Arc<Mutex<Vec<Delivery>>>;

/// The GitHub fake, keeping every delivery it posts.
struct GitHub {
    fake: adapters::github::Fake,
    posted: Posted,
}
impl Executor for GitHub {
    fn execute(&mut self, delivery: &Delivery) -> tl3_controller::Result<Completion> {
        self.posted.lock().unwrap().push(delivery.clone());
        if let Some(tl3_controller::steering::Task::Restart { .. }) = &delivery.steering {
            // As the real adapter does (Q49): read the issue, which a stop left open.
            let issue: tl3_github::Issue = serde_json::from_value(json!({
                "number": delivery.issue, "title": "Fix the sandbox build", "body": "Fix it.",
                "state": "open", "updated_at": "2026-11-02T00:00:00Z", "pull_request": null}))
            .unwrap();
            return Ok(Completion::confirmed(vec![Observation::LinkedRequest {
                repository: delivery.repository.clone(),
                issue,
                original: delivery.node(),
            }]));
        }
        self.fake.execute(delivery)
    }
    fn reconcile(&mut self, delivery: &Delivery) -> tl3_controller::Result<Reconciliation> {
        self.fake.reconcile(delivery)
    }
}

/// The worker fake, with every review run left running for the test to answer.
struct HeldReviews {
    fake: adapters::workers::Fake,
    review: Arc<Mutex<Option<Delivery>>>,
}
impl Executor for HeldReviews {
    fn execute(&mut self, delivery: &Delivery) -> tl3_controller::Result<Completion> {
        if matches!(
            delivery.output,
            Output::Effect {
                kind: EffectKind::StartRun(Role::Review),
                ..
            }
        ) {
            *self.review.lock().unwrap() = Some(delivery.clone());
            return Ok(Completion::confirmed(vec![]));
        }
        self.fake.execute(delivery)
    }
    fn reconcile(&mut self, delivery: &Delivery) -> tl3_controller::Result<Reconciliation> {
        if self
            .review
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|review| review.key == delivery.key)
        {
            return Ok(Reconciliation::Pending);
        }
        self.fake.reconcile(delivery)
    }
}

fn drain(controller: &mut Controller) {
    for _ in 0..400 {
        if !controller.drive_once().unwrap() {
            return;
        }
    }
    panic!("the controller never went idle");
}

fn summary(posted: &Posted) -> Delivery {
    posted
        .lock()
        .unwrap()
        .iter()
        .find(|d| {
            matches!(
                d.output,
                Output::Notice {
                    notice: Notice::Summary(_),
                    ..
                }
            )
        })
        .cloned()
        .expect("a closing summary")
}

fn operations_notes(posted: &Posted) -> Vec<Delivery> {
    posted
        .lock()
        .unwrap()
        .iter()
        .filter(|d| d.issue == OPERATIONS && d.repository.name == "trustladderiii")
        .cloned()
        .collect()
}

fn assert_operations_note(posted: &Posted, step: &str) {
    let notes = operations_notes(posted);
    assert_eq!(notes.len(), 1, "one note per stop: {notes:#?}");
    let text = notes[0].note.as_deref().expect("a code-written note");
    assert!(!text.contains('\n'), "a one-line note: {text}");
    assert!(
        text.contains("RobTand/tl3-sandbox#42") && text.contains("tl3_review"),
        "{text}"
    );
    assert!(text.contains(step), "{text}");
}

#[test]
fn an_open_request_stops_for_tl3_review_after_fail_fast_is_turned_on() {
    let mut database = support::Database::start();
    let settings = database.create("fail_fast_round_one");
    let root = tempfile::tempdir().unwrap();
    let fakes = root.path().join("fakes");
    let posted: Posted = Arc::default();
    let review = Arc::new(Mutex::new(None));
    let registry = || {
        let mut registry = adapters::simulation(&fakes).unwrap();
        registry.register(
            Upstream::Workers,
            HeldReviews {
                fake: adapters::workers::Fake::new(&fakes),
                review: review.clone(),
            },
        );
        registry.register(
            Upstream::GitHub,
            GitHub {
                fake: adapters::github::Fake::new(&fakes),
                posted: posted.clone(),
            },
        );
        registry
    };

    // Filed while fail_fast is off: it waits for its first review.
    let mut c = Controller::open(
        Store::open(&settings).unwrap(),
        registry(),
        bootstrap(false),
    )
    .unwrap();
    c.intake(labelled()).unwrap();
    drain(&mut c);
    let world = c.world().unwrap();
    assert!(!world.fail_fast);
    assert_eq!(world.nodes[0].phase, Phase::Reviewing);
    let spent = world.nodes[0].credit.spent;
    drop(c);

    // tl3d starts again with fail_fast on: the stored world takes it at open,
    // and a second open with the same setting writes nothing.
    let mut c =
        Controller::open(Store::open(&settings).unwrap(), registry(), bootstrap(true)).unwrap();
    assert!(c.world().unwrap().fail_fast, "open reconciles fail_fast");
    let reconciled = c.snapshot().unwrap();
    let journal = Store::open(&settings)
        .unwrap()
        .events_after(tl3_store::Revision::ZERO, 1000)
        .unwrap();
    assert!(
        journal
            .iter()
            .any(|e| e.key.starts_with("controller:gates:")
                && e.payload["fail_fast"] == json!({"stored": false, "configured": true})),
        "the change is journalled"
    );
    drop(c);
    let mut c =
        Controller::open(Store::open(&settings).unwrap(), registry(), bootstrap(true)).unwrap();
    assert_eq!(
        c.snapshot().unwrap(),
        reconciled,
        "reopening writes nothing"
    );

    // Round 0 fails review: the free fix round is unchanged.
    let changes = |c: &mut Controller, name: &str| {
        let d = review.lock().unwrap().take().expect("a review run");
        c.observe(
            name,
            Observation::Kernel(Event::Reply {
                node: d.node(),
                generation: d.generation,
                reply: Reply::ChangesRequested,
            }),
        )
        .unwrap();
        drain(c);
    };
    changes(&mut c, "round-0-review");
    let n = c.world().unwrap().nodes[0];
    assert_eq!((n.phase, n.attempt, n.round), (Phase::Reviewing, 1, 1));

    // Round 1 fails review: the request stops for TL3 review.
    changes(&mut c, "round-1-review");
    let n = c.world().unwrap().nodes[0];
    assert_eq!(n.phase, Phase::Closed(Outcome::Stopped));
    assert_eq!(n.review_stop, Some(ReviewStop::ChangesRequested));
    assert_eq!(n.credit.spent, spent, "no credit spent");
    assert_eq!((n.attempt, n.round), (1, 1), "no new attempt");
    assert!(n.question.is_none());
    assert!(
        !posted.lock().unwrap().iter().any(|d| matches!(
            d.output,
            Output::Notice {
                notice: Notice::Ask { .. },
                ..
            }
        )),
        "Rob is asked nothing"
    );

    let summary = summary(&posted);
    assert_eq!(summary.issue, 42);
    let request = tl3_controller::adapters::github::writer_input(
        &summary,
        Some(Notice::Summary(Outcome::Stopped)),
        "Fix the sandbox build",
        "Fix the sandbox build",
    )
    .unwrap();
    let outcome = request.facts["outcome"].as_str().unwrap();
    assert!(
        outcome.contains("TL3") && outcome.contains("review"),
        "{outcome}"
    );
    assert_operations_note(&posted, "changes_requested");
    let owed = Store::open(&settings).unwrap().postmortems("0").unwrap();
    assert!(
        owed.iter().any(|p| p.reason == "needed_fix"),
        "a postmortem is owed as needed_fix: {owed:?}"
    );
}

#[test]
fn plan_cant_meet_stops_with_the_planners_report_and_no_rerun() {
    let mut database = support::Database::start();
    let settings = database.create("fail_fast_plan");
    let root = tempfile::tempdir().unwrap();
    let fakes = root.path().join("fakes");
    let posted: Posted = Arc::default();
    let plans = Arc::new(Mutex::new(0));
    let counted = plans.clone();
    let mut registry = adapters::simulation(&fakes).unwrap();
    registry.register(
        Upstream::Workers,
        support::workers::Workers::new(
            None,
            adapters::workers::Fake::new(&fakes),
            vec![],
            move |_, completion: &mut Completion| {
                for observation in &mut completion.observations {
                    if let Observation::Plan {
                        node, generation, ..
                    } = observation
                    {
                        *counted.lock().unwrap() += 1;
                        let forecast = serde_json::json!({"tokenizer":"o200k_base", "brief_tokens":1,
                            "file_tokens":{"value.py":0}, "overhead_tokens":0, "tokens":1,
                            "fit_tokens":100, "context_tokens":150, "fits":true});
                        *observation = Observation::WorkerReply {
                            node: *node,
                            generation: *generation,
                            role: "planner".into(),
                            reply: serde_json::json!({"kind":"cant_meet", "files":["value.py"],
                                "report":"The goal needs a schema change outside the plan.",
                                "violations":[], "forecast":forecast}),
                            model: None,
                        };
                    }
                }
            },
        ),
    );
    registry.register(
        Upstream::GitHub,
        GitHub {
            fake: adapters::github::Fake::new(&fakes),
            posted: posted.clone(),
        },
    );
    let mut c =
        Controller::open(Store::open(&settings).unwrap(), registry, bootstrap(true)).unwrap();
    c.intake(labelled()).unwrap();
    drain(&mut c);
    let n = c.world().unwrap().nodes[0];
    assert_eq!(n.phase, Phase::Closed(Outcome::Stopped));
    assert_eq!(n.review_stop, Some(ReviewStop::PlanCantMeet));
    assert_eq!(*plans.lock().unwrap(), 1, "the planner never re-runs");
    let summary = summary(&posted);
    let request = tl3_controller::adapters::github::writer_input(
        &summary,
        Some(Notice::Summary(Outcome::Stopped)),
        "Fix the sandbox build",
        "Fix the sandbox build",
    )
    .unwrap();
    assert_eq!(request.facts["findings"]["source"], "planner");
    assert_eq!(
        request.facts["findings"]["report"],
        "The goal needs a schema change outside the plan."
    );
    let rendered = tl3_controller::adapters::workers::writer::checked_draft(
        &mut tl3_controller::adapters::workers::writer::FakeWriter,
        "fail-fast-summary",
        request,
        &Default::default(),
    )
    .unwrap();
    assert!(
        rendered.contains("The goal needs a schema change outside the plan."),
        "the deterministic summary must retain the recorded fail-fast report: {rendered}"
    );
    assert!(
        !rendered.contains("The reason for stopping was not captured."),
        "the recorded review stop must supply the summary's stop reason: {rendered}"
    );
    assert_operations_note(&posted, "plan_cant_meet");
    let owed = Store::open(&settings).unwrap().postmortems("0").unwrap();
    assert!(
        owed.iter().any(|p| p.reason == "needed_fix"),
        "a postmortem is owed as needed_fix: {owed:?}"
    );
}

/// Each failing step's own worker documents reach `review_stops/<node>`.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Source {
    Checks,
    Review,
    Ci,
    Maker,
    Budget,
    Planner,
}

struct Stopped {
    node: tl3_kernel::Node,
    record: Value,
    posted: Posted,
    controller: Controller,
    settings: tl3_config::Store,
    _database: support::Database,
    _root: tempfile::TempDir,
}

fn put(store: &mut Store, table: &str, value: Value) {
    let snapshot = store.load().unwrap();
    let mut commit = Commit::new(
        format!("fixture-{table}"),
        json!({"fixture": table}),
        &snapshot,
    );
    commit.rows.push(RowChange::Put {
        table: table.into(),
        id: "0".into(),
        value,
    });
    store.commit(&commit).unwrap();
}

fn stop_with(source: Source, operations: bool) -> Stopped {
    let mut database = support::Database::start();
    let settings = database.create(&format!("evidence_{source:?}").to_lowercase());
    let root = tempfile::tempdir().unwrap();
    let fakes = root.path().join("fakes");
    let posted: Posted = Arc::default();
    let mut registry = adapters::simulation(&fakes).unwrap();
    registry.register(
        Upstream::Workers,
        support::workers::Workers::new(
            None,
            adapters::workers::Fake::new(&fakes),
            vec![],
            move |d: &Delivery, completion: &mut Completion| {
                let Output::Effect {
                    kind: EffectKind::StartRun(role),
                    ..
                } = d.output
                else {
                    return;
                };
                let (role, reply) = match (source, role) {
                    (Source::Review, Role::Review) => ("reviewer", json!({"verdict": "changes",
                        "blocking_findings": [{"citation": {"kind": "goal", "id": "goal-1"},
                            "problem": "The total is wrong.", "evidence": "value.py returns 3, not 4."}],
                        "findings": []})),
                    (Source::Maker, Role::Make) => ("maker", json!({"kind": "cant_meet",
                        "report": "The goal needs a schema change outside the plan."})),
                    (Source::Budget, Role::Make) => ("maker", json!({"kind": "cant_meet",
                        "report": "Final verification remains unfinished.",
                        "budget_exhaustion": {
                            "kind": "execution_budget_exhausted", "dimension": "no_progress",
                            "no_progress_window": 16, "no_progress_turns": 16, "last_progress_kind": "edit",
                            "tool_turns": {"used": 83, "allowed": 512},
                            "input_tokens": {"used": 123456, "allowed": 6000000},
                            "operation": {"node": 0, "generation": 0, "kind": "make"},
                            "preserved_work": {"head": "a".repeat(40), "tree": "b".repeat(40)},
                            "unfinished_obligations": ["CHANGELOG.md update", "final regression checks"]
                        }})),
                    (Source::Planner, Role::Plan) => ("planner", json!({"kind": "leaf",
                        "files": ["value.py"],
                        "checks": [{"kind": "regression", "node_ids": ["test_value.py::test_total"], "goal_ids": [0]}],
                        "no_design_questions": true, "lands_on_its_own": true,
                        "violations": ["The plan declares a protected path: .github/workflows/ci.yml"],
                        "forecast": {"tokenizer": "o200k_base", "brief_tokens": 1, "file_tokens": {"value.py": 0},
                            "overhead_tokens": 0, "tokens": 1, "fit_tokens": 100, "context_tokens": 150, "fits": true}})),
                    _ => return,
                };
                completion.observations = vec![Observation::WorkerReply {
                    node: d.node(),
                    generation: d.generation,
                    role: role.into(),
                    reply,
                    model: None,
                }];
            },
        )
        .transform_reconciled(),
    );
    registry.register(
        Upstream::PrismaBuild,
        support::workers::Workers::new(
            None,
            adapters::prismabuild::Fake::new(&fakes),
            vec![],
            move |d: &Delivery, completion: &mut Completion| {
                if source == Source::Checks
                    && matches!(
                        d.output,
                        Output::Effect {
                            kind: EffectKind::SubmitChecks,
                            ..
                        }
                    )
                {
                    completion.observations = vec![Observation::Checks {
                        node: d.node(),
                        generation: d.generation,
                        action_key: "fixture-checks".into(),
                        base_sha: "0".repeat(40),
                        sha: d.head_sha.clone().unwrap(),
                        changed_paths: vec![],
                        failed_tests: vec![FailedTest {
                            test: "tests/test_value.py::test_total".into(),
                            message: "assert 3 == 4".into(),
                        }],
                        report: true,
                        pass: false,
                        results: vec![],
                    }];
                }
            },
        )
        .transform_reconciled(),
    );
    registry.register(
        Upstream::Mergify,
        support::workers::Workers::new(
            None,
            adapters::mergify::Fake::new(&fakes),
            vec![],
            move |d: &Delivery, completion: &mut Completion| {
                if source == Source::Ci {
                    completion.observations = vec![Observation::CiFailed {
                        node: d.node(),
                        infra: false,
                        checks: vec![CiFailure {
                            name: "unit tests".into(),
                            summary: "test_total failed: assert 3 == 4".into(),
                            url: None,
                        }],
                    }];
                }
            },
        )
        .transform_reconciled(),
    );
    registry.register(
        Upstream::GitHub,
        GitHub {
            fake: adapters::github::Fake::new(&fakes),
            posted: posted.clone(),
        },
    );
    let mut boot = bootstrap(true);
    if !operations {
        boot.operations = None;
    }
    let mut controller = Controller::open(Store::open(&settings).unwrap(), registry, boot).unwrap();
    // Review reads its criteria from the request's materials and goals.
    let mut store = Store::open(&settings).unwrap();
    store
        .put_worker_materials("0", &json!({"standing_rules": []}))
        .unwrap();
    put(
        &mut store,
        "goals",
        json!([{"description": "Return the order total.", "verification": "test"}]),
    );
    controller.intake(labelled()).unwrap();
    drain(&mut controller);
    let node = controller.world().unwrap().nodes[0];
    let record = controller
        .snapshot()
        .unwrap()
        .rows
        .into_iter()
        .find(|r| r.table == "review_stops" && r.id == "0")
        .map(|r| r.value)
        .unwrap_or(Value::Null);
    Stopped {
        node,
        record,
        posted,
        controller,
        settings,
        _database: database,
        _root: root,
    }
}

#[test]
fn each_failing_step_records_its_own_evidence() {
    for (source, stop, step, from, text) in [
        (
            Source::Checks,
            ReviewStop::ChecksFailed,
            "checks_failed",
            "checks_and_review",
            "assert 3 == 4",
        ),
        (
            Source::Review,
            ReviewStop::ChangesRequested,
            "changes_requested",
            "checks_and_review",
            "The total is wrong.",
        ),
        (
            Source::Ci,
            ReviewStop::CiFailed,
            "ci_failed",
            "checks_and_review",
            "test_total failed",
        ),
        (
            Source::Maker,
            ReviewStop::MakerCantMeet,
            "maker_cant_meet",
            "maker",
            "schema change outside the plan",
        ),
        (
            Source::Planner,
            ReviewStop::PlanCantMeet,
            "plan_cant_meet",
            "planner",
            "protected path",
        ),
    ] {
        let stopped = stop_with(source, true);
        assert_eq!(
            stopped.node.phase,
            Phase::Closed(Outcome::Stopped),
            "{source:?}"
        );
        assert_eq!(stopped.node.review_stop, Some(stop), "{source:?}");
        assert_eq!(
            stopped.record["step"], step,
            "{source:?}: {}",
            stopped.record
        );
        assert_eq!(
            stopped.record["findings"]["source"], from,
            "{source:?}: {}",
            stopped.record
        );
        let report = stopped.record["findings"]["report"]
            .as_str()
            .unwrap_or_default();
        assert!(report.contains(text), "{source:?}: {report}");
        assert_operations_note(&stopped.posted, step);
    }
}

#[test]
fn maker_budget_exhaustion_reaches_the_stop_report_and_operations_notice() {
    let mut stopped = stop_with(Source::Budget, true);
    assert_eq!(stopped.node.phase, Phase::Closed(Outcome::Stopped));
    assert_eq!(stopped.node.review_stop, Some(ReviewStop::MakerCantMeet));
    let diagnostic = &stopped.record["findings"]["budget_exhaustion"];
    assert_eq!(diagnostic["kind"], "execution_budget_exhausted");
    assert_eq!(diagnostic["tool_turns"]["used"], 83);
    assert_eq!(diagnostic["preserved_work"]["tree"], "b".repeat(40));
    assert!(stopped.record["text"].as_str().unwrap().contains("budget"));
    let notes = operations_notes(&stopped.posted);
    assert_eq!(notes.len(), 1);
    let text = notes[0].note.as_deref().unwrap();
    assert!(text.contains("execution_budget_exhausted"), "{text}");
    assert!(
        text.contains("no-progress turns 16/16") && text.contains("last progress edit"),
        "{text}"
    );
    assert!(
        text.contains("83/512") && text.contains("123456/6000000"),
        "{text}"
    );
    assert!(
        text.contains("CHANGELOG.md update") && text.contains("final regression checks"),
        "{text}"
    );
    assert!(
        !text.contains("Restart it"),
        "exhaustion must not suggest a blind restart: {text}"
    );
    let snapshot = stopped.controller.snapshot().unwrap();
    assert!(!snapshot.rows.iter().any(|r| r.table == "questions"));
    assert_operations_note(&stopped.posted, "maker_cant_meet");
}

#[test]
fn without_an_operations_issue_no_note_is_posted() {
    let stopped = stop_with(Source::Maker, false);
    assert_eq!(stopped.node.review_stop, Some(ReviewStop::MakerCantMeet));
    let notes: Vec<_> = stopped
        .posted
        .lock()
        .unwrap()
        .iter()
        .filter(|d| {
            matches!(
                d.output,
                Output::Comment {
                    kind: CommentKind::OperationsNote,
                    ..
                }
            )
        })
        .cloned()
        .collect();
    assert!(
        notes.is_empty(),
        "the issue summary already says it stopped: {notes:#?}"
    );
}

#[test]
fn a_restart_after_a_review_stop_files_a_new_request_and_leaves_the_stop_alone() {
    let mut stopped = stop_with(Source::Maker, true);
    let summary = summary(&stopped.posted);
    assert_eq!(summary.issue, 42);
    assert!(
        matches!(
            summary.output,
            Output::Notice {
                notice: Notice::Summary(Outcome::Stopped),
                ..
            }
        ),
        "a stop posts a summary without closing the issue"
    );
    assert!(
        tl3_store::ConsoleCommands::open(&stopped.settings)
            .unwrap()
            .submit(
                "00000000-0000-4000-8000-000000004400",
                0,
                &json!({"kind": "restart"})
            )
            .unwrap()
    );
    tl3_controller::ingress::drain(&mut stopped.controller).unwrap();
    drain(&mut stopped.controller);
    let world = stopped.controller.world().unwrap();
    assert_eq!(world.nodes.len(), 2, "the restart files a new request");
    assert_eq!(
        world.nodes[0], stopped.node,
        "the stopped node is left alone"
    );
    let snapshot = stopped.controller.snapshot().unwrap();
    let binding = |id: &str| {
        snapshot
            .rows
            .iter()
            .find(|r| r.table == "requests" && r.id == id)
            .map(|r| r.value["issue"].clone())
    };
    assert_eq!(
        binding("1"),
        binding("0"),
        "the new request is on the same, open issue"
    );
}

#[test]
fn a_stored_world_from_before_fail_fast_decodes_with_it_off() {
    let (world, _) = tl3_kernel::step(
        tl3_kernel::World::new(&[1], 1),
        Event::NewRequest {
            repo: tl3_kernel::RepoId(0),
            priority: tl3_kernel::Priority::Unranked,
        },
    );
    let mut stored = serde_json::to_value(&world).unwrap();
    stored.as_object_mut().unwrap().remove("fail_fast").unwrap();
    for node in stored["nodes"].as_array_mut().unwrap() {
        node.as_object_mut().unwrap().remove("review_stop").unwrap();
    }
    let decoded: tl3_kernel::World = serde_json::from_value(stored).unwrap();
    assert!(!decoded.fail_fast);
    assert_eq!(decoded.nodes[0].review_stop, None);
    assert_eq!(decoded, world);
}

/// A request-123-shaped scripted protocol flow; the real harness's Q55 negatives
/// are exercised separately in test_forced_reproduction.py.
#[test]
fn stage_owned_handover_keeps_fail_fast_and_independent_checking() {
    struct Workers {
        fake: adapters::workers::Fake,
        settings: tl3_config::Store,
        mode: &'static str,
    }
    fn goals() -> Value {
        json!([
            {"description":"Repair the ordinary deep-path test.","verification":"test"},
            {"description":"Keep changes inside the declared test file.","verification":"review"},
            {"description":"Independent Q55 forced-base proof.","verification":"test","stage":"checking"},
            {"description":"Stable branch and non-draft PR with before/after proof.","verification":"review","stage":"publishing",
             "publication":{"non_draft":true,"body_lines":["Change type: test"],"proof":true}}
        ])
    }
    fn forcing() -> Value {
        json!({"symptom":"Old test assumes residency", "environment":{},
            "patches":[{"target":"example.RESIDENCY","value":false}]})
    }
    impl Executor for Workers {
        fn execute(&mut self, d: &Delivery) -> tl3_controller::Result<Completion> {
            let reply = match d.output {
                Output::Effect {
                    kind: EffectKind::StartRun(Role::Ingest),
                    ..
                } => Some((
                    "ingest",
                    json!({
                    "kind":"brief","brief":"Repair the test; scope remains maker-owned; publish the proof.",
                    "goals":goals(),"memory_ids":[],"files":["test_deep.py"],
                    "forecast":{"tokenizer":"o200k_base","brief_tokens":1,"file_tokens":{"test_deep.py":0},
                        "overhead_tokens":0,"tokens":1,"fit_tokens":100,"context_tokens":150,"fits":true}}),
                )),
                Output::Effect {
                    kind: EffectKind::StartRun(Role::Plan),
                    ..
                } => {
                    return Ok(Completion::confirmed(vec![Observation::Plan {
                        node: d.node(),
                        generation: d.generation,
                        paths: vec!["test_deep.py".into()],
                        fits: true,
                    }]));
                }
                Output::Effect {
                    kind: EffectKind::StartRun(Role::Make),
                    ..
                } => {
                    let mut supplied = goals();
                    for (i, g) in supplied.as_array_mut().unwrap().iter_mut().enumerate() {
                        g["id"] = json!(format!("goal-{}", i + 1));
                    }
                    Store::open(&self.settings)?.put_worker_materials(
                        &d.node().0.to_string(),
                        &json!({"maker":{"goals":supplied}}),
                    )?;
                    let checks = json!([
                        {"goal_id":"goal-1","status":if self.mode=="unmet-maker" {"not_met"}else{"met"},
                            "evidence":"Normal head passed; development base pass is intentionally unforced.","why":"Repair is incomplete."},
                        {"goal_id":"goal-2","status":"met","evidence":"Only the declared test changed."},
                        {"goal_id":"goal-3","status":"pending","why":"Controller Checking owns forced proof."},
                        {"goal_id":"goal-4","status":"pending","why":"Publishing owns branch and PR actions."}]);
                    let mut checks = checks;
                    checks[0]
                        .as_object_mut()
                        .unwrap()
                        .remove(if self.mode == "unmet-maker" {
                            "evidence"
                        } else {
                            "why"
                        });
                    let plan = adapters::prismabuild::CheckRun {
                        repository: "/fixture".into(),
                        base_sha: tl3_worktrees::CommitId::new("a".repeat(40))?,
                        reviewed_sha: None,
                        reproducing_unavailable: None,
                        selected_failures: Vec::new(),
                        plan: vec![adapters::prismabuild::Check {
                            name: "forced-deep-path".into(),
                            kind: adapters::prismabuild::Kind::Demonstrating,
                            tests: vec!["test_deep.py::test_deep_path".into()],
                            forcing: Some(forcing()),
                            proof: None,
                            declared_symptom: None,
                        }],
                    };
                    return Ok(Completion::confirmed(vec![
                        Observation::CheckPlan {
                            node: d.node(),
                            sha: "b".repeat(40),
                            run: plan,
                        },
                        Observation::WorkerReply {
                            node: d.node(),
                            generation: d.generation,
                            role: "maker".into(),
                            reply: json!({"kind":"done","head":"b".repeat(40),"self_check":checks}),
                            model: None,
                        },
                    ]));
                }
                Output::Effect {
                    kind: EffectKind::StartRun(Role::Review),
                    ..
                } => Some((
                    "reviewer",
                    json!({
                    "verdict":"approve","head_sha":d.head_sha,"blocking_findings":[],"findings":[]}),
                )),
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
                self.fake.execute(d)
            }
        }
        fn reconcile(&mut self, d: &Delivery) -> tl3_controller::Result<Reconciliation> {
            self.fake.reconcile(d)
        }
    }
    struct Checking {
        mode: &'static str,
        fake: adapters::prismabuild::Fake,
    }
    impl Executor for Checking {
        fn execute(&mut self, d: &Delivery) -> tl3_controller::Result<Completion> {
            if !matches!(
                d.output,
                Output::Effect {
                    kind: EffectKind::SubmitChecks,
                    ..
                }
            ) {
                return self.fake.execute(d);
            }
            assert_eq!(
                d.checks.as_ref().unwrap().plan[0].forcing,
                Some(forcing()),
                "forcing reaches independent Checking unchanged"
            );
            let passed = !matches!(self.mode, "base-passes" | "forced-head-fails");
            Ok(Completion::confirmed(vec![Observation::Checks {
                node: d.node(),
                generation: d.generation,
                action_key: "c".repeat(64),
                base_sha: "a".repeat(40),
                sha: d.head_sha.clone().unwrap(),
                changed_paths: vec![
                    if self.mode == "outside-scope" {
                        "unrelated.py"
                    } else {
                        "test_deep.py"
                    }
                    .into(),
                ],
                failed_tests: vec![],
                report: true,
                pass: passed,
                results: vec![json!({
                    "name":"forced-deep-path","kind":"demonstrating","forcing":forcing(),
                    "verdict":if passed {"pass"}else{"fail"},"tests":[{"test":"test_deep.py::test_deep_path",
                        "base":if self.mode=="base-passes" {"passed"}else{"assertion"},
                        "head_forced":if self.mode=="forced-head-fails" {"assertion"}else{"passed"},"head":"passed","passed":passed}]})],
            }]))
        }
        fn reconcile(&mut self, d: &Delivery) -> tl3_controller::Result<Reconciliation> {
            self.fake.reconcile(d)
        }
    }
    let mut database = support::Database::start();
    for mode in [
        "pass",
        "unmet-maker",
        "outside-scope",
        "base-passes",
        "forced-head-fails",
    ] {
        let settings = database.create(&format!("stage_{}", mode.replace('-', "_")));
        let root = tempfile::tempdir().unwrap();
        let fakes = root.path().join("fakes");
        let posted: Posted = Arc::default();
        let mut registry = adapters::simulation(&fakes).unwrap();
        registry.register(
            Upstream::Workers,
            Workers {
                fake: adapters::workers::Fake::new(&fakes),
                settings: settings.clone(),
                mode,
            },
        );
        registry.register(
            Upstream::PrismaBuild,
            Checking {
                mode,
                fake: adapters::prismabuild::Fake::new(&fakes),
            },
        );
        registry.register(
            Upstream::GitHub,
            GitHub {
                fake: adapters::github::Fake::new(&fakes),
                posted: posted.clone(),
            },
        );
        let mut controller =
            Controller::open(Store::open(&settings).unwrap(), registry, bootstrap(true)).unwrap();
        controller.intake(labelled()).unwrap();
        drain(&mut controller);
        let deliveries = posted.lock().unwrap();
        let publication = deliveries.iter().find(|d| {
            matches!(
                d.output,
                Output::Effect {
                    kind: EffectKind::OpenPr,
                    ..
                }
            )
        });
        if mode == "pass" {
            assert_eq!(
                controller.world().unwrap().nodes[0].phase,
                Phase::Closed(Outcome::Merged)
            );
            let facts = publication
                .expect("honest handover reaches Publishing")
                .evidence
                .as_ref()
                .unwrap();
            assert_eq!(facts.goals.len(), 4);
            assert_eq!(
                facts.maker_handover.as_ref().unwrap()["self_check"][2]["status"],
                "pending"
            );
            assert_eq!(
                facts.checks[0]["results"][0]["tests"][0]["base"],
                "assertion"
            );
            assert_eq!(facts.reviews[0]["verdict"], "approve");
        } else {
            assert!(
                publication.is_none(),
                "{mode} must fail at its owning stage"
            );
            assert!(
                deliveries.iter().any(|d| matches!(
                    d.output,
                    Output::Notice {
                        notice: Notice::Summary(Outcome::Stopped),
                        ..
                    }
                )),
                "fail_fast still stops {mode}"
            );
        }
    }
    database.stop();
}

#[test]
fn t629_local_body_defect_stops_for_tl3_review_without_owner_question() {
    struct BodyRefusal {
        fake: adapters::github::Fake,
        posted: Posted,
    }
    impl Executor for BodyRefusal {
        fn execute(&mut self, delivery: &Delivery) -> tl3_controller::Result<Completion> {
            self.posted.lock().unwrap().push(delivery.clone());
            if matches!(
                delivery.output,
                Output::Effect {
                    kind: EffectKind::OpenPr,
                    ..
                }
            ) {
                tl3_controller::pr_body::validate(
                    "Change type: test\nChange type: fix\nprivate synthetic-secret body",
                    &[tl3_config::BodyLine {
                        label: "Change type".into(),
                        values: vec!["test".into(), "fix".into()],
                        default: "test".into(),
                    }],
                )?;
                panic!("conflicting body must be refused");
            }
            self.fake.execute(delivery)
        }
        fn reconcile(&mut self, delivery: &Delivery) -> tl3_controller::Result<Reconciliation> {
            self.fake.reconcile(delivery)
        }
    }
    let mut database = support::Database::start();
    let settings = database.create("t629_body_refusal");
    let root = tempfile::tempdir().unwrap();
    let posted = Arc::new(Mutex::new(Vec::new()));
    let mut registry = adapters::simulation(root.path()).unwrap();
    registry.register(
        Upstream::GitHub,
        BodyRefusal {
            fake: adapters::github::Fake::new(root.path()),
            posted: posted.clone(),
        },
    );
    let mut controller =
        Controller::open(Store::open(&settings).unwrap(), registry, bootstrap(true)).unwrap();
    controller.intake(labelled()).unwrap();
    drain(&mut controller);
    let node = controller.world().unwrap().nodes[0];
    assert_eq!(
        node.phase,
        Phase::Closed(Outcome::Stopped),
        "TL3's body refusal must use its review stop"
    );
    assert!(node.question.is_none());
    assert_eq!(node.credit.spent, 0);
    let posted = posted.lock().unwrap();
    assert!(
        !posted.iter().any(|d| matches!(
            d.output,
            Output::Notice {
                notice: Notice::Ask { .. },
                ..
            }
        )),
        "no owner question may be dispatched"
    );
    assert_eq!(
        posted
            .iter()
            .filter(|d| matches!(
                d.output,
                Output::Effect {
                    kind: EffectKind::OpenPr,
                    ..
                }
            ))
            .count(),
        1
    );
    let note = posted
        .iter()
        .find(|d| {
            d.issue == OPERATIONS
                && d.note
                    .as_deref()
                    .is_some_and(|text| text.contains("tl3_review"))
        })
        .expect("operations review note");
    for required in [
        "pr_policy",
        "Change type",
        "expected_count",
        "found_count",
        "conflicting",
    ] {
        assert!(note.note.as_ref().unwrap().contains(required));
    }
    assert!(!format!("{posted:?}").contains("synthetic-secret"));
    let snapshot = controller.snapshot().unwrap();
    assert!(
        snapshot
            .rows
            .iter()
            .any(|r| r.table == "publication_failures" && r.value["class"] == "pr_policy")
    );
    assert!(
        !snapshot
            .rows
            .iter()
            .any(|r| r.table == "incidents" && r.value["open"] == true)
    );
}
