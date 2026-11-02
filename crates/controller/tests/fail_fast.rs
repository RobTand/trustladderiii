//! Q54 (#440): `limits.fail_fast` reaches the stored world at every open, so
//! an open request stops for TL3 review at its next retry. The stop spends no
//! credit, asks Rob nothing, posts the summary with the failing step and its
//! evidence, and leaves a one-line note on the operations issue.
mod support;

use std::sync::{Arc, Mutex};
use tl3_controller::{
    Bootstrap, Controller, LabelledIssue, Repository, adapters,
    executor::{Completion, Delivery, Executor, Observation, Output, Reconciliation, Upstream},
};
use tl3_kernel::{EffectKind, Event, Notice, Outcome, Phase, Reply, ReviewStop, Role};
use tl3_store::Store;

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
    assert_operations_note(&posted, "plan_cant_meet");
}
