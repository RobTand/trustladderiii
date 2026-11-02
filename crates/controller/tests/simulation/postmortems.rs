//! #403 slice B: the postmortem runner, outside the kernel, on spare pool capacity.
use super::*;
use std::{cell::RefCell, rc::Rc};
use tl3_controller::Result;

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
    permanent: bool,
    reply: Option<serde_json::Value>,
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
            return self.fake.reconcile(delivery);
        }
        let judge = self.judge.borrow();
        if !judge.finished {
            return Ok(Reconciliation::Pending);
        }
        Ok(Reconciliation::Found(Completion::confirmed(vec![
            Observation::Postmortem {
                key: delivery.key.clone(),
                reply: Some(judge.reply.clone().unwrap_or_else(|| reply(2))),
                failure: None,
            },
        ])))
    }
}

fn start(settings: &tl3_config::Store, root: &Path, judge: &Rc<RefCell<Judge>>) -> Controller {
    let mut registry = adapters::simulation(root).unwrap();
    registry.register(
        Upstream::Workers,
        Workers {
            fake: adapters::workers::Fake::new(root),
            judge: judge.clone(),
        },
    );
    let mut controller =
        Controller::open(Store::open(settings).unwrap(), registry, bootstrap()).unwrap();
    controller.run_postmortems();
    controller
}

fn restart(controller: Controller, settings: &tl3_config::Store) {
    drop(controller);
    controller_lock_barrier(settings);
}

fn issue(number: u64) -> LabelledIssue {
    LabelledIssue {
        delivery: format!("fixture-delivery-{number}"),
        number,
        title: format!("Fix the sandbox build {number}"),
        ..labelled()
    }
}

/// The stored sample share; the store must have migrated first.
fn share(settings: &tl3_config::Store, share: &str) {
    raw(settings)
        .execute(
            "UPDATE tl3_settings SET value = $1::text::jsonb WHERE key = 'postmortem_sample_share'",
            &[&share],
        )
        .unwrap();
}

fn status(settings: &tl3_config::Store, node: &str) -> Vec<(String, Option<String>)> {
    Store::open(settings)
        .unwrap()
        .postmortems(node)
        .unwrap()
        .into_iter()
        .map(|record| (record.status, record.run_key))
        .collect()
}

fn jobs(controller: &mut Controller) -> usize {
    controller
        .snapshot()
        .unwrap()
        .rows
        .iter()
        .filter(|row| row.table == "postmortem_jobs")
        .count()
}

/// A crash after the claim and a crash while the judge runs: each postmortem
/// is launched once and stored once. Only one runs at a time, and delivery
/// work never waits for it.
pub(super) fn exactly_once_across_crashes(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let judge = Rc::new(RefCell::new(Judge::default()));
    let mut controller = start(settings, root.path(), &judge);
    share(settings, "1");
    controller.intake(issue(42)).unwrap();
    for _ in 0..300 {
        assert!(
            controller.drive_once().unwrap(),
            "the postmortem was never claimed"
        );
        if status(settings, "0")
            .first()
            .is_some_and(|(status, _)| status == "running")
        {
            break;
        }
    }
    let claimed = status(settings, "0");
    let run_key = claimed[0].1.clone().expect("a claim records its run key");
    assert_eq!(
        judge.borrow().launched.len(),
        0,
        "the crash comes before dispatch"
    );
    assert_eq!(jobs(&mut controller), 1);

    restart(controller, settings);
    let mut controller = start(settings, root.path(), &judge);
    assert_eq!(
        status(settings, "0"),
        [("running".to_string(), Some(run_key.clone()))],
        "a claim with a live delivery stays running across a restart"
    );
    drain(&mut controller);
    assert_eq!(judge.borrow().launched.get(&run_key), Some(&1));

    // New delivery work runs and merges while the judge is still running, and
    // its own postmortem waits: one at a time.
    controller.intake(issue(43)).unwrap();
    drain(&mut controller);
    let world = controller.world().unwrap();
    assert_eq!(world.nodes[1].phase, Phase::Closed(Outcome::Merged));
    assert_eq!(status(settings, "1"), [("owed".to_string(), None)]);
    assert_eq!(jobs(&mut controller), 1);

    restart(controller, settings);
    judge.borrow_mut().finished = true;
    let mut controller = start(settings, root.path(), &judge);
    drain(&mut controller);
    assert_eq!(status(settings, "0")[0].0, "done");
    assert_eq!(status(settings, "1")[0].0, "done", "the next one ran after");
    assert!(
        judge.borrow().launched.values().all(|count| *count == 1),
        "{:?}",
        judge.borrow().launched
    );
    assert_eq!(judge.borrow().launched.len(), 2);
    assert_eq!(jobs(&mut controller), 0);

    let mut store = Store::open(settings).unwrap();
    let id = store.postmortems("0").unwrap()[0].id;
    let stored = store.postmortem(id).unwrap().unwrap();
    assert_eq!(stored.cause.as_deref(), Some("tl3"));
    assert_eq!(stored.assessment_ids, Some(json!(["a-1"])));
    assert_eq!(stored.recommendations.len(), 2);
    for (rank, recommendation) in stored.recommendations.iter().enumerate() {
        assert_eq!(recommendation.rank, rank as i16 + 1);
        assert!(
            recommendation.fingerprint.starts_with("rec-")
                && recommendation.fingerprint.len() == 20,
            "{}",
            recommendation.fingerprint
        );
    }
    let later: Vec<_> = store
        .code_grades("0")
        .unwrap()
        .into_iter()
        .filter(|grade| grade.trigger == format!("postmortem:{id}"))
        .map(|grade| grade.dimension)
        .collect();
    assert_eq!(later, ["tokens", "cache_rate"]);
    let mut client = raw(settings);
    let retained: Vec<serde_json::Value> = client
        .query("SELECT item FROM tl3_memory_outbox", &[])
        .unwrap()
        .iter()
        .map(|row| row.get(0))
        .filter(|item: &serde_json::Value| {
            item["tags"]
                .as_array()
                .is_some_and(|tags| tags.contains(&json!("postmortem")))
        })
        .collect();
    assert_eq!(retained.len(), 2, "each postmortem's lessons, once");
    let text = format!("{stored:?} {retained:?}");
    for planted in PLANTED {
        assert!(!text.contains(planted), "{planted} was stored or retained");
    }
    restart(controller, settings);
    let mut controller = start(settings, root.path(), &judge);
    assert!(!controller.drive_once().unwrap(), "nothing more to do");
    assert_eq!(judge.borrow().launched.len(), 2);
}

/// A `running` postmortem left by a process that died with no delivery is
/// owed again at startup, and then runs.
pub(super) fn running_rows_recover_at_startup(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let judge = Rc::new(RefCell::new(Judge {
        finished: true,
        ..Judge::default()
    }));
    let mut controller = start(settings, root.path(), &judge);
    share(settings, "0");
    controller.intake(issue(42)).unwrap();
    drain(&mut controller);
    assert!(status(settings, "0").is_empty(), "a clean merge at share 0");
    restart(controller, settings);
    raw(settings)
        .execute(
            "INSERT INTO tl3_postmortems (node_id, trigger, reason, repo, issue, title, status, run_key, revision, owed_at, started_at)
             SELECT '0', 'rob:dead', 'rob_asked', 'RobTand/tl3-sandbox', 42, 'Fix the sandbox build 42', 'running', 'postmortem:dead', max(revision), now(), now() FROM tl3_events",
            &[],
        )
        .unwrap();
    let mut controller = start(settings, root.path(), &judge);
    assert_eq!(status(settings, "0"), [("owed".to_string(), None)]);
    drain(&mut controller);
    assert_eq!(status(settings, "0")[0].0, "done");
    assert_eq!(judge.borrow().launched.len(), 1);
}

/// A launch refused for good, or a reply with four recommendations, fails the
/// postmortem with a fixed class. Neither raises an incident or posts.
pub(super) fn failures_are_fixed_classes(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let judge = Rc::new(RefCell::new(Judge {
        permanent: true,
        ..Judge::default()
    }));
    let mut controller = start(settings, root.path(), &judge);
    share(settings, "1");
    controller.intake(issue(42)).unwrap();
    drain(&mut controller);
    let mut store = Store::open(settings).unwrap();
    let record = store.postmortems("0").unwrap()[0].clone();
    assert_eq!(record.status, "failed");
    let failure = store.postmortem(record.id).unwrap().unwrap().failure;
    assert_eq!(failure.as_deref(), Some("launch_failed"));
    let incidents = |controller: &mut Controller| {
        controller
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .filter(|row| row.table == "incidents")
            .count()
    };
    assert_eq!(incidents(&mut controller), 0);

    {
        let mut judge = judge.borrow_mut();
        judge.permanent = false;
        judge.finished = true;
        judge.reply = Some(reply(4));
    }
    controller.intake(issue(43)).unwrap();
    drain(&mut controller);
    let record = store.postmortems("1").unwrap()[0].clone();
    assert_eq!(record.status, "failed");
    let failed = store.postmortem(record.id).unwrap().unwrap();
    assert_eq!(failed.failure.as_deref(), Some("invalid_reply"));
    assert!(failed.recommendations.is_empty());
    assert_eq!(incidents(&mut controller), 0);
}
