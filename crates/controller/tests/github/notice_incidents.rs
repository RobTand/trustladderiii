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
            "question" => input.kind == MessageKind::Question,
            "summary" => input.kind == MessageKind::Summary,
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
fn open(
    settings: &tl3_config::Store,
    root: &Path,
    uri: &str,
    faults: Arc<Mutex<Faults>>,
) -> Controller {
    let mut registry = adapters::simulation(root).unwrap();
    let ask = faults.lock().unwrap().kind == "question";
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
            BTreeMap::from([(3, client(uri))]),
            WriterFault(faults),
            0,
            Policy::default(),
            root,
            boot.operations.clone(),
        )
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
        "question_403",
        "lost_summary",
        "incident_no_loop",
        "dropped_change",
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
    for name in ["rejected_ask", "lost_summary", "dropped_change"] {
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
    let api = Server::new();
    let kind = match name {
        "rejected_ask" | "question_403" => "question",
        "dropped_change" => "dropped",
        _ => "summary",
    };
    let faults = Arc::new(Mutex::new(Faults {
        reject: name != "question_403",
        incident: name == "incident_no_loop",
        kind: kind.into(),
    }));
    if name == "question_403" {
        api.state.lock().unwrap().forbidden_question = true;
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
    drain(&mut c);
    let snapshot = c.snapshot().unwrap();
    let failed = Store::open(settings)
        .unwrap()
        .effects()
        .unwrap()
        .into_iter()
        .find(|e| e.state == tl3_store::EffectState::Failed && !e.key.starts_with("incident:"));
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
    assert!(detail.contains(if name == "question_403" {
        "permission"
    } else {
        "message_rejected"
    }));
    if kind == "question" {
        let status =
            tl3_controller::status::load(&mut tl3_store::Reader::open(settings).unwrap()).unwrap();
        assert!(
            status.requests[0].waiting_on.contains("failed"),
            "unposted question must wait on its failed post, not an unseen answer"
        );
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
    faults.lock().unwrap().reject = false;
    faults.lock().unwrap().incident = false;
    api.state.lock().unwrap().forbidden_question = false;
    let mut comment = fixture(true)["comment"].clone();
    comment["id"] = json!(90001);
    comment["body"] = json!(format!("clear incident {incident_id}"));
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
                .env("TL3_NOTICE_CUT", point)
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
        api.state
            .lock()
            .unwrap()
            .comments
            .iter()
            .filter(|v| {
                let s = v["body"].as_str().unwrap();
                match kind {
                    "question" => s.contains("recommend option"),
                    "dropped" => s.contains("Return goodbye instead of hello."),
                    _ => s.contains("merged"),
                }
            })
            .count()
    };
    // Check the original marker too, including summaries that posted before closing failed.
    let comments = api.state.lock().unwrap().comments.clone();
    assert!(
        comments.iter().any(|v| v["body"]
            .as_str()
            .unwrap()
            .contains(&GitHub::marker(&failed.key))),
        "retry must preserve the notice marker"
    );
    assert_eq!(
        count(&api),
        1,
        "retry posts the original notice exactly once"
    );
    drop(c);
    let mut c = open(settings, root.path(), &api.uri(), faults);
    drain(&mut c);
    assert_eq!(
        count(&api),
        1,
        "restart must not duplicate a retried notice"
    );
    assert!(c.world().unwrap().consistent());
    if kind == "dropped" {
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
pub(super) fn child(args: &[String]) {
    let settings = serde_json::from_slice(&fs::read(&args[2]).unwrap()).unwrap();
    let root = Path::new(&args[3]);
    let selected = env::var("TL3_NOTICE_CUT").unwrap();
    let faults = Arc::new(Mutex::new(Faults {
        kind: args[5].clone(),
        ..Faults::default()
    }));
    let mut c = open(&settings, root, &args[4], faults);
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
