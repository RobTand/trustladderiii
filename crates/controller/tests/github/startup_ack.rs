use super::*;

struct BlockCatchUp {
    entered: mpsc::Sender<()>,
    release: Mutex<mpsc::Receiver<()>>,
    api: Api,
}

impl Respond for BlockCatchUp {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        self.entered.send(()).unwrap();
        self.release.lock().unwrap().recv().unwrap();
        self.api.respond(request)
    }
}

pub(super) fn run(database: &mut support::Database) {
    loop_scenarios(database);
    let during = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        during_catch_up(&database.create("startup_ack"));
    }));
    before_source_run(&database.create("bound_ack"));
    if let Err(error) = during {
        std::panic::resume_unwind(error);
    }
}

fn during_catch_up(settings: &tl3_config::Store) {
    let api = Server::new();
    api.state.lock().unwrap().gap = true;
    let (entered, started) = mpsc::channel();
    let (release, blocked) = mpsc::channel();
    api.runtime.block_on(async {
        Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/repos/owner/sandbox/issues"))
            .respond_with(BlockCatchUp {
                entered,
                release: Mutex::new(blocked),
                api: Api(api.state.clone()),
            })
            .with_priority(1)
            .up_to_n_times(1)
            .mount(&api.server)
            .await;
    });
    let root = tempfile::tempdir().unwrap();
    let mut controller = controller(settings, root.path(), &api.uri());
    let source = Webhooks::bind(
        "127.0.0.1:0".parse().unwrap(),
        Store::open(settings).unwrap(),
        bootstrap(),
        b"test-secret".to_vec(),
        Some(CatchUp::new(BTreeMap::from([(3, client(&api.uri()))])).unwrap()),
    )
    .unwrap();
    let address = source.address();
    let control = source.control();
    let (sender, receiver) = mpsc::channel();
    let source_thread = thread::spawn(move || source.run(sender));
    started.recv_timeout(Duration::from_secs(10)).unwrap();
    let (sent, response) = mpsc::channel();
    let post_thread = thread::spawn(move || {
        let start = Instant::now();
        let statuses = [
            post(address, "startup-label", false, fixture(false), true),
            post(address, "startup-label", false, fixture(false), true),
            post(address, "startup-comment", true, fixture(true), true),
        ];
        sent.send((statuses, start.elapsed())).unwrap();
    });
    let timely = response.recv_timeout(Duration::from_secs(1));
    let pending = Store::open(settings).unwrap().pending_ingress().unwrap();
    if timely.is_ok() {
        for _ in 0..3 {
            receiver.recv_timeout(Duration::from_secs(10)).unwrap();
        }
        ingress::drain(&mut controller).unwrap();
        assert_eq!(controller.world().unwrap().nodes.len(), 1);
    }
    // Release even on the red implementation so the failure is an assertion, not a hung job.
    release.send(()).unwrap();
    post_thread.join().unwrap();
    if timely.is_ok() {
        receiver.recv_timeout(Duration::from_secs(10)).unwrap();
    }
    control.stop();
    source_thread.join().unwrap().unwrap();
    let (statuses, elapsed) =
        timely.expect("signed POSTs must finish while startup catch-up is blocked");
    assert_eq!(statuses, [202; 3]);
    assert!(elapsed < Duration::from_secs(1));
    assert_eq!(pending.len(), 2, "2xx deliveries must already be durable");
    assert!(pending.iter().all(|row| row.source == "github-webhook"));
    assert!(
        Store::open(settings)
            .unwrap()
            .ingress_cursor("github-catch-up")
            .unwrap()
            .is_some()
    );
    ingress::drain(&mut controller).unwrap();
    let snapshot = controller.snapshot().unwrap();
    assert_eq!(
        controller.world().unwrap().nodes.len(),
        1,
        "catch-up label and webhook have one request"
    );
    assert_eq!(
        snapshot
            .rows
            .iter()
            .filter(|row| row.table == "inbox"
                && serde_json::from_value::<Observation>(row.value.clone()).is_ok_and(
                    |observation| matches!(observation, Observation::GitHubComment { .. })
                ))
            .count(),
        1,
        "catch-up and webhook apply the comment once"
    );
    ingress::drain(&mut controller).unwrap();
    assert_eq!(controller.snapshot().unwrap().revision, snapshot.revision);
    assert!(
        Store::open(settings)
            .unwrap()
            .pending_ingress()
            .unwrap()
            .is_empty()
    );
    drain(&mut controller);
    assert!(
        Store::open(settings)
            .unwrap()
            .comment_retired("owner/sandbox/3/9")
            .unwrap()
    );
    assert_eq!(controller.world().unwrap().nodes.len(), 1);
    assert_eq!(api.state.lock().unwrap().posts, 2);
    println!(
        "test startup_ack_during_catch_up ... ok ({elapsed:?}; durable before catch-up finished)"
    );
}

fn before_source_run(settings: &tl3_config::Store) {
    let source = Webhooks::bind(
        "127.0.0.1:0".parse().unwrap(),
        Store::open(settings).unwrap(),
        bootstrap(),
        b"test-secret".to_vec(),
        None,
    )
    .unwrap();
    let address = source.address();
    let control = source.control();
    let (sent, response) = mpsc::channel();
    let request = thread::spawn(move || {
        sent.send(post(
            address,
            "before-source-run",
            false,
            fixture(false),
            true,
        ))
        .unwrap();
    });
    let timely = response.recv_timeout(Duration::from_secs(1));
    let pending = Store::open(settings).unwrap().pending_ingress().unwrap();
    let (sender, _receiver) = mpsc::channel();
    let running = thread::spawn(move || source.run(sender));
    request.join().unwrap();
    control.stop();
    running.join().unwrap().unwrap();
    assert_eq!(
        timely
            .expect("bound listener must answer before controller startup or source registration"),
        202
    );
    assert_eq!(pending.len(), 1);
    println!("test startup_ack_before_source_run ... ok");
}

struct Passes {
    entered: mpsc::Sender<()>,
    release: Mutex<mpsc::Receiver<bool>>,
    api: Api,
}
impl Respond for Passes {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        self.entered.send(()).unwrap();
        if self.release.lock().unwrap().recv().unwrap() {
            self.api.respond(request)
        } else {
            ResponseTemplate::new(422)
        }
    }
}

fn loop_scenarios(database: &mut support::Database) {
    for scenario in ["gap", "failure", "stop", "reconnect"] {
        let settings = database.create(&format!("loop_{scenario}"));
        let api = Server::new();
        let (entered, calls) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        api.runtime.block_on(async {
            Mock::given(wiremock::matchers::method("GET"))
                .and(wiremock::matchers::path("/repos/owner/sandbox/issues"))
                .respond_with(Passes { entered, release: Mutex::new(blocked), api: Api(api.state.clone()) })
                .with_priority(1).mount(&api.server).await;
        });
        let source = Webhooks::bind("127.0.0.1:0".parse().unwrap(), Store::open(&settings).unwrap(), bootstrap(), b"test-secret".to_vec(), Some(CatchUp::new(BTreeMap::from([(3, client(&api.uri()))])).unwrap())).unwrap();
        let address = source.address();
        let control = source.control();
        let (sender, wakes) = mpsc::channel();
        let (done, finished) = mpsc::channel();
        let running = thread::spawn(move || { done.send(source.run(sender)).unwrap(); });
        calls.recv_timeout(Duration::from_secs(10)).unwrap();
        if scenario == "stop" {
            control.stop();
            assert!(finished.recv_timeout(Duration::from_millis(250)).is_err(), "Stop must join the outstanding fetch");
            release.send(true).unwrap();
            finished.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
            running.join().unwrap();
            assert!(calls.try_recv().is_err());
            println!("test startup_loop_stop_joins_inflight_fetch ... ok");
            continue;
        }
        if scenario == "gap" {
            for _ in 0..12 { control.gap(); }
            // A signed request is processed after all the Gap messages, before release.
            assert_eq!(post(address, "gap-barrier", false, fixture(false), true), 202);
            wakes.recv_timeout(Duration::from_secs(10)).unwrap();
        }
        release.send(scenario != "failure").unwrap();
        if scenario == "failure" {
            assert!(calls.recv_timeout(Duration::from_millis(250)).is_err(), "failed passes must wait for an event, without a timer");
            assert!(Store::open(&settings).unwrap().ingress_cursor("github-catch-up").unwrap().is_none());
            assert_eq!(post(address, "retry-event", false, fixture(false), true), 202);
            wakes.recv_timeout(Duration::from_secs(10)).unwrap();
        } else {
            wakes.recv_timeout(Duration::from_secs(10)).unwrap();
        }
        if scenario == "reconnect" {
            database.restart();
            control.gap();
        }
        calls.recv_timeout(Duration::from_secs(10)).unwrap();
        release.send(true).unwrap();
        wakes.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(Store::open(&settings).unwrap().ingress_cursor("github-catch-up").unwrap().is_some());
        assert!(calls.recv_timeout(Duration::from_millis(250)).is_err(), "all gaps during a fetch must coalesce into exactly one follow-up pass");
        control.stop();
        finished.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
        running.join().unwrap();
        assert_eq!(api.state.lock().unwrap().catch_up_passes, if scenario == "failure" { 1 } else { 2 });
        println!("test startup_loop_{scenario} ... ok");
    }
}
