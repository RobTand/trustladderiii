//! #828 / Q53: operator reset evidence goes through authenticated comment ingestion.
#[path = "../../store/tests/support/mod.rs"]
mod support;
use serde_json::{Value, json};
use std::{
    cell::Cell,
    rc::Rc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tl3_controller::{
    Bootstrap, Controller, LabelledIssue, Repository, adapters,
    clock::Clock,
    executor::{Completion, Delivery, Executor, Look, Observation, Reconciliation, Upstream},
    replay, status,
};
use tl3_store::{Commit, Reader, RowChange, Store};
const MODEL: &str = "gpt-6.1-sol";
const NOW: u64 = 1_800_000_000_000;
const RESET: u64 = NOW + 600_000;
#[derive(Clone)]
struct Time(Rc<Cell<u64>>);
impl Clock for Time {
    fn now(&self) -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(self.0.get())
    }
}
struct Workers;
impl Executor for Workers {
    fn configured_model(&self, model: &str) -> bool {
        model == MODEL
    }
    fn quota_model_eligible(
        &mut self,
        delivery: &Delivery,
        model: &str,
    ) -> tl3_controller::Result<bool> {
        Ok(model == MODEL && delivery.key != "other")
    }
    fn execute(&mut self, _: &Delivery) -> tl3_controller::Result<Completion> {
        Ok(Completion::confirmed(vec![]))
    }
    fn reconcile(&mut self, _: &Delivery) -> tl3_controller::Result<Reconciliation> {
        Ok(Reconciliation::Pending)
    }
    fn look(&mut self, _: &Delivery) -> tl3_controller::Result<Look> {
        Ok(Look::Healthy)
    }
}
struct Fixture {
    c: Controller,
    store: Store,
    settings: tl3_config::Store,
    _root: tempfile::TempDir,
    time: Time,
    _database: support::Database,
}
impl Fixture {
    fn new(name: &str) -> Self {
        let mut db = support::Database::start();
        let settings = db.create(name);
        Self::with_database(db, settings)
    }
    fn with_database(db: support::Database, settings: tl3_config::Store) -> Self {
        let root = tempfile::tempdir().unwrap();
        let mut registry = adapters::simulation(root.path()).unwrap();
        registry.register(Upstream::Workers, Workers);
        let time = Time(Rc::new(Cell::new(NOW)));
        let mut c = Controller::open_with_clock(
            Store::open(&settings).unwrap(),
            registry,
            Bootstrap {
                rob: 42,
                slots: 1,
                pool: 2,
                fail_fast: false,
                repos: vec![repo("sandbox")],
                operations: Some(tl3_config::OperationsIssue {
                    owner: "owner".into(),
                    name: "ops".into(),
                    installation_id: 3,
                    number: 17,
                }),
            },
            time.clone(),
        )
        .unwrap();
        c.intake(LabelledIssue {
            delivery: "intake".into(),
            owner: "owner".into(),
            repo: "sandbox".into(),
            installation: 3,
            number: 12,
            title: "Test quota".into(),
            actor: 42,
            label: "tl3".into(),
            labels: vec![],
        })
        .unwrap();
        drain(&mut c);
        let store = Store::open(&settings).unwrap();
        let mut f = Self {
            c,
            store,
            settings,
            _root: root,
            time,
            _database: db,
        };
        let delivery =
            f.c.snapshot()
                .unwrap()
                .rows
                .into_iter()
                .find(|r| r.table == "allowances")
                .unwrap()
                .value;
        for (id, model, quota) in [
            ("waiting-1", MODEL, true),
            ("waiting-2", MODEL, true),
            ("other", "other-model", true),
            ("running", MODEL, false),
        ] {
            let mut state = delivery.clone();
            state["delivery"]["key"] = json!(id);
            state["quota_ms"] = if quota { json!(RESET) } else { Value::Null };
            state["last_quota_ms"] = json!(RESET);
            state["last_quota_model"] = json!(model);
            state["due_ms"] = json!(RESET);
            f.put("allowances", id, state);
        }
        f.put("model_quotas", MODEL, json!({"reset_ms":RESET}));
        // The base does not instrument kernel replay; use the existing supported anchor harness.
        let snap = f.store.load().unwrap();
        let mut anchor = Commit::new("fixture:anchor", json!({}), &snap);
        anchor.result = json!({"kernel_anchor": f.c.world().unwrap()});
        f.store.commit(&anchor).unwrap();
        f
    }
    fn put(&mut self, table: &str, id: &str, value: Value) {
        let snap = self.store.load().unwrap();
        let mut commit = Commit::new(
            format!("fixture:{}:{table}:{id}", snap.revision.get()),
            json!({}),
            &snap,
        );
        commit.rows.push(RowChange::Put {
            table: table.into(),
            id: id.into(),
            value,
        });
        self.store.commit(&commit).unwrap();
    }
    fn comment(&mut self, delivery: &str, id: u64, actor: u64, issue: u64, body: &str) {
        self.c
            .observe(
                delivery,
                Observation::GitHubComment {
                    repository: repo("ops"),
                    issue,
                    comment: tl3_github::Comment {
                        id,
                        body: body.into(),
                        user: tl3_github::Actor {
                            id: actor,
                            login: "fixture".into(),
                        },
                        created_at: chrono::DateTime::UNIX_EPOCH,
                        updated_at: chrono::DateTime::UNIX_EPOCH,
                        issue_url: format!("https://api.github.com/repos/owner/ops/issues/{issue}"),
                    },
                },
            )
            .unwrap();
        // Consume only: do not dispatch reply effects or run the awakened looks yet.
        assert!(self.c.drive_once().unwrap());
    }
    fn clear(&mut self, id: u64) {
        self.comment(
            &format!("clear-{id}"),
            id,
            42,
            17,
            &format!("clear quota {MODEL}"),
        );
    }
    fn row(&mut self, table: &str, id: &str) -> Option<Value> {
        self.store
            .load()
            .unwrap()
            .rows
            .into_iter()
            .find(|r| r.table == table && r.id == id)
            .map(|r| r.value)
    }
    fn reply(&mut self) -> String {
        self.store
            .load()
            .unwrap()
            .pending_effects
            .into_iter()
            .filter_map(|e| e.payload["note"].as_str().map(str::to_owned))
            .collect::<Vec<_>>()
            .join("\n")
    }
}
fn repo(name: &str) -> Repository {
    Repository {
        owner: "owner".into(),
        name: name.into(),
        installation: 3,
    }
}
fn drain(c: &mut Controller) {
    for _ in 0..100 {
        if !c.drive_once().unwrap() {
            return;
        }
    }
    panic!("did not settle");
}

/// Q53 / #828: one commit deletes the hold and wakes only waits on that model.
#[test]
fn clears_and_wakes_atomically() {
    let mut f = Fixture::new("quota_clear");
    f.clear(100);
    assert!(
        f.row("model_quotas", MODEL).is_none(),
        "operator reset must delete quota hold"
    );
    for id in ["waiting-1", "waiting-2"] {
        assert_eq!(f.row("allowances", id).unwrap()["due_ms"], NOW);
        assert_eq!(f.row("allowances", id).unwrap()["quota_ms"], NOW);
    }
    for id in ["other", "running"] {
        assert_eq!(f.row("allowances", id).unwrap()["due_ms"], RESET);
    }
    let mut reader = Reader::open(&f.settings).unwrap();
    let event = reader
        .events_after(tl3_store::Revision::ZERO, 128)
        .unwrap()
        .into_iter()
        .find(|e| e.key == format!("model-quota-clear:{MODEL}:100"))
        .expect("named quota-clear commit");
    let snap = f.store.load().unwrap();
    for id in ["waiting-1", "waiting-2"] {
        assert_eq!(
            snap.rows
                .iter()
                .find(|r| r.table == "allowances" && r.id == id)
                .unwrap()
                .version,
            event.receipt.revision
        );
    }
    assert!(
        f.reply()
            .contains(&format!("quota for {MODEL} cleared by operator"))
    );
}
/// Q53 / #828: a typo or an unconfigured model cannot delete any hold.
#[test]
fn unknown_and_unconfigured_models_reply_without_mutation() {
    let mut f = Fixture::new("quota_unknown");
    for (id, model) in [(101, "typo"), (102, "claude-sonnet-5-5")] {
        f.put("model_quotas", model, json!({"reset_ms":RESET}));
        f.comment(
            &format!("unknown-{id}"),
            id,
            42,
            17,
            &format!("clear quota {model}"),
        );
        assert!(
            f.reply()
                .contains(&format!("{model} is not a configured model")),
            "{}",
            f.reply()
        );
        assert!(f.row("model_quotas", model).is_some());
    }
    assert_eq!(f.row("allowances", "waiting-1").unwrap()["due_ms"], RESET);
}
/// Q53 / #828: the no-hold answer never wakes allowances or changes clear history.
#[test]
fn configured_without_hold_replies_without_effects() {
    let mut f = Fixture::new("quota_no_hold");
    f.clear(100);
    f.time.0.set(NOW + 1000);
    let history = f.row("model_quota_clears", MODEL);
    f.clear(101);
    assert!(
        f.reply()
            .contains(&format!("no quota hold recorded for {MODEL}"))
    );
    assert_eq!(f.row("model_quota_clears", MODEL), history);
    assert_eq!(f.row("allowances", "waiting-1").unwrap()["due_ms"], NOW);
}
/// Q53 / #828: delivery replay and a new clear comment are harmless.
#[test]
fn duplicate_comments_apply_once() {
    let mut f = Fixture::new("quota_duplicate");
    f.clear(100);
    let history = f.row("model_quota_clears", MODEL).expect("clear recorded");
    f.time.0.set(NOW + 1000);
    f.comment("redelivery", 100, 42, 17, &format!("clear quota {MODEL}"));
    f.clear(101);
    assert_eq!(f.row("model_quota_clears", MODEL), Some(history));
    assert_eq!(f.row("allowances", "waiting-2").unwrap()["due_ms"], NOW);
}
/// Q53 / #828: a later provider report establishes a new hold normally.
#[test]
fn later_quota_report_records_normally() {
    let mut f = Fixture::new("quota_later");
    f.clear(100);
    assert!(f.row("model_quotas", MODEL).is_none());
    f.c.observe(
        "later-quota",
        Observation::Quota {
            key: "waiting-1".into(),
            reset_ms: RESET + 1000,
            model: Some(MODEL.into()),
        },
    )
    .unwrap();
    assert!(f.c.drive_once().unwrap());
    assert_eq!(
        f.row("model_quotas", MODEL).unwrap()["reset_ms"],
        RESET + 1000
    );
    f.comment(
        "old-clear-redelivery",
        100,
        42,
        17,
        &format!("clear quota {MODEL}"),
    );
    assert_eq!(
        f.row("model_quotas", MODEL).unwrap()["reset_ms"],
        RESET + 1000
    );
}
/// Q53 / #828: replay-check accepts the committed clear and leaves the store identical.
#[test]
fn clear_survives_replay_check() {
    let mut f = Fixture::new("quota_replay");
    f.clear(100);
    assert!(f.row("model_quotas", MODEL).is_none());
    let before = f.store.load().unwrap();
    f.comment(
        "replayed-clear",
        100,
        42,
        17,
        &format!("clear quota {MODEL}"),
    );
    assert_eq!(before.rows, f.store.load().unwrap().rows);
    let before = f.store.load().unwrap();
    let report = replay::run(
        &mut Reader::open(&f.settings).unwrap(),
        Some("fixture:anchor"),
    )
    .unwrap();
    assert!(report.events >= 3);
    let after = f.store.load().unwrap();
    assert_eq!(before.rows, after.rows);
    assert_eq!(before.revision, after.revision);
    let event = Reader::open(&f.settings)
        .unwrap()
        .events_after(tl3_store::Revision::ZERO, 128)
        .unwrap()
        .into_iter()
        .find(|event| event.key == format!("model-quota-clear:{MODEL}:100"))
        .unwrap();
    let mut reconstructed = Fixture::new("quota_reconstructed");
    reconstructed
        .c
        .observe("clear-100", serde_json::from_value(event.payload).unwrap())
        .unwrap();
    assert!(reconstructed.c.drive_once().unwrap());
    assert_eq!(
        after.rows,
        reconstructed.store.load().unwrap().rows,
        "replaying the recorded operations input reconstructs every store row"
    );
}
/// Q53 / #828: the existing status quota data renders the operator clear time.
#[test]
fn status_reports_operator_clear() {
    let mut f = Fixture::new("quota_status");
    f.clear(100);
    let view = status::load(&mut Reader::open(&f.settings).unwrap()).unwrap();
    assert!(
        view.quota_models
            .iter()
            .any(|line| line.contains(&format!("quota for {MODEL} cleared by operator at"))),
        "{:?}",
        view.quota_models
    );
}
/// Q53 / #828: differential command boundary, matching clear incident's auth and grammar.
#[test]
fn incident_and_quota_share_authorization_and_parser_boundary() {
    let mut f = Fixture::new("quota_auth");
    for (index, actor, issue) in [(0, 7, 17), (1, 42, 18)] {
        let mut receipts = vec![];
        for (kind, value) in [("incident", "missing"), ("quota", MODEL)] {
            let id = 200 + index * 2 + receipts.len() as u64;
            f.comment(
                &format!("auth-{id}"),
                id,
                actor,
                issue,
                &format!("clear {kind} {value}"),
            );
            receipts.push(
                f.row("operations_comments", &format!("owner/ops/3/{id}"))
                    .is_some(),
            );
        }
        assert_eq!(receipts, [false, false]);
        assert!(f.row("model_quotas", MODEL).is_some());
    }
    for (index, template, expected) in [
        (0, "clear {kind} {value}", true),
        (1, " \nclear {kind} {value}\n ", true),
        (2, "Clear {kind} {value}", false),
        (3, "clear  {kind} {value}", false),
        (4, "clear {kind}  {value}", false),
    ] {
        f.put(
            "incidents",
            "fixture",
            json!({"id":"fixture", "node":0,
            "open":true, "needs_rob":true, "blocking":false, "summary":"fixture",
            "signature":{"upstream":"GitHub", "class":"permission"}}),
        );
        f.put("model_quotas", MODEL, json!({"reset_ms":RESET}));
        let mut cleared = vec![];
        for (offset, kind, value) in [(0, "incident", "fixture"), (1, "quota", MODEL)] {
            let id = 400 + index * 2 + offset;
            let body = template.replace("{kind}", kind).replace("{value}", value);
            f.comment(&format!("grammar-{id}"), id, 42, 17, &body);
            cleared.push(if kind == "incident" {
                f.row("incidents", "fixture").unwrap()["open"] == false
            } else {
                f.row("model_quotas", MODEL).is_none()
            });
        }
        assert_eq!(cleared, [expected, expected], "{template}");
    }
}
