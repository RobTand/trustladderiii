//! #604: evidence at the controller boundary survives dispatch and restart.
use super::*;
use crate::executor::{Executor, Reconciliation, Upstream};
use std::sync::{Arc, Mutex};

struct Refusing {
    fake: adapters::github::Fake,
    class: &'static str,
    status: Option<u16>,
    notices: Arc<Mutex<Vec<Delivery>>>,
}
impl Executor for Refusing {
    fn execute(&mut self, delivery: &Delivery) -> Result<Completion> {
        if matches!(
            delivery.output,
            DeliveryOutput::Effect {
                kind: tl3_kernel::EffectKind::OpenPr,
                ..
            }
        ) {
            if self.class == "pr_obligation" {
                let mut d = delivery.clone();
                d.evidence = Some(evidence::Evidence {
                    goals: vec![json!({"id":"branch","stage":"publishing",
                    "publication":{"branch":"prismabuild-2646"}})],
                    ..Default::default()
                });
                evidence::validate_publication(&d, "prismabuild-2646-tl3-0", false, "body")?;
            }
            return Err(match self.status {
                Some(status) => tl3_github::Error::Http { status }.into(),
                None => Error::Permanent {
                    class: self.class,
                    source: "Bearer synthetic-secret; response body never stored".into(),
                },
            });
        }
        if matches!(
            delivery.output,
            DeliveryOutput::Notice {
                notice: tl3_kernel::Notice::Incident { .. },
                ..
            }
        ) {
            self.notices.lock().unwrap().push(delivery.clone());
        }
        self.fake.execute(delivery)
    }
    fn reconcile(&mut self, delivery: &Delivery) -> Result<Reconciliation> {
        self.fake.reconcile(delivery)
    }
}

fn refused(class: &'static str, status: Option<u16>) -> (Value, Delivery, Vec<Value>) {
    let mut db = store_support::Database::start();
    let settings = db.create("publication_diagnostic");
    let root = tempfile::tempdir().unwrap();
    let notices = Arc::new(Mutex::new(Vec::new()));
    let mut registry = adapters::simulation(root.path()).unwrap();
    registry.register(
        Upstream::GitHub,
        Refusing {
            fake: adapters::github::Fake::new(root.path()),
            class,
            status,
            notices: notices.clone(),
        },
    );
    let bootstrap = Bootstrap {
        repos: vec![Repository {
            owner: "owner".into(),
            name: "sandbox".into(),
            installation: 3,
        }],
        rob: 42,
        slots: 1,
        pool: 1,
        fail_fast: false,
        operations: None,
    };
    let mut c =
        Controller::open(Store::open(&settings).unwrap(), registry, bootstrap.clone()).unwrap();
    c.intake(LabelledIssue {
        delivery: "label".into(),
        owner: "owner".into(),
        repo: "sandbox".into(),
        installation: 3,
        number: 2646,
        title: "Publish a fix".into(),
        actor: 42,
        label: "tl3".into(),
        labels: vec![],
    })
    .unwrap();
    for _ in 0..300 {
        if !c.drive_once().unwrap() {
            break;
        }
    }
    let snapshot = c.store.load().unwrap();
    let incident = snapshot
        .rows
        .iter()
        .find(|r| r.table == "incidents")
        .expect("permanent refusal needs an incident")
        .value
        .clone();
    let notice = notices
        .lock()
        .unwrap()
        .first()
        .expect("incident posted")
        .clone();
    let events = c
        .store
        .events_after(tl3_store::Revision::try_from(0).unwrap(), 1000)
        .unwrap()
        .into_iter()
        .map(|e| e.payload)
        .collect::<Vec<_>>();
    drop(c);
    let mut restarted = Controller::open(
        Store::open(&settings).unwrap(),
        adapters::simulation(root.path()).unwrap(),
        bootstrap,
    )
    .unwrap();
    assert_eq!(
        restarted
            .store
            .load()
            .unwrap()
            .rows
            .iter()
            .find(|r| r.table == "incidents")
            .unwrap()
            .value,
        incident
    );
    (incident, notice, events)
}

#[test]
fn t604_openpr_branch_refusal_names_class_branch_mismatch_and_effect_in_notice_and_journal() {
    let (incident, notice, events) = refused("pr_obligation", None);
    let text = notice.note.as_ref().unwrap();
    for required in [
        "pr_obligation",
        "OpenPr",
        "branch mismatch",
        "prismabuild-2646-tl3-0",
        "effect:0:",
    ] {
        assert!(text.contains(required), "missing {required}: {text}");
    }
    assert_eq!(incident["cause"], "tl3");
    assert!(
        events
            .iter()
            .any(|e| e["diagnostic"]["class"] == "pr_obligation")
    );
    let input = adapters::github::writer_input(
        &notice,
        Some(tl3_kernel::Notice::Incident { needs_rob: true }),
        "Publish a fix",
        "request",
    )
    .unwrap();
    assert!(input.facts.to_string().contains("pr_obligation"));
}

#[test]
fn t604_every_local_publication_class_is_safe_distinct_and_tl3_owned() {
    for class in ["pr_evidence", "pr_policy"] {
        let (incident, notice, events) = refused(class, None);
        assert_eq!(incident["cause"], "tl3", "{incident}");
        assert!(notice.note.as_ref().unwrap().contains(class));
        let diagnostic = events
            .iter()
            .find_map(|e| e.get("diagnostic"))
            .expect("journal diagnostic");
        assert_eq!(diagnostic["class"], class);
        assert_eq!(diagnostic["origin"], "local_validation");
        assert!(diagnostic["status"].is_null());
        assert!(diagnostic["endpoint"].is_null());
        assert!(!format!("{incident:?}{notice:?}{events:?}").contains("synthetic-secret"));
    }
}

#[test]
fn t604_http_refusals_retain_status_without_inventing_an_endpoint() {
    // The fake has no operation context. Actual GitHub calls supply a typed endpoint.
    for (status, class) in [(403, "permission"), (422, "invalid_request")] {
        let (incident, notice, events) = refused(class, Some(status));
        assert_eq!(incident["cause"], "upstream");
        let diagnostic = events
            .iter()
            .find_map(|e| e.get("diagnostic"))
            .expect("journal diagnostic");
        assert_eq!(diagnostic["status"], status);
        assert_eq!(diagnostic["class"], class);
        assert!(diagnostic["endpoint"].is_null());
        assert!(notice.note.unwrap().contains(&format!("HTTP {status}")));
    }
}

#[test]
fn t604_first_permanent_failure_is_not_called_recurring() {
    let (_, notice, _) = refused("pr_policy", None);
    assert!(!notice.note.unwrap().contains("recurring"));
}

#[test]
fn t604_historical_local_causes_are_not_guessed_to_be_github() {
    for class in ["pr_evidence", "pr_obligation", "pr_policy"] {
        assert_eq!(looks::Cause::of(Upstream::GitHub, class), looks::Cause::Tl3);
    }
}
