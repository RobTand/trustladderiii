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
            } | DeliveryOutput::Comment {
                kind: executor::CommentKind::OperationsNote,
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
        operations: Some(tl3_config::OperationsIssue {
            owner: "owner".into(),
            name: "operations".into(),
            installation_id: 3,
            number: 81,
        }),
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
    if status == Some(404) {
        let pending = c
            .store
            .load()
            .unwrap()
            .pending_effects
            .into_iter()
            .find(|effect| {
                let delivery: Delivery = serde_json::from_value(effect.payload.clone()).unwrap();
                matches!(
                    delivery.output,
                    DeliveryOutput::Effect {
                        kind: tl3_kernel::EffectKind::OpenPr,
                        ..
                    }
                )
            })
            .expect("first 404 stays pending");
        c.look_key(&pending.key).unwrap();
        for _ in 0..300 {
            if !c.drive_once().unwrap() {
                break;
            }
        }
    }
    let snapshot = c.store.load().unwrap();
    let incident = snapshot
        .rows
        .iter()
        .find(|r| {
            r.table
                == if class == "pr_policy" {
                    "review_stops"
                } else {
                    "incidents"
                }
        })
        .expect("permanent refusal needs a retained stop or incident")
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
            .find(|r| r.table
                == if class == "pr_policy" {
                    "review_stops"
                } else {
                    "incidents"
                })
            .unwrap()
            .value,
        incident
    );
    (incident, notice, events)
}

#[test]
fn t604_openpr_branch_refusal_names_class_branch_mismatch_and_effect_in_notice_and_journal() {
    let (incident, notice, events) = refused("pr_obligation", None);
    assert_eq!(notice.issue, 81);
    assert_eq!(notice.repository.name, "operations");
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
        if class == "pr_policy" {
            assert_eq!(incident["step"], "publication_body");
            assert!(notice.note.as_ref().unwrap().contains("tl3_review"));
        } else {
            assert_eq!(incident["cause"], "tl3", "{incident}");
        }
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
    for (status, class) in [
        (403, "permission"),
        (404, "not_found"),
        (422, "invalid_request"),
    ] {
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

#[test]
fn t604_typed_http_operations_have_only_status_and_templated_endpoint() {
    let fixture = git_tests::Repo::new();
    let mut delivery = fixture.delivery();
    delivery.output = DeliveryOutput::Effect {
        node: NodeId(0),
        seq: 8,
        kind: tl3_kernel::EffectKind::OpenPr,
        head: None,
    };
    for (status, class) in [
        (403, "permission"),
        (404, "not_found"),
        (422, "invalid_request"),
    ] {
        let error = publication_failure::open_pr(tl3_github::Error::Http { status });
        let diagnostic =
            publication_failure::Diagnostic::capture(&delivery, Some(&error), class).unwrap();
        let diagnostic = json!(diagnostic);
        assert_eq!(diagnostic["status"], status);
        assert_eq!(diagnostic["endpoint"], "POST /repos/{owner}/{repo}/pulls");
        assert_eq!(diagnostic["origin"], "github_http");
        assert_eq!(
            error.is_github_not_found(),
            status == 404,
            "preserve the first-404 retry decision"
        );
    }
}

#[test]
fn t604_unknown_historical_refusals_do_not_invent_cause_or_http_context() {
    let fixture = git_tests::Repo::new();
    let diagnostic =
        publication_failure::Diagnostic::capture(&fixture.delivery(), None, "unknown").unwrap();
    assert!(matches!(
        diagnostic.origin,
        publication_failure::Origin::Unavailable
    ));
    assert!(diagnostic.status.is_none());
    assert!(diagnostic.endpoint.is_none());
    assert!(diagnostic.render().contains("historical cause unavailable"));
}

#[test]
fn t604_known_permanent_context_classes_remain_distinguishable() {
    let fixture = git_tests::Repo::new();
    for class in [
        "invalid_branch",
        "invalid_commit",
        "invalid_remote",
        "invalid_worktree_path",
        "invalid_token",
        "invalid_response",
        "missing_branch",
        "reserved_branch",
        "worktree_occupied",
        "dirty_worktree",
        "worktree_recovery",
        "actions_write_required",
        "actions_run_expired",
        "actions_refused",
        "setup",
    ] {
        let diagnostic =
            publication_failure::Diagnostic::capture(&fixture.delivery(), None, class).unwrap();
        assert_eq!(diagnostic.class, class);
        assert!(diagnostic.status.is_none());
        assert!(diagnostic.endpoint.is_none());
    }
}

#[test]
fn t604_authentication_refusal_names_token_endpoint_instead_of_pr_endpoint() {
    let fixture = git_tests::Repo::new();
    let error = publication_failure::open_pr(tl3_github::Error::InstallationToken {
        source: Box::new(tl3_github::Error::Http { status: 403 }),
    });
    let diagnostic =
        publication_failure::Diagnostic::capture(&fixture.delivery(), Some(&error), "permission")
            .unwrap();
    assert_eq!(
        diagnostic.endpoint.as_deref(),
        Some("POST /app/installations/{installation}/access_tokens")
    );
    assert_eq!(diagnostic.status, Some(403));
}
