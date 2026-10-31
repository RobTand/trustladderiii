use super::*;

const LOCAL: &str = "0000000000000000000000000000000000000001";
const PUSHED: &str = "ffffffffffffffffffffffffffffffffffffffff";

pub(super) fn run(database: &mut support::Database) {
    let mut failures = 0;
    for endpoint in ["check-runs", "check-suites"] {
        for status in [422, 404] {
            let settings = database.create(&format!("missing_{endpoint}_{status}"));
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                missing_head(&settings, endpoint, status);
            }))
            .is_err()
            {
                failures += 1;
            }
        }
    }
    for (endpoint, status) in [
        ("issues", 422),
        ("issues/comments", 404),
        ("pulls", 403),
        ("issues/events", 503),
        (
            "commits/0000000000000000000000000000000000000001/check-runs",
            403,
        ),
        (
            "commits/0000000000000000000000000000000000000001/check-suites",
            503,
        ),
    ] {
        let settings = database.create(&format!("diagnostic_{failures}_{status}"));
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            failed_request(&settings, endpoint, status);
        }))
        .is_err()
        {
            failures += 1;
        }
    }
    assert_eq!(failures, 0, "catch-up regressions failed");
}

fn track(settings: &tl3_config::Store, api: &Server, root: &Path) -> Store {
    let mut controller = controller(settings, root, &api.uri());
    controller.intake(labelled()).unwrap();
    let mut store = Store::open(settings).unwrap();
    store.intern_commit("0", LOCAL).unwrap();
    store.intern_commit("0", PUSHED).unwrap();
    store
}

fn reject(api: &Server, endpoint: &str, status: u16) {
    api.runtime.block_on(async {
        Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path(format!(
                "/repos/owner/sandbox/{endpoint}"
            )))
            .respond_with(
                ResponseTemplate::new(status)
                    .set_body_json(json!({"message": "No commit found for SHA; secret-response"})),
            )
            .with_priority(1)
            .mount(&api.server)
            .await;
    });
}

fn missing_head(settings: &tl3_config::Store, endpoint: &str, status: u16) {
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let mut store = track(settings, &api, root.path());
    let mut check = event_fixture("check_run")["check_run"].clone();
    check["head_sha"] = json!(PUSHED);
    api.state.lock().unwrap().checks.push(check);
    // A successful runs read must not leak evidence if the suites read says the head is absent.
    let mut check = event_fixture("check_run")["check_run"].clone();
    check["head_sha"] = json!(LOCAL);
    api.state.lock().unwrap().checks.push(check);
    reject(&api, &format!("commits/{LOCAL}/{endpoint}"), status);
    let catch_up = CatchUp::new(BTreeMap::from([(3, client(&api.uri()))])).unwrap();
    assert!(
        catch_up.pass(&mut store, &bootstrap()).is_ok(),
        "catch-up must skip an unknown tracked head ({endpoint} HTTP {status})"
    );
    let pending = store.pending_ingress().unwrap();
    let GitHubDelivery::CatchUp { changes, .. } =
        serde_json::from_slice(&pending[0].payload).unwrap()
    else {
        panic!("missing catch-up evidence");
    };
    assert_eq!(changes.check_runs.len(), 1);
    assert_eq!(changes.check_runs[0].head_sha, PUSHED);
    assert!(store.ingress_cursor("github-catch-up").unwrap().is_some());
    for endpoint in ["check-runs", "check-suites"] {
        assert!(api.requests().iter().any(|request| {
            request.url.path() == format!("/repos/owner/sandbox/commits/{PUSHED}/{endpoint}")
        }));
    }
    println!("test catch_up_missing_head_{endpoint}_{status} ... ok");
}

fn failed_request(settings: &tl3_config::Store, endpoint: &str, status: u16) {
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let mut store = track(settings, &api, root.path());
    let catch_up = CatchUp::new(BTreeMap::from([(3, client(&api.uri()))])).unwrap();
    catch_up.pass(&mut store, &bootstrap()).unwrap();
    let cursor = store.ingress_cursor("github-catch-up").unwrap();
    let pending = store.pending_ingress().unwrap();
    reject(&api, endpoint, status);
    let error = catch_up.pass(&mut store, &bootstrap()).unwrap_err();
    let diagnostic = error.to_string();
    assert!(
        diagnostic.contains(&format!("GET repos/owner/sandbox/{endpoint}"))
            && diagnostic.contains(&format!("HTTP {status}")),
        "catch-up error must name the failed endpoint and HTTP status: {diagnostic}"
    );
    assert!(!diagnostic.contains("secret-response"));
    assert!(!diagnostic.contains("disposable-test-token"));
    assert_eq!(store.ingress_cursor("github-catch-up").unwrap(), cursor);
    assert_eq!(store.pending_ingress().unwrap(), pending);
    println!("test catch_up_failure_diagnostic_{endpoint}_{status} ... ok");
}
