//! #632: recovery is effective without a controller restart.
#[path = "../../store/tests/support/mod.rs"]
mod support;

use serde_json::json;
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};
use tl3_controller::{
    Bootstrap, Controller, LabelledIssue, PostmortemPace, Repository, Result, adapters,
    executor::{Completion, Delivery, Executor, Look, Observation, Reconciliation, Upstream},
    status,
};
use tl3_store::{Reader, Store};

const PLANTED: [&str; 3] = [
    "sk-planted403abcdefghijklmnop",
    "ghp_planted403abcdefghijklmnop",
    "0123456789abcdef0123456789abcdef01234567",
];

fn reply(recommendations: usize) -> serde_json::Value {
    let planted = format!(
        "The maker printed {} with token={} and {}.",
        PLANTED[0], PLANTED[1], PLANTED[2]
    );
    json!({
        "postmortem": {
            "summary": format!("The maker needed a fix round. {planted}"),
            "cause": "tl3",
            "went_wrong": "The maker skipped a planned check.",
            "timeline": [{"at": "Making", "event": "The maker ran."}],
            "process_failures": [{"phase": "maker", "kind": "prompt or skill defect",
                "what": "It skipped a check.", "evidence": "The review."}],
            "recommendations": (0..recommendations).map(|n| json!({
                "phase": "maker", "kind": "prompt", "target": "maker", "operation": "replace",
                "before": "Run the tests.", "after": format!("Run check {n} first."),
                "evidence": "The review.", "effect": "Fewer fix rounds."})).collect::<Vec<_>>(),
            "notes": planted.clone(),
            "lessons": [format!("Run every planned check. {planted}")]
        },
        "grades": [{"dimension": "correctness", "grade": 4, "rationale": "The review.",
            "judge_model": "claude-sonnet-5-5", "assessment_id": "a-1"}],
        "assessment_ids": ["a-1"],
        "usage": {"traces": 2, "input_tokens": 1000, "output_tokens": 40,
            "cache_read_input_tokens": 600, "cache_creation_input_tokens": 100},
        "models": {"postmortem": "claude-opus-5-5", "grades": "claude-sonnet-5-5"}
    })
}

#[derive(Default)]
struct Judge {
    /// Launches of each postmortem delivery.
    launched: BTreeMap<String, u32>,
    finished: bool,
    hold_makers: bool,
    permanent: bool,
    reply: Option<serde_json::Value>,
    /// A finished run fails with this class instead of replying.
    failing: Option<&'static str>,
    /// A running postmortem reads as alive but never moves.
    stalled: bool,
    /// Stalled runs the controller stopped.
    stopped: u32,
}

/// The simulated workers, with postmortems that finish only when told to,
/// like a judge still running when tl3d stops.
struct Workers {
    fake: adapters::workers::Fake,
    judge: Rc<RefCell<Judge>>,
}
impl Executor for Workers {
    fn execute(&mut self, delivery: &Delivery) -> Result<Completion> {
        if !delivery.is_postmortem() {
            if self.judge.borrow().hold_makers
                && matches!(
                    delivery.output,
                    tl3_controller::executor::Output::Effect {
                        kind: tl3_kernel::EffectKind::StartRun(tl3_kernel::Role::Make),
                        ..
                    }
                )
            {
                return Ok(Completion::confirmed(vec![]));
            }
            return self.fake.execute(delivery);
        }
        let mut judge = self.judge.borrow_mut();
        *judge.launched.entry(delivery.key.clone()).or_default() += 1;
        if judge.permanent {
            return Ok(Completion::permanent("fixture"));
        }
        Ok(Completion::confirmed(vec![]))
    }
    fn reconcile(&mut self, delivery: &Delivery) -> Result<Reconciliation> {
        if !delivery.is_postmortem() {
            if self.judge.borrow().hold_makers
                && matches!(
                    delivery.output,
                    tl3_controller::executor::Output::Effect {
                        kind: tl3_kernel::EffectKind::StartRun(tl3_kernel::Role::Make),
                        ..
                    }
                )
            {
                return Ok(Reconciliation::Pending);
            }
            return self.fake.reconcile(delivery);
        }
        let judge = self.judge.borrow();
        if !judge.finished {
            return Ok(Reconciliation::Pending);
        }
        if let Some(failure) = judge.failing {
            return Ok(Reconciliation::Found(Box::new(Completion::confirmed(
                vec![Observation::Postmortem {
                    key: delivery.key.clone(),
                    reply: None,
                    failure: Some(failure.into()),
                }],
            ))));
        }
        Ok(Reconciliation::Found(Box::new(Completion::confirmed(
            vec![Observation::Postmortem {
                key: delivery.key.clone(),
                reply: Some(judge.reply.clone().unwrap_or_else(|| reply(2))),
                failure: None,
            }],
        ))))
    }
    fn look(&mut self, delivery: &Delivery) -> Result<Look> {
        if delivery.is_postmortem() && self.judge.borrow().stalled {
            return Ok(Look::Alive {
                progress: "stuck".into(),
            });
        }
        Ok(match self.reconcile(delivery)? {
            Reconciliation::Found(completion) => Look::Found(completion),
            Reconciliation::Absent => Look::Absent,
            Reconciliation::Pending => Look::Pending,
        })
    }
    /// The worker adapter stops a stalled run, and its record reads as a
    /// failed worker.
    fn stop_stalled(&mut self, delivery: &Delivery) -> Result<Completion> {
        if !delivery.is_postmortem() {
            return self.fake.stop_stalled(delivery);
        }
        self.judge.borrow_mut().stopped += 1;
        Ok(Completion::confirmed(vec![Observation::Postmortem {
            key: delivery.key.clone(),
            reply: None,
            failure: Some("worker_failed".into()),
        }]))
    }
}

fn raw(settings: &tl3_config::Store) -> postgres::Client {
    postgres::Config::new()
        .host(&settings.host)
        .port(settings.port)
        .user(&settings.user)
        .dbname(&settings.database)
        .password("tl3-disposable-test-password")
        .connect(postgres::NoTls)
        .unwrap()
}

fn drain(controller: &mut Controller) {
    for _ in 0..600 {
        if !controller.drive_once().unwrap() {
            return;
        }
    }
    panic!("controller failed to settle");
}

fn pace(per_day: u32) -> PostmortemPace {
    PostmortemPace {
        per_day,
        models: vec!["claude-opus-5-5".into()],
    }
}

struct Fixture {
    controller: Controller,
    settings: tl3_config::Store,
    judge: Rc<RefCell<Judge>>,
    _root: tempfile::TempDir,
    _database: support::Database,
}

impl Fixture {
    fn new() -> Self {
        let mut database = support::Database::start();
        let settings = database.create("recovery");
        let root = tempfile::tempdir().unwrap();
        let judge = Rc::new(RefCell::new(Judge::default()));
        let mut registry = adapters::simulation(root.path()).unwrap();
        registry.register(
            Upstream::Workers,
            Workers {
                fake: adapters::workers::Fake::new(root.path()),
                judge: judge.clone(),
            },
        );
        let mut controller = Controller::open(
            Store::open(&settings).unwrap(),
            registry,
            Bootstrap {
                repos: vec![Repository {
                    owner: "RobTand".into(),
                    name: "tl3-sandbox".into(),
                    installation: 1,
                }],
                rob: 24923402,
                slots: 1,
                pool: 1,
                fail_fast: false,
                operations: Some(tl3_config::OperationsIssue {
                    owner: "RobTand".into(),
                    name: "tl3-sandbox".into(),
                    installation_id: 1,
                    number: 1,
                }),
            },
        )
        .unwrap();
        raw(&settings)
            .execute(
                "UPDATE tl3_settings SET value = '1' WHERE key = 'postmortem_sample_share'",
                &[],
            )
            .unwrap();
        for number in 42..46 {
            controller
                .intake(LabelledIssue {
                    delivery: format!("fixture-{number}"),
                    owner: "RobTand".into(),
                    repo: "tl3-sandbox".into(),
                    installation: 1,
                    number,
                    title: format!("Fix build {number}"),
                    actor: 24923402,
                    label: "tl3".into(),
                    labels: vec![],
                })
                .unwrap();
            drain(&mut controller);
        }
        Self {
            controller,
            settings,
            judge,
            _root: root,
            _database: database,
        }
    }

    fn pause(&mut self) -> String {
        self.judge.borrow_mut().finished = true;
        self.judge.borrow_mut().failing = Some("worker_failed");
        self.controller.run_postmortems(pace(3));
        drain(&mut self.controller);
        assert_eq!(self.claims(), 3);
        self.pause_id().expect("three failures pause claims")
    }

    fn pause_id(&mut self) -> Option<String> {
        self.controller
            .snapshot()
            .unwrap()
            .rows
            .into_iter()
            .find(|row| {
                row.table == "incidents"
                    && row.value["open"] == true
                    && row.value["signature"]["class"] == "postmortems_paused"
            })
            .map(|row| row.id)
    }

    fn clear(&mut self, id: &str) {
        self.controller.observe(&format!("clear-{id}"), Observation::GitHubComment {
            repository: Repository { owner: "RobTand".into(), name: "tl3-sandbox".into(), installation: 1 },
            issue: 1,
            comment: serde_json::from_value(json!({"id": 1000 + self.claims(), "body": format!("clear incident {id}"),
                "user": {"id": 24923402, "login": "RobTand"}, "created_at": "2026-11-03T00:00:00Z",
                "updated_at": "2026-11-03T00:00:00Z", "issue_url": "https://api.github.com/repos/RobTand/tl3-sandbox/issues/1"})).unwrap(),
        }).unwrap();
    }

    fn claims(&self) -> u32 {
        Store::open(&self.settings)
            .unwrap()
            .postmortem_claims_today()
            .unwrap()
    }

    fn reason(&self) -> Option<status::PostmortemBlock> {
        let view = status::load(&mut Reader::open(&self.settings).unwrap()).unwrap();
        if let Some(reason) = view.postmortem_blocked {
            let text = format!("Postmortems blocked: {}", reason.as_str());
            assert!(status::render_text(&view).contains(&text));
            assert!(status::render_html(&view).contains(&text));
            assert_eq!(serde_json::to_value(reason).unwrap(), reason.as_str());
        }
        view.postmortem_blocked
    }

    fn capacity(&mut self, waiting: bool) {
        self.judge.borrow_mut().hold_makers = true;
        for number in 46..(if waiting { 48 } else { 47 }) {
            self.controller
                .intake(LabelledIssue {
                    delivery: format!("capacity-{number}"),
                    owner: "RobTand".into(),
                    repo: "tl3-sandbox".into(),
                    installation: 1,
                    number,
                    title: format!("Hold build {number}"),
                    actor: 24923402,
                    label: "tl3".into(),
                    labels: vec![],
                })
                .unwrap();
            drain(&mut self.controller);
        }
    }
}

#[test]
fn clearing_pause_resets_the_streak_without_restart() {
    let mut f = Fixture::new();
    let id = f.pause();
    f.judge.borrow_mut().finished = false;
    f.controller.run_postmortems(pace(10));
    f.clear(&id);
    drain(&mut f.controller);
    assert_eq!(
        f.claims(),
        4,
        "clearing the pause must reset the failure boundary"
    );
    assert!(f.pause_id().is_none());
}

#[test]
fn recovery_permits_exactly_one_validation_with_no_allowance() {
    let mut f = Fixture::new();
    let id = f.pause();
    f.judge.borrow_mut().failing = None;
    f.controller.run_postmortems(pace(0));
    f.clear(&id);
    drain(&mut f.controller);
    assert_eq!(
        f.claims(),
        4,
        "one validation is allowed even with zero daily allowance"
    );
    let done: i64 = raw(&f.settings)
        .query_one(
            "SELECT count(*) FROM tl3_postmortem_claims WHERE outcome = 'done'",
            &[],
        )
        .unwrap()
        .get(0);
    assert_eq!(done, 1, "the validation outcome is recorded");
    drain(&mut f.controller);
    assert_eq!(f.claims(), 4, "the validation permit is consumed once");
}

#[test]
fn validation_failure_repauses_after_one_run() {
    let mut f = Fixture::new();
    let id = f.pause();
    f.clear(&id);
    drain(&mut f.controller);
    assert_eq!(
        f.claims(),
        4,
        "a failed validation must not start more retries"
    );
    assert_ne!(
        f.pause_id().as_deref(),
        Some(id.as_str()),
        "the new failure needs a new pause incident"
    );
    assert!(f.pause_id().is_some());
}

#[test]
fn recovery_resets_allowance_without_deleting_claim_history() {
    let mut f = Fixture::new();
    let id = f.pause();
    f.judge.borrow_mut().failing = None;
    f.clear(&id);
    drain(&mut f.controller);
    assert_eq!(
        f.claims(),
        6,
        "the unchanged daily limit of three counts claims after recovery"
    );
    let failed: i64 = raw(&f.settings)
        .query_one(
            "SELECT count(*) FROM tl3_postmortem_claims WHERE outcome = 'failed'",
            &[],
        )
        .unwrap()
        .get(0);
    assert_eq!(failed, 3, "recovery retains the old attempts for audit");
}

#[test]
fn typed_reason_for_paused_streak() {
    let mut f = Fixture::new();
    f.pause();
    assert_eq!(f.reason(), Some(status::PostmortemBlock::PausedStreak));
}

#[test]
fn typed_reason_for_daily_allowance() {
    let mut f = Fixture::new();
    f.controller.run_postmortems(pace(0));
    drain(&mut f.controller);
    assert_eq!(f.reason(), Some(status::PostmortemBlock::DailyAllowance));
}

#[test]
fn typed_reason_for_q39_waiting_delivery() {
    let mut f = Fixture::new();
    f.capacity(true);
    f.controller.run_postmortems(pace(10));
    drain(&mut f.controller);
    assert_eq!(
        f.reason(),
        Some(status::PostmortemBlock::Q39WaitingDelivery)
    );
    assert_eq!(f.claims(), 0, "Q39 still yields to a waiting delivery");
}

#[test]
fn typed_reason_for_no_spare_capacity() {
    let mut f = Fixture::new();
    f.capacity(false);
    f.controller.run_postmortems(pace(10));
    drain(&mut f.controller);
    assert_eq!(f.reason(), Some(status::PostmortemBlock::NoSpareCapacity));
    assert_eq!(f.claims(), 0);
}

#[test]
fn new_release_recovers_the_pause_for_a_validation_run() {
    let mut f = Fixture::new();
    f.controller.start_release("before-fix").unwrap();
    f.pause();
    f.judge.borrow_mut().finished = false;
    f.controller.start_release("after-fix").unwrap();
    drain(&mut f.controller);
    assert_eq!(
        f.claims(),
        4,
        "a new release must recover the postmortem pause"
    );
    assert!(f.pause_id().is_none());
}
