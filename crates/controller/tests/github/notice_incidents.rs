use super::*;
use sha2::Digest;
use tl3_controller::executor::Completion;
use tl3_kernel::{EffectKind, Event, Reply, Role};

#[derive(Default)]
struct Faults {
    reject: bool,
    incident: bool,
    kind: String,
}
struct WriterFault(Arc<Mutex<Faults>>);
impl Writer for WriterFault {
    fn draft(&mut self, key: &str, input: &WriteRequest) -> tl3_controller::Result<WriterReply> {
        let f = self.0.lock().unwrap();
        let targeted = match f.kind.as_str() {
            "question" | "cleanup" => input.kind == MessageKind::Question,
            "summary" => input.kind == MessageKind::Summary,
            "note" => input.kind == MessageKind::Note && !key.starts_with("incident:"),
            "dropped" => input.facts["change_quote"].is_string(),
            _ => false,
        };
        if (f.reject && targeted) || (f.incident && key.starts_with("incident:")) {
            return Ok(WriterReply::Text(adapters::workers::writer::TextDraft {
                text: "The leaf in workers/src/private.rs failed.".into(),
            }));
        }
        FakeWriter.draft(key, input)
    }
}
struct Complaint(adapters::workers::Fake);
impl Executor for Complaint {
    fn execute(&mut self, d: &Delivery) -> tl3_controller::Result<Completion> {
        if d.is_classifier() {
            Ok(Completion::confirmed(vec![Observation::Classified {
                key: d.key.clone(),
                reply: Some(
                    json!({"kind":"complaint","summary":"The result needs a correction.","revert":false}),
                ),
            }]))
        } else {
            self.0.execute(d)
        }
    }
    fn reconcile(&mut self, d: &Delivery) -> tl3_controller::Result<Reconciliation> {
        self.0.reconcile(d)
    }
}
struct SplitPlan(adapters::workers::Fake);
impl Executor for SplitPlan {
    fn execute(&mut self, d: &Delivery) -> tl3_controller::Result<Completion> {
        if matches!(
            d.output,
            Out::Effect {
                kind: EffectKind::StartRun(Role::Plan),
                ..
            }
        ) {
            Ok(Completion::confirmed(vec![Observation::Kernel(
                Event::Reply {
                    node: d.node(),
                    generation: d.generation,
                    reply: Reply::PlanSplit,
                },
            )]))
        } else {
            self.0.execute(d)
        }
    }
    fn reconcile(&mut self, d: &Delivery) -> tl3_controller::Result<Reconciliation> {
        self.0.reconcile(d)
    }
}
struct CleanupFailure {
    fake: adapters::worktrees::Fake,
    failed: std::path::PathBuf,
}
impl Executor for CleanupFailure {
    fn execute(&mut self, d: &Delivery) -> tl3_controller::Result<Completion> {
        if matches!(
            d.output,
            Out::Effect {
                kind: EffectKind::RemoveWorktree,
                ..
            }
        ) && !self.failed.exists()
        {
            fs::write(&self.failed, "failed once").unwrap();
            return Ok(Completion::permanent("permission"));
        }
        self.fake.execute(d)
    }
    fn reconcile(&mut self, d: &Delivery) -> tl3_controller::Result<Reconciliation> {
        self.fake.reconcile(d)
    }
}
fn open(
    settings: &tl3_config::Store,
    root: &Path,
    uri: &str,
    faults: Arc<Mutex<Faults>>,
) -> Controller {
    let mut registry = adapters::simulation(root).unwrap();
    let ask = faults.lock().unwrap().kind == "question";
    if faults.lock().unwrap().kind == "cleanup" {
        registry.register(
            Upstream::Worktrees,
            CleanupFailure {
                fake: adapters::worktrees::Fake::new(root),
                failed: root.join("cleanup-failed"),
            },
        );
    }
    if faults.lock().unwrap().kind == "filing" {
        registry.register(
            Upstream::Workers,
            Complaint(adapters::workers::Fake::new(root)),
        );
    }
    if ask {
        registry.register(
            Upstream::Workers,
            SplitPlan(adapters::workers::Fake::new(root)),
        );
    }
    let mut boot = bootstrap();
    boot.operations = Some(tl3_config::OperationsIssue {
        owner: "owner".into(),
        name: "tl3".into(),
        installation_id: 3,
        number: 17,
    });
    registry.register(
        Upstream::GitHub,
        adapters::github::Adapter::new(
            BTreeMap::from([(
                3,
                client_with_pem(uri, fs::read_to_string(root.join("mock-app.pem")).unwrap()),
            )]),
            WriterFault(faults),
            0,
            Policy::default(),
            root,
            boot.operations.clone(),
        )
        .unwrap()
        .with_repositories(&repositories())
        .unwrap(),
    );
    let mut store = Store::open(settings).unwrap();
    if let Ok(point) = env::var("TL3_NOTICE_CUT") {
        let root = root.to_path_buf();
        store.set_boundary_hook(move |actual, key, event| {
            let stable = if key.starts_with("delivery:") {
                format!("{}:{}:{}", event["effect"], event["from"], event["to"])
            } else {
                key.into()
            };
            cut(&root, &point, &format!("{actual:?}"), &stable);
        });
    }
    Controller::open(store, registry, boot).unwrap()
}
pub(super) fn run(db: &mut support::Database) {
    let mut failures = 0;
    for name in [
        "rejected_ask",
        "cleanup_ask",
        "question_403",
        "question_retry_rejected",
        "question_401",
        "question_404",
        "question_410",
        "question_422",
        "stale_ask",
        "probe_ask",
        "ask_state",
        "status_ask",
        "question_dialogue",
        "lost_summary",
        "incident_no_loop",
        "dropped_change",
        "lost_note",
        "filing_403",
    ] {
        let settings = db.create(name);
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            scenario(&settings, name, "none")
        }))
        .is_err()
        {
            eprintln!("test notice_{name} ... FAILED");
            failures += 1;
        } else {
            println!("test notice_{name} ... ok");
        }
    }
    assert_eq!(failures, 0, "notice incident scenarios failed");
    for name in [
        "rejected_ask",
        "cleanup_ask",
        "question_403",
        "incident_no_loop",
        "probe_ask",
        "lost_summary",
        "dropped_change",
        "lost_note",
        "filing_403",
    ] {
        for point in [
            "EventWritten",
            "RowsWritten",
            "EffectsWritten",
            "TransitionsWritten",
            "CreditWritten",
            "BeforeCommit",
            "AfterCommit",
            "BeforeDispatch",
            "AfterDispatch",
            "BeforeReconcile",
            "AfterReconcile",
        ] {
            scenario(
                &db.create(&format!("notice_cut_{name}_{point}")),
                name,
                point,
            );
            println!("test notice_{name}_retry_{point} ... ok");
        }
    }
}
fn scenario(settings: &tl3_config::Store, name: &str, point: &str) {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("mock-app.pem"), PEM.as_bytes()).unwrap();
    let api = Server::new();
    let kind = match name {
        name if name.starts_with("question_")
            || matches!(
                name,
                "rejected_ask" | "stale_ask" | "probe_ask" | "ask_state" | "status_ask"
            ) =>
        {
            "question"
        }
        "cleanup_ask" => "cleanup",
        "dropped_change" => "dropped",
        "lost_note" => "note",
        "filing_403" => "filing",
        _ => "summary",
    };
    let faults = Arc::new(Mutex::new(Faults {
        reject: !name.starts_with("question_4"),
        incident: name == "incident_no_loop",
        kind: kind.into(),
    }));
    if name == "question_retry_rejected" {
        api.state.lock().unwrap().question_status = Some(403);
        faults.lock().unwrap().reject = false;
    }
    if let Some(status) = name.strip_prefix("question_4") {
        api.state.lock().unwrap().question_status = Some(format!("4{status}").parse().unwrap());
    }
    if name == "dropped_change" {
        let mut c = steering::stopping_replacement(settings, root.path(), &api);
        let snapshot = c.snapshot().unwrap();
        let head = c.world().unwrap().nodes[0].head.unwrap();
        let sha = snapshot
            .interned
            .iter()
            .find(|v| v.kind == tl3_store::InternKind::Commit && v.id == head.0 as i64)
            .unwrap()
            .value
            .clone();
        c.observe(
            "notice-merge",
            Observation::Merged {
                node: NodeId(0),
                sha,
                checks_passed: true,
            },
        )
        .unwrap();
    }
    let mut c = open(settings, root.path(), &api.uri(), faults.clone());
    if name != "dropped_change" {
        c.intake(labelled()).unwrap();
    }
    let is_ask = matches!(kind, "question" | "cleanup");
    let question_world = if is_ask {
        for _ in 0..100 {
            if c.world().unwrap().nodes[0].question.is_some() {
                break;
            }
            assert!(c.drive_once().unwrap());
        }
        Some(c.world().unwrap())
    } else {
        None
    };
    drain(&mut c);
    if kind == "filing" {
        api.state.lock().unwrap().forbidden_filing = true;
        let mut comment = fixture(true)["comment"].clone();
        comment["id"] = json!(90000);
        comment["body"] = json!("The result needs a correction.");
        c.observe(
            "file-correction",
            Observation::GitHubComment {
                repository: bootstrap().repos.remove(0),
                issue: 12,
                comment: serde_json::from_value(comment).unwrap(),
            },
        )
        .unwrap();
        drain(&mut c);
    }
    let snapshot = c.snapshot().unwrap();
    let failed = Store::open(settings)
        .unwrap()
        .effects()
        .unwrap()
        .into_iter()
        .find(|e| {
            e.state == tl3_store::EffectState::Failed
                && e.payload["output"]["Effect"].is_null()
                && !e.key.starts_with("incident:")
        });
    assert!(
        failed.is_some(),
        "{name}: the rejected notice must finish Failed"
    );
    let failed = failed.unwrap();
    let incident = snapshot.rows.iter().find(|r| {
        r.table == "incidents" && r.value["open"] == true && r.value["effect"] == failed.key
    });
    assert!(
        incident.is_some(),
        "{name}: failed notice must raise a durable incident naming its delivery"
    );
    let incident = incident.unwrap();
    let incident_id = incident.id.clone();
    let detail = incident.value.to_string();
    assert!(detail.contains("Add a blue button"));
    let notice_kind = match kind {
        "question" | "cleanup" => "question",
        "filing" => "replacement filing",
        "dropped" => "dropped-change note",
        "note" => "note",
        "summary" => "closing summary",
        _ => unreachable!(),
    };
    assert!(
        incident.value["summary"]
            .as_str()
            .unwrap()
            .contains(&format!("The {notice_kind} could not be posted.")),
        "{name}: incident must name the notice kind"
    );
    assert!(detail.contains(match name {
        "question_401" | "question_403" | "question_retry_rejected" | "filing_403" => "permission",
        "question_404" | "question_410" => "not_found",
        "question_422" => "invalid_request",
        _ => "message_rejected",
    }));
    if is_ask {
        let status =
            tl3_controller::status::load(&mut tl3_store::Reader::open(settings).unwrap()).unwrap();
        assert!(
            status.requests[0].waiting_on.contains("failed"),
            "unposted question must show its failed post, not an unseen answer"
        );
    }
    if name == "status_ask" {
        let status =
            tl3_controller::status::load(&mut tl3_store::Reader::open(settings).unwrap()).unwrap();
        assert!(
            status.requests[0].waiting_on.contains("never posted"),
            "status must say that the open question was never posted"
        );
    }
    if name == "ask_state" {
        assert_eq!(
            c.world().unwrap(),
            *question_world.as_ref().unwrap(),
            "a failed question post must leave the entire kernel world untouched"
        );
    }
    if name == "question_dialogue" {
        let mut comment = fixture(true)["comment"].clone();
        comment["id"] = json!(90002);
        comment["body"] = json!("Stop this request.");
        c.observe(
            "question-dialogue",
            Observation::GitHubComment {
                repository: bootstrap().repos.remove(0),
                issue: 12,
                comment: serde_json::from_value(comment).unwrap(),
            },
        )
        .unwrap();
        let classify = loop {
            if let Some(effect) = c
                .snapshot()
                .unwrap()
                .pending_effects
                .into_iter()
                .find(|e| e.payload["steering"]["Classify"].is_object())
            {
                break effect;
            }
            assert!(c.drive_once().unwrap());
        };
        let question =
            &classify.payload["steering"]["Classify"]["inputs"]["state"]["open_question"];
        assert_eq!(
            question["kind"], "needs_splitting",
            "failed posting must preserve the classifier's original question kind"
        );
        assert_eq!(question["answers"], json!(["change_request", "stop"]));
        return;
    }
    if name == "stale_ask" {
        c.observe(
            "stop-before-clear",
            Observation::Kernel(Event::Command {
                node: NodeId(0),
                id: 77,
                command: tl3_kernel::Command::Stop,
            }),
        )
        .unwrap();
        drain(&mut c);
        assert!(c.world().unwrap().nodes[0].question.is_none());
    }
    if name == "incident_no_loop" {
        assert_eq!(
            snapshot
                .rows
                .iter()
                .filter(|r| r.table == "incidents")
                .count(),
            1
        );
        let status =
            tl3_controller::status::load(&mut tl3_store::Reader::open(settings).unwrap()).unwrap();
        assert!(
            status.requests[0]
                .details
                .open_incidents
                .iter()
                .any(|s| s.contains("incident") && s.contains("post")),
            "failed incident post must be visible on status"
        );
        let before = c.snapshot().unwrap();
        for _ in 0..3 {
            assert!(!c.drive_once().unwrap());
        }
        assert_eq!(
            before,
            c.snapshot().unwrap(),
            "failed incident must not loop"
        );
    }
    if name == "question_retry_rejected" {
        clear_incident(&mut c, &incident_id);
        drain(&mut c);
        let snapshot = c.snapshot().unwrap();
        let incidents: Vec<_> = snapshot
            .rows
            .iter()
            .filter(|r| r.table == "incidents")
            .collect();
        assert_eq!(
            incidents.len(),
            2,
            "a rejected retry raises exactly one later incident"
        );
        let second = incidents.iter().find(|r| r.id != incident_id).unwrap();
        assert_eq!(second.value["open"], true);
        assert_eq!(
            second.value["needs_rob"], true,
            "a later notice incident requires Rob"
        );
        let retry_key = second.value["effect"].as_str().unwrap().to_owned();
        assert_eq!(api.state.lock().unwrap().rejected_question_posts, 2);
        for _ in 0..3 {
            c.look_key(&retry_key).unwrap();
            drain(&mut c);
        }
        drop(c);
        let mut c = open(settings, root.path(), &api.uri(), faults);
        c.look_key(&retry_key).unwrap();
        drain(&mut c);
        let after = c.snapshot().unwrap();
        assert_eq!(
            after.rows.iter().filter(|r| r.table == "incidents").count(),
            2
        );
        assert!(
            after.rows.iter().any(|r| r.table == "incidents"
                && r.id == second.id
                && r.value["open"] == true
                && r.value["needs_rob"] == true),
            "a passing probe must not clear the second incident, including after restart"
        );
        assert_eq!(
            api.state.lock().unwrap().rejected_question_posts,
            2,
            "a passing probe must not cause a third post"
        );
        return;
    }
    faults.lock().unwrap().reject = false;
    faults.lock().unwrap().incident = false;
    api.state.lock().unwrap().forbidden_question = false;
    api.state.lock().unwrap().question_status = None;
    api.state.lock().unwrap().forbidden_filing = false;
    let before_retry = c.world().unwrap();
    if point == "none" {
        if name == "probe_ask" {
            c.look_key(&failed.key).unwrap();
        } else {
            clear_incident(&mut c, &incident_id);
        }
    }
    drop(c);
    if point != "none" {
        let config = root.path().join("settings.json");
        fs::write(&config, serde_json::to_vec(&json!({"host":settings.host,"port":settings.port,"database":settings.database,"user":settings.user,"password_file":settings.password_file,"connect_timeout_secs":settings.connect_timeout_secs})).unwrap()).unwrap();
        let mut crashes = 0;
        loop {
            let mut connection = postgres::Config::new()
                .host(&settings.host)
                .port(settings.port)
                .user(&settings.user)
                .dbname(&settings.database)
                .password("tl3-disposable-test-password")
                .connect(postgres::NoTls)
                .unwrap();
            connection
                .query_one("SELECT pg_advisory_lock($1)", &[&0x544c33434f4e5452_i64])
                .unwrap();
            connection
                .query_one("SELECT pg_advisory_unlock($1)", &[&0x544c33434f4e5452_i64])
                .unwrap();
            drop(connection);
            let status = Command::new(env::current_exe().unwrap())
                .arg("--notice-retry-child")
                .arg(&config)
                .arg(root.path())
                .arg(api.uri())
                .arg(kind)
                .arg(&incident_id)
                .arg(&failed.key)
                .env("TL3_NOTICE_CUT", point)
                .env(
                    "TL3_NOTICE_PROBE",
                    if name == "probe_ask" { "1" } else { "0" },
                )
                .status()
                .unwrap();
            if status.success() {
                break;
            }
            assert_eq!(status.code(), Some(86));
            crashes += 1;
            assert!(crashes < 200, "notice retry crash replay must converge");
        }
        assert!(crashes > 0, "the requested save point must be reached");
    }
    let mut c = open(settings, root.path(), &api.uri(), faults.clone());
    drain(&mut c);
    let count = |api: &Server| {
        let state = api.state.lock().unwrap();
        let records = if kind == "filing" {
            &state.linked
        } else {
            &state.comments
        };
        records
            .iter()
            .filter(|v| {
                v["body"]
                    .as_str()
                    .unwrap()
                    .contains(&GitHub::marker(&failed.key))
            })
            .count()
    };
    if name != "stale_ask" {
        assert!(
            !c.snapshot()
                .unwrap()
                .rows
                .iter()
                .any(|r| r.table == "notice_incidents" && r.id == failed.key),
            "confirmed retry must delete its notice incident counter"
        );
    }
    let expected_posts = usize::from(name != "stale_ask");
    assert_eq!(
        count(&api),
        expected_posts,
        "retry posts the original notice exactly once unless its question is stale"
    );
    if kind == "filing" {
        assert_eq!(
            c.world().unwrap().nodes[0],
            before_retry.nodes[0],
            "retrying a correction filing leaves its closed request unchanged"
        );
    } else {
        assert_eq!(
            c.world().unwrap(),
            before_retry,
            "clearing a notice incident must not advance the kernel"
        );
    }
    if is_ask && name != "stale_ask" {
        assert_eq!(
            c.world().unwrap(),
            question_world.unwrap(),
            "failed posting and recovery must preserve the original question and kernel world"
        );
        let status =
            tl3_controller::status::load(&mut tl3_store::Reader::open(settings).unwrap()).unwrap();
        assert_eq!(status.requests[0].waiting_on, "Rob's answer");
    }
    assert!(
        !c.snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "incidents" && r.id == incident_id && r.value["open"] == true),
        "clearing must close the notice incident"
    );
    drop(c);
    let mut c = open(settings, root.path(), &api.uri(), faults);
    drain(&mut c);
    assert_eq!(
        count(&api),
        expected_posts,
        "restart must not duplicate a retried notice"
    );
    assert!(c.world().unwrap().consistent());
    if kind == "cleanup" {
        let world = c.world().unwrap();
        let question = world.nodes[0].question.unwrap();
        assert_eq!(question.kind, tl3_kernel::QKind::CleanupFailed);
        c.observe(
            "retry-cleanup-step",
            Observation::Kernel(Event::Command {
                node: NodeId(0),
                id: world.nodes[0].last_cmd + 1,
                command: tl3_kernel::Command::Answer {
                    question: question.id,
                    answer: tl3_kernel::Answer::Retry,
                },
            }),
        )
        .unwrap();
        drain(&mut c);
        assert_eq!(
            c.world().unwrap().nodes[0].phase,
            Phase::Closed(Outcome::Merged),
            "Retry must retry the cleanup step after its question has been posted"
        );
    }
    if kind == "dropped" {
        let delivery: Delivery = serde_json::from_value(failed.payload.clone()).unwrap();
        let Some(tl3_controller::steering::Task::ReplacementDropped { text, .. }) =
            delivery.steering
        else {
            panic!("dropped notice must retain Rob's change");
        };
        let state = api.state.lock().unwrap();
        let body = state
            .comments
            .iter()
            .find(|v| {
                v["body"]
                    .as_str()
                    .unwrap()
                    .contains(&GitHub::marker(&failed.key))
            })
            .unwrap()["body"]
            .as_str()
            .unwrap();
        for line in text.lines() {
            assert!(
                body.contains(&format!("    {line}")),
                "retried dropped-change note must keep Rob's quote in its body: {body}"
            );
        }
        assert!(body.contains("merged before") && body.contains("again"));
        assert!(
            !c.snapshot()
                .unwrap()
                .rows
                .iter()
                .any(|r| r.table == "replacement")
        );
    }
}

fn cut(root: &Path, selected: &str, actual: &str, key: &str) {
    if selected == actual {
        let marker = root.join(format!(
            "notice-cut-{actual}-{:x}",
            Sha256::digest(key.as_bytes())
        ));
        if !marker.exists() {
            fs::write(marker, key).unwrap();
            std::process::exit(86);
        }
    }
}

fn clear_incident(c: &mut Controller, id: &str) {
    let mut comment = fixture(true)["comment"].clone();
    comment["id"] = json!(90001);
    comment["body"] = json!(format!("clear incident {id}"));
    c.observe(
        "retry-notice",
        Observation::GitHubComment {
            repository: Repository {
                owner: "owner".into(),
                name: "tl3".into(),
                installation: 3,
            },
            issue: 17,
            comment: serde_json::from_value(comment).unwrap(),
        },
    )
    .unwrap();
}
pub(super) fn child(args: &[String]) {
    let settings = serde_json::from_slice(&fs::read(&args[2]).unwrap()).unwrap();
    let root = Path::new(&args[3]);
    let selected = env::var("TL3_NOTICE_CUT").unwrap();
    let faults = Arc::new(Mutex::new(Faults {
        kind: args[5].clone(),
        ..Faults::default()
    }));
    let mut c = open(&settings, root, &args[4], faults);
    if env::var("TL3_NOTICE_PROBE").as_deref() == Ok("1") {
        c.look_key(&args[7]).unwrap();
    } else {
        clear_incident(&mut c, &args[6]);
    }
    for _ in 0..500 {
        let progressed = c
            .drive_once_with(|point, key| {
                if selected.ends_with("Reconcile") && point == Boundary::AfterDispatch {
                    cut(root, "lose-response", "lose-response", key);
                }
                cut(root, &selected, &format!("{point:?}"), key);
            })
            .unwrap();
        if !progressed {
            return;
        }
    }
    panic!("notice retry child did not settle");
}
