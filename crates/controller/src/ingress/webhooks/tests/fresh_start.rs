use super::*;

struct FreshStartHistory;
impl Fetcher for FreshStartHistory {
    fn fetch(
        &self,
        since: DateTime<Utc>,
        _world_start: DateTime<Utc>,
        _snapshot: tl3_store::Snapshot,
        bootstrap: &Bootstrap,
    ) -> Result<CatchUpBatch> {
        let old: DateTime<Utc> = "2026-01-01T00:00:00Z".parse().unwrap();
        println!("FRESH_START_FETCH_SINCE {since}");
        let changes: tl3_github::RepoChanges = serde_json::from_value(json!({
            "repository": "synthetic/project",
            "issues": [{"number": 42, "title": "closed historical issue", "body": null, "state": "closed", "updated_at": old, "labels": [{"name":"tl3"}], "pull_request": null}],
            "labels": if since < old { json!([[42, {"id": 7, "event":"labeled", "actor":{"id":1,"login":"synthetic-rob"}, "label":{"name":"tl3"}, "created_at":old}]]) } else { json!([]) },
            "comments": if since < old { json!([{"id":8,"body":"tl3 approve","user":{"id":1,"login":"synthetic-rob"},"created_at":old,"updated_at":old,"issue_url":"https://api.github.com/repos/synthetic/project/issues/42"}]) } else { json!([]) },
            "pull_requests": [], "check_runs": [], "check_suites": []
        })).unwrap();
        let delivery = GitHubDelivery::CatchUp {
            repository: bootstrap.repos[0].clone(),
            changes: Box::new(changes),
        };
        Ok(CatchUpBatch {
            deliveries: vec![("synthetic-history".into(), serde_json::to_vec(&delivery)?)],
            cursor: Some("2026-11-08T05:00:00Z".parse().unwrap()),
        })
    }
}

#[test]
fn fresh_start_rejects_closed_history_and_old_rob_approval() {
    let mut database = database::Database::start();
    let settings = database.create("fresh_start_history");
    let mut store = Store::open(&settings).unwrap();
    let mut bootstrap = bootstrap();
    bootstrap.repos.push(Repository {
        owner: "synthetic".into(),
        name: "project".into(),
        installation: 1,
    });
    initialize_intake(&mut store, &bootstrap, &[], instant, |_, _| unreachable!()).unwrap();
    assert_eq!(
        store.ingress_cursor("github-catch-up").unwrap(),
        Some(json!(instant()))
    );
    let (sender, receiver) = mpsc::channel();
    fetch_catch_up(
        Arc::new(FreshStartHistory),
        store.reader().unwrap(),
        bootstrap.clone(),
        sender,
    )
    .join()
    .unwrap();
    let WebhookMessage::CaughtUp(_, batch) = receiver.recv().unwrap() else {
        panic!("expected caught-up result")
    };
    batch.unwrap().persist(&mut store).unwrap();
    let inputs: Vec<_> = store
        .pending_ingress()
        .unwrap()
        .iter()
        .flat_map(|stored| translate(replay(stored).unwrap(), &bootstrap))
        .collect();
    let admissions = inputs
        .iter()
        .filter(|input| matches!(input, Input::LabelledIssue(_)))
        .count();
    let approvals = inputs
        .iter()
        .filter(|input| {
            matches!(
                input,
                Input::Observation {
                    observation: Observation::GitHubComment { .. },
                    ..
                }
            )
        })
        .count();
    println!("FRESH_START_HISTORY_OBSERVED admissions={admissions} old_rob_approvals={approvals}");
    assert_eq!(
        (admissions, approvals),
        (0, 0),
        "fresh world must not replay closed historical intake or old Rob approval"
    );
    database.stop();
}

fn instant() -> DateTime<Utc> {
    "2026-11-08T04:00:00Z".parse().unwrap()
}

fn served() -> Bootstrap {
    let mut bootstrap = bootstrap();
    bootstrap.repos.push(Repository {
        owner: "synthetic".into(),
        name: "project".into(),
        installation: 1,
    });
    bootstrap
}

fn entry(number: u64) -> tl3_config::InitialIntake {
    tl3_config::InitialIntake {
        owner: "synthetic".into(),
        repo: "project".into(),
        number,
    }
}

fn current(number: u64) -> tl3_github::Issue {
    serde_json::from_value(json!({"number":number,"title":"synthetic issue","body":null,"state":"open","updated_at":instant(),"labels":[{"name":"tl3"}],"pull_request":null})).unwrap()
}

fn persisted_inputs(store: &mut Store, bootstrap: &Bootstrap) -> Vec<Input> {
    store
        .pending_ingress()
        .unwrap()
        .iter()
        .flat_map(|row| translate(replay(row).unwrap(), bootstrap))
        .collect()
}

#[test]
fn explicit_initial_intake_is_atomic_and_uses_clock_before_validation() {
    let mut database = database::Database::start();
    let settings = database.create("fresh_initial_explicit");
    let mut store = Store::open(&settings).unwrap();
    let captured = std::cell::Cell::new(false);
    let observer = settings.clone();
    initialize_intake(
        &mut store,
        &served(),
        &[entry(1455), entry(1574)],
        || {
            captured.set(true);
            instant()
        },
        |_, number| {
            assert!(
                captured.get(),
                "clock must be captured before any network read"
            );
            let mut read = Store::open(&observer).unwrap();
            assert!(read.ingress_cursor("github-catch-up").unwrap().is_none());
            assert!(read.pending_ingress().unwrap().is_empty());
            Ok(current(number))
        },
    )
    .unwrap();
    assert_eq!(
        store.ingress_cursor("github-catch-up").unwrap(),
        Some(json!(instant()))
    );
    let inputs = persisted_inputs(&mut store, &served());
    let admitted: Vec<_> = inputs
        .iter()
        .filter_map(|input| match input {
            Input::LabelledIssue(issue) => Some((issue.number, issue.delivery.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(
        admitted,
        [
            (1455, "initial-intake:synthetic/project:1455"),
            (1574, "initial-intake:synthetic/project:1574")
        ]
    );
    database.stop();
}

#[test]
fn invalid_initial_intake_fails_closed_without_partial_commit_or_secret() {
    let mut database = database::Database::start();
    for (index, reason) in [
        "closed",
        "unlabelled",
        "pr",
        "missing",
        "fetch failure",
        "wrong number",
        "unserved",
        "zero",
    ]
    .into_iter()
    .enumerate()
    {
        let settings = database.create(&format!("fresh_invalid_{index}"));
        let mut store = Store::open(&settings).unwrap();
        let mut bad = entry(1574);
        if reason == "unserved" {
            bad.owner = "unserved".into();
        }
        if reason == "zero" {
            bad.number = 0;
        }
        let error = initialize_intake(
            &mut store,
            &served(),
            &[entry(1455), bad],
            instant,
            |_, number| {
                if number == 1455 {
                    return Ok(current(number));
                }
                let mut issue = current(number);
                match reason {
                    "closed" => issue.state = "closed".into(),
                    "unlabelled" => issue.labels.clear(),
                    "pr" => {
                        issue.pull_request = Some(tl3_github::PullLink {
                            url: "https://synthetic.invalid/pull/1574".into(),
                        })
                    }
                    "wrong number" => issue.number = 999,
                    "missing" | "fetch failure" => {
                        return Err(Error::Invalid("synthetic-secret upstream response"));
                    }
                    _ => unreachable!(),
                }
                Ok(issue)
            },
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("initial_intake[1]"), "{reason}: {error}");
        assert!(
            error.contains(if reason == "zero" { "#0" } else { "#1574" }),
            "{error}"
        );
        assert!(!error.contains("synthetic-secret"), "{error}");
        assert!(
            store.ingress_cursor("github-catch-up").unwrap().is_none(),
            "{reason}"
        );
        assert!(store.pending_ingress().unwrap().is_empty(), "{reason}");
    }
    database.stop();
}

#[test]
fn restart_ignores_changed_list_and_never_calls_clock_or_fetch() {
    let mut database = database::Database::start();
    let settings = database.create("fresh_restart");
    let mut store = Store::open(&settings).unwrap();
    initialize_intake(
        &mut store,
        &served(),
        &[entry(1455)],
        instant,
        |_, number| Ok(current(number)),
    )
    .unwrap();
    let before = store.pending_ingress().unwrap();
    drop(store);
    let mut store = Store::open(&settings).unwrap();
    initialize_intake(
        &mut store,
        &served(),
        &[entry(1574)],
        || panic!("restart clock called"),
        |_, _| panic!("restart fetched initial intake"),
    )
    .unwrap();
    assert_eq!(store.pending_ingress().unwrap(), before);
    assert_eq!(
        store.ingress_cursor("github-catch-up").unwrap(),
        Some(json!(instant()))
    );
    database.stop();
}

#[test]
fn precommit_crash_rolls_back_and_restart_captures_new_startup() {
    let mut database = database::Database::start();
    let settings = database.create("fresh_precommit");
    let mut store = Store::open(&settings).unwrap();
    store.set_boundary_hook(|point, key, _| {
        if point == tl3_store::SavePoint::ReceiveCatchUpBeforeCommit
            && key == "catch-up:github-catch-up"
        {
            panic!("synthetic precommit crash");
        }
    });
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| initialize_intake(
            &mut store,
            &served(),
            &[entry(1455)],
            instant,
            |_, number| Ok(current(number))
        )))
        .is_err()
    );
    drop(store);
    let mut store = Store::open(&settings).unwrap();
    assert!(store.ingress_cursor("github-catch-up").unwrap().is_none());
    assert!(store.pending_ingress().unwrap().is_empty());
    assert!(
        store
            .ingress_cursor("github-world-start")
            .unwrap()
            .is_none()
    );
    let restarted = instant() + chrono::Duration::seconds(10);
    initialize_intake(
        &mut store,
        &served(),
        &[entry(1574)],
        || restarted,
        |_, number| Ok(current(number)),
    )
    .unwrap();
    assert_eq!(
        store.ingress_cursor("github-catch-up").unwrap(),
        Some(json!(restarted))
    );
    assert!(
        matches!(&persisted_inputs(&mut store, &served())[0], Input::LabelledIssue(issue) if issue.number == 1574)
    );
    database.stop();
}

#[test]
fn postcommit_crash_preserves_cursor_and_intake_before_any_catchup() {
    let mut database = database::Database::start();
    let settings = database.create("fresh_postcommit");
    let mut store = Store::open(&settings).unwrap();
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            initialize_intake(
                &mut store,
                &served(),
                &[entry(1455)],
                instant,
                |_, number| Ok(current(number)),
            )
            .unwrap();
            panic!("synthetic crash after outer commit and before fetch");
        }))
        .is_err()
    );
    drop(store);
    let mut store = Store::open(&settings).unwrap();
    initialize_intake(
        &mut store,
        &served(),
        &[entry(1574)],
        || panic!("startup already committed"),
        |_, _| panic!("must not readmit"),
    )
    .unwrap();
    assert_eq!(
        store.ingress_cursor("github-catch-up").unwrap(),
        Some(json!(instant()))
    );
    let (sender, receiver) = mpsc::channel();
    fetch_catch_up(
        Arc::new(FreshStartHistory),
        store.reader().unwrap(),
        served(),
        sender,
    )
    .join()
    .unwrap();
    let WebhookMessage::CaughtUp(_, batch) = receiver.recv().unwrap() else {
        panic!("missing fetch result")
    };
    batch.unwrap().persist(&mut store).unwrap();
    let inputs = persisted_inputs(&mut store, &served());
    let admissions: Vec<_> = inputs
        .iter()
        .filter_map(|input| match input {
            Input::LabelledIssue(issue) => Some(issue.number),
            _ => None,
        })
        .collect();
    assert_eq!(admissions, [1455]);
    assert!(!inputs.iter().any(|input| matches!(
        input,
        Input::Observation {
            observation: Observation::GitHubComment { .. },
            ..
        }
    )));
    database.stop();
}

#[test]
fn missing_cursor_blocks_history_fetch() {
    let mut database = database::Database::start();
    let settings = database.create("fresh_blocked_history");
    let store = Store::open(&settings).unwrap();
    struct Forbidden;
    impl Fetcher for Forbidden {
        fn fetch(
            &self,
            _: DateTime<Utc>,
            _: DateTime<Utc>,
            _: tl3_store::Snapshot,
            _: &Bootstrap,
        ) -> Result<CatchUpBatch> {
            panic!("history fetched before startup commit")
        }
    }
    let (sender, receiver) = mpsc::channel();
    fetch_catch_up(
        Arc::new(Forbidden),
        store.reader().unwrap(),
        served(),
        sender,
    )
    .join()
    .unwrap();
    let WebhookMessage::CaughtUp(_, result) = receiver.recv().unwrap() else {
        panic!("missing result")
    };
    assert!(
        result
            .err()
            .unwrap()
            .to_string()
            .contains("cursor is not initialized")
    );
    database.stop();
}

#[test]
fn ingress_has_no_epoch_fallback() {
    let mut database = database::Database::start();
    let settings = database.create("fresh_no_epoch");
    let mut store = Store::open(&settings).unwrap();
    assert!(matches!(
        catch_up_state(&mut store.reader().unwrap()),
        Err(Error::Invalid("GitHub catch-up cursor is not initialized"))
    ));
    let before = Utc::now().with_nanosecond(0).unwrap();
    let catch_up = CatchUp::new(BTreeMap::new(), vec![]).unwrap();
    assert!(catch_up.pass(&mut store, &bootstrap()).unwrap().is_empty());
    let cursor: DateTime<Utc> =
        crate::decode_stored(store.ingress_cursor("github-catch-up").unwrap().unwrap()).unwrap();
    assert!(cursor >= before && cursor <= Utc::now());
    assert_eq!(
        store.ingress_cursor("github-world-start").unwrap(),
        Some(json!(cursor))
    );
    assert!(store.pending_ingress().unwrap().is_empty());
    database.stop();
}

#[test]
fn validation_window_overlap_deduplicates_by_issue_identity() {
    let mut database = database::Database::start();
    let settings = database.create("fresh_overlap");
    let root = tempfile::tempdir().unwrap();
    let mut store = Store::open(&settings).unwrap();
    let event = std::cell::RefCell::new(None);
    initialize_intake(&mut store, &served(), &[entry(1455), entry(1455)], instant, |repo, number| {
        assert!(event.borrow().is_none(), "duplicate initial list must fetch once");
        let changes: tl3_github::RepoChanges = serde_json::from_value(json!({
            "repository":"synthetic/project", "issues":[current(number)],
            "labels":[[number, {"id":91,"event":"labeled","actor":{"id":1,"login":"synthetic-rob"},"label":{"name":"tl3"},"created_at":instant()+chrono::Duration::seconds(1)}]],
            "comments":[],"pull_requests":[],"check_runs":[],"check_suites":[]
        })).unwrap();
        *event.borrow_mut() = Some(GitHubDelivery::CatchUp { repository:repo.clone(), changes:Box::new(changes) });
        Ok(current(number))
    }).unwrap();
    struct Window(GitHubDelivery);
    impl Fetcher for Window {
        fn fetch(
            &self,
            since: DateTime<Utc>,
            _world_start: DateTime<Utc>,
            _: tl3_store::Snapshot,
            _: &Bootstrap,
        ) -> Result<CatchUpBatch> {
            assert_eq!(since, instant(), "catchup must start at prevalidation T0");
            Ok(CatchUpBatch {
                deliveries: vec![("window-label".into(), serde_json::to_vec(&self.0)?)],
                cursor: Some(instant() + chrono::Duration::seconds(2)),
            })
        }
    }
    let (sender, receiver) = mpsc::channel();
    fetch_catch_up(
        Arc::new(Window(event.into_inner().unwrap())),
        store.reader().unwrap(),
        served(),
        sender,
    )
    .join()
    .unwrap();
    let WebhookMessage::CaughtUp(_, batch) = receiver.recv().unwrap() else {
        panic!("missing window result")
    };
    batch.unwrap().persist(&mut store).unwrap();
    let inputs = persisted_inputs(&mut store, &served());
    assert_eq!(
        inputs
            .iter()
            .filter(|input| matches!(input, Input::LabelledIssue(_)))
            .count(),
        2,
        "exercise distinct initial and catch-up delivery IDs"
    );
    let mut controller = Controller::open(
        Store::open(&settings).unwrap(),
        crate::adapters::simulation(root.path()).unwrap(),
        served(),
    )
    .unwrap();
    drain(&mut controller).unwrap();
    assert_eq!(
        controller.world().unwrap().nodes.len(),
        1,
        "one request for overlapping issue identities"
    );
    let snapshot = controller.snapshot().unwrap();
    assert_eq!(
        snapshot
            .rows
            .iter()
            .filter(|row| row.table == "requests")
            .count(),
        1
    );
    drop(controller);
    let mut restarted = Controller::open(
        Store::open(&settings).unwrap(),
        crate::adapters::simulation(root.path()).unwrap(),
        served(),
    )
    .unwrap();
    drain(&mut restarted).unwrap();
    assert_eq!(restarted.world().unwrap().nodes.len(), 1);
    database.stop();
}

#[test]
fn real_github_transport_keeps_prestartup_comments_and_closed_labels_out() {
    use tl3_github::{AppConfig, ClientOptions};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let mut database = database::Database::start();
    let settings = database.create("fresh_http");
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let server = runtime.block_on(MockServer::start());
    let t0: DateTime<Utc> = "2026-11-08T05:00:00.750Z".parse().unwrap();
    let boundary = t0.with_nanosecond(0).unwrap();
    let old = boundary - chrono::Duration::seconds(1);
    let during_validation = boundary;
    let read_settings = settings.clone();
    runtime.block_on(async {
        Mock::given(wiremock::matchers::any()).respond_with(move |request: &wiremock::Request| {
            let path = request.url.path();
            if path == "/app/installations/1/access_tokens" {
                return ResponseTemplate::new(201).set_body_json(json!({"token":"synthetic-token","expires_at":"2099-01-01T00:00:00Z"}));
            }
            let validation = path == "/repos/synthetic/project/issues/1455";
            std::thread::scope(|scope| {
                scope.spawn(|| {
                    let mut store = Store::open(&read_settings).unwrap();
                    if validation {
                        assert!(store.ingress_cursor("github-catch-up").unwrap().is_none(), "validation precedes commit");
                        assert!(store.pending_ingress().unwrap().is_empty());
                    } else {
                        assert_eq!(store.ingress_cursor("github-world-start").unwrap(),Some(json!(boundary)), "history requires durable immutable boundary");
                        assert!(store.ingress_cursor("github-catch-up").unwrap().is_some());
                        assert!(store.pending_ingress().unwrap().iter().any(|row|row.delivery_key == "initial-intake:synthetic/project:1455"),"initial intake must be durable before history");
                    }
                }).join().unwrap();
            });
            if validation {
                return ResponseTemplate::new(200).set_body_json(json!(current(1455)));
            }
            let body = match path {
                "/repos/synthetic/project/issues" => {
                    let mut closed = current(42); closed.state = "closed".into();
                    json!([closed,current(1455),current(99)])
                }
                "/repos/synthetic/project/issues/comments" => json!([
                    {"id":1,"body":"tl3 approve","user":{"id":1,"login":"synthetic-rob"},"created_at":old,"updated_at":Utc::now(),"issue_url":"https://api.github.com/repos/synthetic/project/issues/42"},
                    {"id":2,"body":"tl3 approve","user":{"id":1,"login":"synthetic-rob"},"created_at":during_validation,"updated_at":Utc::now(),"issue_url":"https://api.github.com/repos/synthetic/project/issues/1455"},
                    {"id":3,"body":"tl3 approve","user":{"id":1,"login":"synthetic-rob"},"created_at":during_validation,"updated_at":Utc::now(),"issue_url":"https://api.github.com/repos/synthetic/project/issues/1455"}
                ]),
                "/repos/synthetic/project/issues/events" => json!([
                    {"id":99,"event":"labeled","actor":{"id":1,"login":"synthetic-rob"},"issue":{"number":42},"label":{"name":"tl3"},"created_at":during_validation},
                    {"id":98,"event":"labeled","actor":{"id":1,"login":"synthetic-rob"},"issue":{"number":1455},"label":{"name":"tl3"},"created_at":during_validation},
                    {"id":97,"event":"labeled","actor":{"id":1,"login":"synthetic-rob"},"issue":{"number":1455},"label":{"name":"tl3"},"created_at":during_validation},
                    {"id":96,"event":"labeled","actor":{"id":1,"login":"synthetic-rob"},"issue":{"number":99},"label":{"name":"tl3"},"created_at":old}
                ]),
                "/repos/synthetic/project/pulls" => json!([]),
                _ => panic!("unexpected synthetic endpoint {path}"),
            };
            ResponseTemplate::new(200).set_body_json(body)
        }).mount(&server).await;
    });
    let client = GitHub::new(
        AppConfig {
            id: 123,
            slug: "synthetic".into(),
            pem: include_str!("../../../../../mergify/tests/fixtures/throwaway-key.pem").into(),
            webhook_secret: "synthetic".into(),
        },
        1,
        ClientOptions {
            api_url: format!("{}/", server.uri()).parse().unwrap(),
            max_retries: 0,
            max_wait: Duration::ZERO,
            mutation_interval: Duration::ZERO,
            ..ClientOptions::default()
        },
    )
    .unwrap();
    let mut store = Store::open(&settings).unwrap();
    initialize_intake(
        &mut store,
        &served(),
        &[entry(1455)],
        || t0,
        |_, number| {
            runtime
                .block_on(client.issue("synthetic/project", number))
                .map_err(Error::from)
        },
    )
    .unwrap();
    let catchup = CatchUp::new(BTreeMap::from([(1, client)]), vec![]).unwrap();
    catchup
        .fetch(boundary, boundary, store.load().unwrap(), &served())
        .unwrap()
        .persist(&mut store)
        .unwrap();
    let inputs = persisted_inputs(&mut store, &served());
    let admissions: Vec<_> = inputs
        .iter()
        .filter_map(|input| match input {
            Input::LabelledIssue(issue) => Some(issue.number),
            _ => None,
        })
        .collect();
    assert_eq!(
        admissions,
        [1455, 1455, 1455],
        "initial intake and overlap event only"
    );
    let comments: Vec<_> = inputs
        .iter()
        .filter_map(|input| match input {
            Input::Observation {
                observation: Observation::GitHubComment { comment, .. },
                ..
            } => Some(comment.id),
            _ => None,
        })
        .collect();
    assert_eq!(
        comments,
        [2, 3],
        "old Rob approval, even edited after startup, must not replay"
    );
    let advanced: DateTime<Utc> =
        crate::decode_stored(store.ingress_cursor("github-catch-up").unwrap().unwrap()).unwrap();
    assert!(advanced > boundary);
    catchup.pass(&mut store, &served()).unwrap();
    assert_eq!(
        store.ingress_cursor("github-world-start").unwrap(),
        Some(json!(boundary))
    );
    let rows = store.pending_ingress().unwrap();
    let GitHubDelivery::CatchUp { changes, .. } = replay(rows.last().unwrap()).unwrap() else {
        panic!("missing edited-comment recovery")
    };
    assert_eq!(
        changes
            .comments
            .iter()
            .map(|comment| comment.id)
            .collect::<Vec<_>>(),
        [2, 3],
        "postworld edits survive moving-cursor advancement; preworld edits stay excluded"
    );
    assert!(
        changes
            .comments
            .iter()
            .all(|comment| comment.created_at < advanced && comment.updated_at > advanced)
    );
    let root = tempfile::tempdir().unwrap();
    let mut controller = Controller::open(
        Store::open(&settings).unwrap(),
        crate::adapters::simulation(root.path()).unwrap(),
        served(),
    )
    .unwrap();
    drain(&mut controller).unwrap();
    assert_eq!(controller.world().unwrap().nodes.len(), 1);
    database.stop();
}

#[test]
fn unsupported_cursor_without_world_start_refuses_startup_with_named_error() {
    let mut database = database::Database::start();
    let settings = database.create("fresh_unsupported_cursor");
    let mut store = Store::open(&settings).unwrap();
    store
        .receive_catch_up("github-catch-up", &[], &json!(instant()))
        .unwrap();
    let error = initialize_intake(
        &mut store,
        &served(),
        &[entry(1455)],
        || panic!("unsupported state must not capture a new boundary"),
        |_, _| panic!("unsupported state must not fetch"),
    )
    .unwrap_err();
    assert!(matches!(error, Error::CatchUpWithoutWorldStart));
    assert_eq!(
        error.to_string(),
        "unsupported store state: catch-up cursor without world start; fresh-init required"
    );
    assert_eq!(
        store.ingress_cursor("github-catch-up").unwrap(),
        Some(json!(instant()))
    );
    assert!(
        store
            .ingress_cursor("github-world-start")
            .unwrap()
            .is_none()
    );
    assert!(store.pending_ingress().unwrap().is_empty());
    database.stop();
}

#[test]
fn startup_second_is_in_world_and_both_cursors_publish_at_outer_commit() {
    let mut database = database::Database::start();
    let settings = database.create("fresh_outer_commit");
    let observer = settings.clone();
    let mut store = Store::open(&settings).unwrap();
    let observed = Arc::new(AtomicUsize::new(0));
    let releases = Arc::clone(&observed);
    store.set_boundary_hook(move |point, _, _| {
        if point == tl3_store::SavePoint::ReceiveCatchUpAfterCommit {
            let mut read = Store::open(&observer).unwrap();
            assert!(
                read.ingress_cursor("github-world-start").unwrap().is_none(),
                "savepoint release must not publish immutable boundary"
            );
            assert!(
                read.ingress_cursor("github-catch-up").unwrap().is_none(),
                "savepoint release must not publish moving cursor"
            );
            assert!(
                read.pending_ingress().unwrap().is_empty(),
                "savepoint release must not publish inputs"
            );
            releases.fetch_add(1, Ordering::SeqCst);
        }
    });
    let exact: DateTime<Utc> = "2026-11-08T05:00:00.750Z".parse().unwrap();
    let boundary: DateTime<Utc> = "2026-11-08T05:00:00Z".parse().unwrap();
    initialize_intake(
        &mut store,
        &served(),
        &[entry(1455)],
        || exact,
        |_, number| Ok(current(number)),
    )
    .unwrap();
    assert_eq!(observed.load(Ordering::SeqCst), 2);
    let mut read = Store::open(&settings).unwrap();
    assert_eq!(
        read.ingress_cursor("github-world-start").unwrap(),
        Some(json!(boundary))
    );
    assert_eq!(
        read.ingress_cursor("github-catch-up").unwrap(),
        Some(json!(boundary))
    );
    assert_eq!(read.pending_ingress().unwrap().len(), 1);
    database.stop();
}

#[test]
fn database_error_rolls_back_both_cursors_and_all_initial_inputs() {
    let mut database = database::Database::start();
    let settings = database.create("fresh_commit_error");
    let mut fixture = postgres::Config::new()
        .host(&settings.host)
        .port(settings.port)
        .dbname(&settings.database)
        .user(&settings.user)
        .password("tl3-disposable-test-password")
        .connect(postgres::NoTls)
        .unwrap();
    fixture
        .batch_execute(
            "CREATE FUNCTION refuse_initial_cursor() RETURNS trigger LANGUAGE plpgsql AS $$
             BEGIN
                 IF NEW.source = 'github-catch-up' THEN
                     RAISE EXCEPTION 'synthetic initial cursor write failure';
                 END IF;
                 RETURN NEW;
             END $$;
             CREATE TRIGGER refuse_initial_cursor BEFORE INSERT ON tl3_ingress_cursors
             FOR EACH ROW EXECUTE FUNCTION refuse_initial_cursor()",
        )
        .unwrap();
    let mut store = Store::open(&settings).unwrap();
    assert!(
        initialize_intake(
            &mut store,
            &served(),
            &[entry(1455), entry(1574)],
            instant,
            |_, number| Ok(current(number))
        )
        .is_err()
    );
    assert!(
        store
            .ingress_cursor("github-world-start")
            .unwrap()
            .is_none()
    );
    assert!(store.ingress_cursor("github-catch-up").unwrap().is_none());
    assert!(store.pending_ingress().unwrap().is_empty());
    fixture
        .batch_execute("DROP TRIGGER refuse_initial_cursor ON tl3_ingress_cursors")
        .unwrap();
    initialize_intake(
        &mut store,
        &served(),
        &[entry(1455)],
        instant,
        |_, number| Ok(current(number)),
    )
    .unwrap();
    assert_eq!(store.pending_ingress().unwrap().len(), 1);
    database.stop();
}
