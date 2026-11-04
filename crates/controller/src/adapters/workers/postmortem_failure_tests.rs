use super::*;
use crate::git_tests::support::Database;

const PRIVATE: &str = "sk-planted687abcdefghijklmnop request-fragment-687";

struct RecordedJudge {
    fake: Fake,
    reason: Value,
    supervisor: Option<Supervisor>,
}

impl Executor for RecordedJudge {
    fn execute(&mut self, delivery: &Delivery) -> Result<Completion> {
        if delivery.is_postmortem() {
            return match &mut self.supervisor {
                Some(supervisor) => supervisor.execute(delivery),
                None => Ok(Completion::confirmed(vec![])),
            };
        }
        self.fake.execute(delivery)
    }

    fn reconcile(&mut self, delivery: &Delivery) -> Result<Reconciliation> {
        if !delivery.is_postmortem() {
            return self.fake.reconcile(delivery);
        }
        if let Some(supervisor) = &self.supervisor {
            let (_, attempt) = latest(&supervisor.root(&delivery.key))?.unwrap();
            return inspect(&attempt, delivery);
        }
        let attempt = tempfile::tempdir().unwrap();
        write(&attempt.path().join("exit.json"), &json!({"code": 1}))?;
        write(
            &attempt.path().join("memory.json"),
            &json!({"outcome": "failed", "reason": self.reason}),
        )?;
        inspect(attempt.path(), delivery)
    }

    fn resume_after_quota(&mut self, delivery: &Delivery) -> Result<Option<Completion>> {
        match &mut self.supervisor {
            Some(supervisor) if delivery.is_postmortem() => supervisor.resume_after_quota(delivery),
            _ => self.fake.resume_after_quota(delivery),
        }
    }

    fn quota_resume_ms(&mut self, delivery: &Delivery, announced: u64, now: u64) -> Result<u64> {
        match &mut self.supervisor {
            Some(supervisor) if delivery.is_postmortem() => {
                supervisor.quota_resume_ms(delivery, announced, now)
            }
            _ => self.fake.quota_resume_ms(delivery, announced, now),
        }
    }
}

fn drain(controller: &mut crate::Controller) {
    for _ in 0..600 {
        if !controller.drive_once().unwrap() {
            return;
        }
    }
    panic!("controller failed to settle");
}

fn boundary_case(class: &str, status: Value, expected: &str, launch: bool) {
    let root = tempfile::tempdir().unwrap();
    let mut db = Database::start();
    let mut config =
        tl3_config::Config::parse(include_str!("../../../../config/example.toml")).unwrap();
    config.store = db.create("judge_failure");
    let supervisor = if launch {
        config.workers.runs = root.path().join("runs");
        config.workers.python = "/usr/bin/python3".into();
        config.models.key_file = root.path().join("fixture-key");
        fs::write(&config.models.key_file, PRIVATE).unwrap();
        let launcher = root.path().join("judge-launcher.py");
        fs::write(
            &launcher,
            r#"import json, sys
from pathlib import Path
attempt = Path(sys.argv[-1])
request = json.loads((attempt / "request.json").read_text())
if summary := request.get("memory", {}).get("summary_file"):
    Path(summary).write_text(json.dumps({"outcome": "failed", "reason": {
        "failure_class": "iteration_limit", "failure_stage": "postmortem judge failure"}}))
(attempt / "exit.json").write_text(json.dumps({"code": 1}))
print("claimed", flush=True)
"#,
        )
        .unwrap();
        config.workers.launcher =
            vec!["/usr/bin/python3".into(), launcher.to_str().unwrap().into()];
        Some(Supervisor::new(&config, &root.path().join("records")).unwrap())
    } else {
        None
    };
    let mut registry = crate::adapters::simulation(root.path()).unwrap();
    registry.register(
        crate::executor::Upstream::Workers,
        RecordedJudge {
            fake: Fake::new(root.path()),
            supervisor,
            reason: json!({"failure_class": class, "failure_http_status": status,
                "failure_stage": "postmortem judge failure", "failure_message": PRIVATE}),
        },
    );
    let mut controller = crate::Controller::open(
        Store::open(&config.store).unwrap(),
        registry,
        crate::Bootstrap::from_config(&config).unwrap(),
    )
    .unwrap();
    postgres::Config::new()
        .host(&config.store.host)
        .port(config.store.port)
        .user(&config.store.user)
        .dbname(&config.store.database)
        .password("tl3-disposable-test-password")
        .connect(postgres::NoTls)
        .unwrap()
        .execute(
            "UPDATE tl3_settings SET value = '1'::jsonb WHERE key = 'postmortem_sample_share'",
            &[],
        )
        .unwrap();
    for number in 42..46 {
        controller
            .intake(crate::LabelledIssue {
                delivery: format!("fixture-{number}"),
                owner: "RobTand".into(),
                repo: "tl3-sandbox".into(),
                installation: 1,
                number,
                title: format!("Fix build {number}"),
                actor: config.github.rob.id,
                label: "tl3".into(),
                labels: vec![],
            })
            .unwrap();
        drain(&mut controller);
    }
    controller.run_postmortems(crate::PostmortemPace::from_config(&config));
    drain(&mut controller);
    let mut store = Store::open(&config.store).unwrap();
    let streak = store.postmortem_failure_streak(0).unwrap();
    assert_eq!(
        streak.len(),
        3,
        "the existing three-failure pause stays in force"
    );
    assert_eq!(
        streak,
        vec![expected; 3],
        "stored claim failures must retain the safe cause"
    );
    let mut client = postgres::Config::new()
        .host(&config.store.host)
        .port(config.store.port)
        .user(&config.store.user)
        .dbname(&config.store.database)
        .password("tl3-disposable-test-password")
        .connect(postgres::NoTls)
        .unwrap();
    let failed = client
        .query(
            "SELECT p.id, count(c.run_key) FROM tl3_postmortems p JOIN tl3_postmortem_claims c ON c.postmortem_id = p.id WHERE p.failed_at IS NOT NULL GROUP BY p.id",
            &[],
        )
        .unwrap();
    assert!(!failed.is_empty());
    for row in failed {
        let detail = store.postmortem(row.get(0)).unwrap().unwrap();
        assert_eq!(detail.failure.as_deref(), Some(expected));
        let claims: i64 = row.get(1);
        assert!((1..=3).contains(&claims));
        assert_eq!(
            i64::from(detail.record.attempts),
            claims.min(2),
            "each failed claim retains the existing two-retry allowance"
        );
        assert_eq!(
            detail.record.status,
            if claims < 3 { "owed" } else { "failed" }
        );
    }
    let snapshot = controller.snapshot().unwrap();
    let incident = snapshot
        .rows
        .iter()
        .find(|row| {
            row.table == "incidents"
                && row.value["open"] == true
                && row.value["signature"]["class"] == "postmortems_paused"
        })
        .expect("the usual pause must still be raised");
    let encoded = incident.value.to_string();
    assert!(
        encoded.contains(expected),
        "pause incident must name {expected}: {encoded}"
    );
    let mut reader = tl3_store::Reader::open(&config.store).unwrap();
    let status = crate::status::load(&mut reader).unwrap();
    for text in [
        crate::status::render_text(&status),
        crate::status::render_html(&status),
    ] {
        assert!(
            text.contains(expected),
            "status must name {expected}: {text}"
        );
        assert!(!text.contains(PRIVATE));
        assert!(!text.contains("request-fragment-687"));
    }
}

macro_rules! judge_case {
    ($name:ident, $class:expr, $status:expr, $expected:expr) => {
        #[test]
        fn $name() {
            boundary_case($class, $status, $expected, false);
        }
    };
}

judge_case!(quota, "quota", json!("429"), "quota HTTP 429");
judge_case!(refusal, "refusal", json!("403"), "refusal HTTP 403");
judge_case!(http, "http", json!("502"), "http HTTP 502");
judge_case!(model_deadline, "deadline", Value::Null, "deadline");
judge_case!(transport, "transport", Value::Null, "transport");
judge_case!(schema, "schema", Value::Null, "schema");
judge_case!(contract, "contract", Value::Null, "contract");
judge_case!(configuration, "configuration", Value::Null, "configuration");
judge_case!(
    iteration_limit,
    "iteration_limit",
    Value::Null,
    "iteration_limit"
);
judge_case!(mlflow, "mlflow", Value::Null, "mlflow");
judge_case!(other, "other", Value::Null, "other");
judge_case!(unknown_is_other, PRIVATE, Value::Null, "other");
judge_case!(invalid_http_text, "http", json!(PRIVATE), "http");
judge_case!(invalid_http_range, "http", json!("999"), "http");
judge_case!(invalid_http_type, "http", json!(true), "http");

#[test]
fn launched_iteration_limit_reaches_the_pause_and_status() {
    boundary_case("iteration_limit", Value::Null, "iteration_limit", true);
}

/// #759: a postmortem whose grade model is unavailable ends like quota, not
/// as a failed run. The ending charges the grade model, and the run resumes at
/// once with the writer's model grading, as other roles fall back.
#[test]
fn a_postmortem_resumes_when_its_grade_model_is_unavailable() {
    let root = tempfile::tempdir().unwrap();
    let mut db = Database::start();
    let mut config =
        tl3_config::Config::parse(include_str!("../../../../config/example.toml")).unwrap();
    config.store = db.create("postmortem_unavailable");
    config.models.postmortem = config.models.maker.clone();
    let [writer, grader] = [0, 1].map(|index| config.models.postmortem[index].name.clone());
    config.workers.runs = root.path().join("runs");
    config.workers.python = "/usr/bin/python3".into();
    config.models.key_file = root.path().join("fixture-key");
    fs::write(&config.models.key_file, PRIVATE).unwrap();
    let reply = root.path().join("reply.json");
    write(&reply, &crate::postmortems::simulated_reply()).unwrap();
    let launcher = root.path().join("unavailable-launcher.py");
    // The first attempt's grade judge finds its model unroutable; the next replies.
    fs::write(
        &launcher,
        format!(
            r#"import json, sys, time
from pathlib import Path
attempt = Path(sys.argv[-1])
request = json.loads((attempt / "request.json").read_text())
if attempt.name.endswith("00000000"):
    event = {{"event": "quota", "role": "postmortem",
             "reset_ms": int(time.time() * 1000) + 900000,
             "model": request["inputs"]["grade_model"]}}
else:
    event = {{"event": "reply", "role": "postmortem",
             "reply": json.loads(Path({reply:?}).read_text())}}
(attempt / "stdout.jsonl").write_text(json.dumps(event) + "\n")
(attempt / "exit.json").write_text(json.dumps({{"code": 0}}))
print("claimed", flush=True)
"#
        ),
    )
    .unwrap();
    config.workers.launcher = vec!["/usr/bin/python3".into(), launcher.to_str().unwrap().into()];
    let supervisor = Supervisor::new(&config, &root.path().join("records")).unwrap();
    let mut registry = crate::adapters::simulation(root.path()).unwrap();
    registry.register(
        crate::executor::Upstream::Workers,
        RecordedJudge {
            fake: Fake::new(root.path()),
            supervisor: Some(supervisor),
            reason: Value::Null,
        },
    );
    let mut controller = crate::Controller::open(
        Store::open(&config.store).unwrap(),
        registry,
        crate::Bootstrap::from_config(&config).unwrap(),
    )
    .unwrap();
    postgres::Config::new()
        .host(&config.store.host)
        .port(config.store.port)
        .user(&config.store.user)
        .dbname(&config.store.database)
        .password("tl3-disposable-test-password")
        .connect(postgres::NoTls)
        .unwrap()
        .execute(
            "UPDATE tl3_settings SET value = '1'::jsonb WHERE key = 'postmortem_sample_share'",
            &[],
        )
        .unwrap();
    controller
        .intake(crate::LabelledIssue {
            delivery: "fixture-unavailable".into(),
            owner: "RobTand".into(),
            repo: "tl3-sandbox".into(),
            installation: 1,
            number: 42,
            title: "Fix the build".into(),
            actor: config.github.rob.id,
            label: "tl3".into(),
            labels: vec![],
        })
        .unwrap();
    drain(&mut controller);
    controller.run_postmortems(crate::PostmortemPace::from_config(&config));
    let mut store = Store::open(&config.store).unwrap();
    for _ in 0..100 {
        drain(&mut controller);
        controller.look_due().unwrap();
        if matches!(
            store.postmortems("0").unwrap()[0].status.as_str(),
            "done" | "failed"
        ) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(store.postmortems("0").unwrap()[0].status, "done");
    assert!(
        store.postmortem_failure_streak(0).unwrap().is_empty(),
        "an unavailable model is not a failed run"
    );
    let key = store
        .effects()
        .unwrap()
        .into_iter()
        .find(|effect| effect.key.starts_with("postmortem:"))
        .unwrap()
        .key;
    let run = run_root(&config.workers.runs, &key);
    let models = |number: u32| {
        let request = read(&run.join(format!("attempt-{number:08}/request.json")))
            .unwrap()
            .unwrap();
        (
            request["model"]["name"].as_str().unwrap().to_owned(),
            request["inputs"]["grade_model"].as_str().unwrap().to_owned(),
        )
    };
    assert_eq!(models(0), (writer.clone(), grader.clone()));
    assert_eq!(
        models(1),
        (writer.clone(), writer.clone()),
        "the resumed run grades on the writer's model"
    );
    let snapshot = controller.snapshot().unwrap();
    let charged: Vec<_> = snapshot
        .rows
        .iter()
        .filter(|row| row.table == "model_quotas")
        .map(|row| row.id.clone())
        .collect();
    assert_eq!(charged, [grader], "the ending charges the grade model");
}
