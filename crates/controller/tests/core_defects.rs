#[path = "../../store/tests/support/mod.rs"]
mod support;
#[allow(dead_code)]
mod worktree_fixture;

use serde_json::{Value, json};
use std::{cell::RefCell, fs, rc::Rc};
use tl3_controller::{
    Bootstrap, Controller, LabelledIssue, Repository, adapters,
    executor::{
        Completion, Delivery, Executor, Look, Observation, Output, Reconciliation, Upstream,
    },
};
use tl3_kernel::{EffectKind, NodeId, Outcome, Phase};
use tl3_store::{Commit, EffectTransition, NewEffect, RowChange, Store};

fn repository() -> Repository {
    Repository {
        owner: "owner".into(),
        name: "sandbox".into(),
        installation: 3,
    }
}
fn open(settings: &tl3_config::Store, root: &std::path::Path, adapter: Recorder) -> Controller {
    let mut registry = adapters::simulation(root).unwrap();
    registry.register(Upstream::Workers, adapter);
    let mut c = Controller::open(
        Store::open(settings).unwrap(),
        registry,
        Bootstrap {
            repos: vec![repository()],
            rob: 42,
            slots: 1,
            pool: 1,
            operations: None,
        },
    )
    .unwrap();
    c.intake(LabelledIssue {
        delivery: "label".into(),
        owner: "owner".into(),
        repo: "sandbox".into(),
        installation: 3,
        number: 12,
        title: "Fix a greeting".into(),
        actor: 42,
        label: "tl3".into(),
    })
    .unwrap();
    c
}
#[derive(Clone, Default)]
struct Recorder {
    seen: Rc<RefCell<Vec<Delivery>>>,
    observation: Option<Observation>,
    unreadable: bool,
}
impl Executor for Recorder {
    fn execute(&mut self, _: &Delivery) -> tl3_controller::Result<Completion> {
        Ok(Completion::confirmed(
            self.observation.clone().into_iter().collect(),
        ))
    }
    fn reconcile(&mut self, _: &Delivery) -> tl3_controller::Result<Reconciliation> {
        Ok(Reconciliation::Pending)
    }
    fn look(&mut self, delivery: &Delivery) -> tl3_controller::Result<Look> {
        self.seen.borrow_mut().push(delivery.clone());
        if self.unreadable {
            return Err(std::io::Error::from(std::io::ErrorKind::ConnectionRefused).into());
        }
        Ok(match &self.observation {
            Some(observation) => Look::Found(Completion::confirmed(vec![observation.clone()])),
            None => Look::Healthy,
        })
    }
}
fn put(table: &str, id: &str, value: Value) -> RowChange {
    RowChange::Put {
        table: table.into(),
        id: id.into(),
        value,
    }
}
fn worker_key(c: &mut Controller) -> String {
    c.snapshot()
        .unwrap()
        .pending_effects
        .iter()
        .find(|e| e.kind == "Workers")
        .unwrap()
        .key
        .clone()
}
fn progress() -> Observation {
    Observation::Progress {
        key: "work".into(),
        checkpoint: "one".into(),
    }
}

#[test]
fn previous_delivery_shape_reconciles_inflight_worktree() {
    let root = tempfile::tempdir().unwrap();
    let f = worktree_fixture::Fixture::create(root.path());
    let delivery = f.delivery(EffectKind::PrepareWorktree, 1);
    f.adapter().execute(&delivery).unwrap();
    let key: String = delivery.key.bytes().map(|b| format!("{b:02x}")).collect();
    let path = f.repo.join(format!("tl3-controller/effect-{key}.json"));
    let mut record: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    record["completion"] = Value::Null;
    for field in [
        "replanned",
        "checks",
        "job_generation",
        "question",
        "note",
        "marker_key",
        "question_context",
        "steering",
        "request_text",
    ] {
        record["delivery"].as_object_mut().unwrap().remove(field);
    }
    fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    let mut current = delivery.clone();
    current.pull_request = Some(99);
    current.request_text = Some("Refreshed request context".into());
    assert!(matches!(
        f.adapter().reconcile(&current).unwrap(),
        Reconciliation::Found(_)
    ));
    let mut changed = current.clone();
    changed.issue += 1;
    assert!(
        f.adapter().reconcile(&changed).is_err(),
        "changed operation must still be rejected"
    );
}

#[test]
fn look_reads_current_delivery() {
    let mut db = support::Database::start();
    let settings = db.create("look_current");
    let root = tempfile::tempdir().unwrap();
    let adapter = Recorder::default();
    let seen = adapter.seen.clone();
    let mut c = open(&settings, root.path(), adapter);
    let key = worker_key(&mut c);
    let mut store = Store::open(&settings).unwrap();
    let mut change = Commit::new("pr-binding", json!(null), &store.load().unwrap());
    change
        .rows
        .push(put("pull_requests", "0", json!({"number": 99})));
    store.commit(&change).unwrap();
    c.look_key(&key).unwrap();
    assert_eq!(seen.borrow().last().unwrap().pull_request, Some(99));
    db.stop();
}

#[test]
fn comment_output_blocks_session_retirement() {
    let mut db = support::Database::start();
    for watched in [false, true] {
        let settings = db.create(if watched {
            "comment_watch"
        } else {
            "comment_pending"
        });
        let mut store = Store::open(&settings).unwrap();
        let mut change = Commit::new("closed-comment", json!(null), &store.load().unwrap());
        change.rows.push(put(
            "nodes",
            "0",
            json!({"phase": Phase::Closed(Outcome::Merged)}),
        ));
        let payload = json!({"output": Output::Comment { node: NodeId(0), kind: tl3_controller::executor::CommentKind::Classify }});
        change.effects.push(NewEffect {
            key: "comment".into(),
            kind: "Workers".into(),
            payload: payload.clone(),
        });
        if watched {
            change.rows.push(put("watches", "comment", payload));
        }
        store.commit(&change).unwrap();
        if watched {
            let mut change = Commit::new("confirmed", json!(null), &store.load().unwrap());
            change.transitions.extend([
                EffectTransition {
                    key: "comment".into(),
                    from: tl3_store::EffectState::Intended,
                    to: tl3_store::EffectState::Sent,
                },
                EffectTransition {
                    key: "comment".into(),
                    from: tl3_store::EffectState::Sent,
                    to: tl3_store::EffectState::Confirmed,
                },
            ]);
            store.commit(&change).unwrap();
        }
        assert!(
            !store.can_retire_worker_session("0").unwrap(),
            "comment pending={}: session is still needed",
            !watched
        );
    }
    db.stop();
}

#[test]
fn serialized_names_match_store_sql() {
    assert_eq!(
        json!(Phase::Closed(Outcome::Merged)),
        json!({"Closed":"Merged"})
    );
    assert_eq!(json!(Upstream::Workers), "Workers");
    for upstream in [
        Upstream::GitHub,
        Upstream::Mergify,
        Upstream::PrismaBuild,
        Upstream::Workers,
        Upstream::Worktrees,
    ] {
        assert_eq!(json!(upstream), upstream.kind());
    }
    let output = json!(Output::Effect {
        node: NodeId(7),
        seq: 1,
        kind: EffectKind::StopRun,
        head: None
    });
    assert_eq!(output["Effect"]["node"], 7);
    assert_eq!(
        json!(Output::Notice {
            node: NodeId(7),
            notice: tl3_kernel::Notice::Incident { needs_rob: false }
        })["Notice"]["node"],
        7
    );
    assert_eq!(
        json!(Output::Comment {
            node: NodeId(7),
            kind: tl3_controller::executor::CommentKind::Classify
        })["Comment"]["node"],
        7
    );
}

#[test]
fn completion_and_look_deduplicate_old_inbox_formats() {
    let mut db = support::Database::start();
    for mode in ["completion", "watch", "look"] {
        let settings = db.create(mode);
        let root = tempfile::tempdir().unwrap();
        let mut c = open(
            &settings,
            root.path(),
            Recorder {
                observation: Some(progress()),
                ..Default::default()
            },
        );
        let key = worker_key(&mut c);
        let digest = {
            use sha2::Digest;
            format!(
                "{:x}",
                sha2::Sha256::digest(serde_json::to_vec(&progress()).unwrap())
            )
        };
        let old = match mode {
            "completion" => format!("{key}:00000000"),
            "watch" => format!("{key}:00000000:{digest}"),
            _ => format!("external:look:{key}:0:{digest}"),
        };
        let mut store = Store::open(&settings).unwrap();
        let mut change = Commit::new("old-inbox", json!(null), &store.load().unwrap());
        change.transitions.push(EffectTransition {
            key: key.clone(),
            from: tl3_store::EffectState::Intended,
            to: tl3_store::EffectState::Sent,
        });
        change.rows.push(put("inbox", &old, json!(progress())));
        store.commit(&change).unwrap();
        c.look_key(&key).unwrap();
        let inbox: Vec<_> = c
            .snapshot()
            .unwrap()
            .rows
            .into_iter()
            .filter(|r| r.table == "inbox")
            .collect();
        assert_eq!(inbox.len(), 1, "legacy {mode} row must deduplicate");
        assert!(c.drive_once().unwrap());
        let snapshot = store.load().unwrap();
        let mut change = Commit::new("old-found-state", json!(null), &snapshot);
        let row = snapshot
            .rows
            .iter()
            .find(|r| r.table == "allowances" && r.id == key)
            .unwrap();
        let mut value = row.value.clone();
        value["found"] = Value::Null;
        change.rows.push(put("allowances", &key, value));
        store.commit(&change).unwrap();
        c.look_key(&key).unwrap();
        assert!(
            !c.snapshot()
                .unwrap()
                .rows
                .iter()
                .any(|r| r.table == "inbox"),
            "consumed legacy {mode} evidence must stay consumed"
        );
    }
    db.stop();
}

#[test]
fn incidents_store_structured_signature() {
    let mut db = support::Database::start();
    let settings = db.create("structured_incident");
    let root = tempfile::tempdir().unwrap();
    let mut c = open(
        &settings,
        root.path(),
        Recorder {
            unreadable: true,
            ..Default::default()
        },
    );
    let key = worker_key(&mut c);
    c.look_key(&key).unwrap();
    c.look_key(&key).unwrap();
    let incident = c
        .snapshot()
        .unwrap()
        .rows
        .into_iter()
        .find(|r| r.table == "incidents")
        .unwrap();
    assert_eq!(
        incident.value["signature"],
        json!({"upstream":"Workers", "class":"transport"})
    );
    assert!(
        incident
            .id
            .split('.')
            .next()
            .unwrap()
            .parse::<u64>()
            .is_ok()
    );
    assert_eq!(incident.value["needs_rob"], false);
    db.stop();
}

#[test]
fn steering_creates_allowance_with_classifier() {
    let mut db = support::Database::start();
    let settings = db.create("steering_allowance");
    let root = tempfile::tempdir().unwrap();
    let mut c = open(&settings, root.path(), Recorder::default());
    c.observe(
        "comment",
        Observation::GitHubComment {
            repository: repository(),
            issue: 12,
            comment: tl3_github::Comment {
                id: 55,
                body: "Please update the greeting".into(),
                user: tl3_github::Actor {
                    id: 42,
                    login: "rob".into(),
                },
                created_at: chrono::DateTime::UNIX_EPOCH,
                updated_at: chrono::DateTime::UNIX_EPOCH,
                issue_url: "https://api.github.com/repos/owner/sandbox/issues/12".into(),
            },
        },
    )
    .unwrap();
    assert!(c.drive_once().unwrap());
    assert!(c.drive_once().unwrap());
    let snapshot = c.snapshot().unwrap();
    let effect = snapshot
        .pending_effects
        .iter()
        .find(|e| e.key.starts_with("classify:"))
        .unwrap();
    assert!(
        Store::open(&settings)
            .unwrap()
            .worker_watches()
            .unwrap()
            .iter()
            .any(|(key, _)| key == &effect.key),
        "controller watches must match the store SQL contract"
    );
    assert!(
        snapshot
            .rows
            .iter()
            .any(|r| r.table == "allowances" && r.id == effect.key),
        "classifier must receive allowance in its creation transaction"
    );
    db.stop();
}
