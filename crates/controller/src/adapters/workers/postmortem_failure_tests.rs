use super::*;
use crate::git_tests::support::Database;

const PRIVATE: &str = "sk-planted687abcdefghijklmnop request-fragment-687";

struct RecordedJudge {
    fake: Fake,
    reason: Value,
}

impl Executor for RecordedJudge {
    fn execute(&mut self, delivery: &Delivery) -> Result<Completion> {
        if delivery.is_postmortem() {
            return Ok(Completion::confirmed(vec![]));
        }
        self.fake.execute(delivery)
    }

    fn reconcile(&mut self, delivery: &Delivery) -> Result<Reconciliation> {
        if !delivery.is_postmortem() {
            return self.fake.reconcile(delivery);
        }
        let attempt = tempfile::tempdir().unwrap();
        write(&attempt.path().join("exit.json"), &json!({"code": 1}))?;
        write(
            &attempt.path().join("memory.json"),
            &json!({"outcome": "failed", "reason": self.reason}),
        )?;
        inspect(attempt.path(), delivery)
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

fn boundary_case(class: &str, status: Value, expected: &str) {
    let root = tempfile::tempdir().unwrap();
    let mut db = Database::start();
    let mut config =
        tl3_config::Config::parse(include_str!("../../../../config/example.toml")).unwrap();
    config.store = db.create("judge_failure");
    let mut registry = crate::adapters::simulation(root.path()).unwrap();
    registry.register(
        crate::executor::Upstream::Workers,
        RecordedJudge {
            fake: Fake::new(root.path()),
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
            "SELECT id FROM tl3_postmortems WHERE failed_at IS NOT NULL",
            &[],
        )
        .unwrap();
    assert!(!failed.is_empty());
    for row in failed {
        let detail = store.postmortem(row.get(0)).unwrap().unwrap();
        assert_eq!(detail.failure.as_deref(), Some(expected));
        assert_eq!(
            detail.record.attempts, 2,
            "diagnostics must retain the two-retry allowance"
        );
        assert_eq!(detail.record.status, "failed");
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
            boundary_case($class, $status, $expected);
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
