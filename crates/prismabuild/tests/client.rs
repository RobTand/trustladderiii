use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    num::NonZeroU32,
    path::Path,
    time::Duration,
};
use tempfile::TempDir;
use tl3_prismabuild::{
    ActionKey, ActionState, Demand, Error, Options, PrismaBuild, ProgressPhase, SubmissionStatus,
    SubmitAction, Tool,
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{header, method, path, query_param},
};

const EPOCH: &str = "0123456789abcdef0123456789abcdef";

fn key(n: u32) -> ActionKey {
    format!("{n:064x}").parse().unwrap()
}

struct Fixture {
    dir: TempDir,
    client: PrismaBuild,
}

impl Fixture {
    fn new(url: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        for state in ["ready", "claimed", "done", "failed", "withdrawn"] {
            std::fs::create_dir(dir.path().join(state)).unwrap();
        }
        std::fs::write(dir.path().join("exit_code"), "0").unwrap();
        let client = PrismaBuild::new(Options {
            queue_root: dir.path().to_owned(),
            tool: Tool {
                program: "/usr/bin/python3".into(),
                args: vec![
                    Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("tests/fixtures/fake_pbrun.py")
                        .into(),
                    dir.path().into(),
                ],
            },
            bell_url: url.into(),
        })
        .unwrap();
        Self { dir, client }
    }

    fn write(&self, relative: impl AsRef<Path>, value: Value) {
        let path = self.dir.path().join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    }

    fn record(
        &self,
        state: &str,
        n: u32,
        status: Option<&str>,
        code: Option<i32>,
        generation: f64,
    ) {
        self.write(format!("{state}/{}.json", key(n)), json!({
            "schema": if ["ready", "claimed"].contains(&state) { "prismaquant.prismabuild.pool_item.v1" }
                else { "prismaquant.prismabuild.pool_outcome.v1" },
            "action_key": key(n), "published_unix": generation,
            "attempts": 1, "max_attempts": 1, "status": status, "detail": {"returncode": code}
        }));
    }

    fn response(&self, value: Value) {
        self.write("response.json", value);
    }
    fn calls(&self) -> Vec<Vec<String>> {
        std::fs::read_to_string(self.dir.path().join("calls.jsonl"))
            .unwrap()
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect()
    }
}

fn action() -> SubmitAction {
    SubmitAction {
        checkout: "/tmp/checkout with spaces".into(),
        command: vec![
            "worker".into(),
            "literal $(touch never); --timeout-s".into(),
        ],
        demand: Demand {
            cpus: NonZeroU32::new(2).unwrap(),
            mem_gb: 4.0,
            gpu: false,
        },
        container_image: format!("content:sha256:{}", "a".repeat(64)),
        progress: vec![ProgressPhase {
            name: "checks".into(),
            stall_allowance: Duration::from_secs(120),
        }],
        progress_cycle: false,
        env: BTreeMap::from([("RUST_TEST_THREADS".into(), "2".into())]),
    }
}

#[tokio::test]
async fn detached_submission_passes_demand_image_progress_and_literal_arguments() {
    let f = Fixture::new("http://127.0.0.1:9/");
    for (status, expected) in [
        ("submitted", SubmissionStatus::Submitted),
        ("attached", SubmissionStatus::Attached),
        ("cache_hit", SubmissionStatus::CacheHit),
    ] {
        f.response(json!({"schema":"prismabuild.submission.v1", "action_key":key(1),
            "transport":"pool", "status":status, "publication":{"published_unix": if status == "cache_hit" { None } else { Some(100.0) }}}));
        let submission = f.client.submit(&action()).await.unwrap();
        assert_eq!(submission.key, key(1));
        assert_eq!(submission.status, expected);
    }
    let calls = f.calls();
    let args = &calls[0];
    let separator = args.iter().position(|s| s == "--").unwrap();
    assert!(
        !args[..separator]
            .iter()
            .any(|s| s == "--timeout-s" || s == "--wait-s" || s == "--here")
    );
    for pair in [
        ["--cpus", "2"],
        ["--demand", "mem_gb=4,gpu=0"],
        ["--progress-phase", "checks=120"],
        ["--cwd", "/tmp/checkout with spaces"],
        ["--env", "OMP_NUM_THREADS=1"],
        ["--env", "RUST_TEST_THREADS=2"],
    ] {
        assert!(args.windows(2).any(|w| w == pair));
    }
    assert!(args.windows(2).any(|w| w
        == [
            "--container-image",
            &format!("content:sha256:{}", "a".repeat(64))
        ]));
    assert_eq!(
        &args[separator + 1..],
        ["worker", "literal $(touch never); --timeout-s"]
    );
    assert!(args.iter().any(|s| s == "--detach"));
    assert!(args.iter().any(|s| s == "--anywhere"));
}

#[tokio::test]
async fn accepts_legacy_detach_schema_and_rejects_uncertain_tool_results() {
    let f = Fixture::new("http://127.0.0.1:9/");
    f.response(
        json!({"schema":"prismaquant.prismabuild.pbrun_detach.v1", "action_key":key(1),
        "transport":"pool", "status":"attached", "published_unix":123.0}),
    );
    assert_eq!(
        f.client.submit(&action()).await.unwrap().published_unix,
        Some(123.0)
    );
    std::fs::write(f.dir.path().join("exit_code"), "74").unwrap();
    assert!(matches!(
        f.client.submit(&action()).await,
        Err(Error::Tool { code: Some(74) })
    ));
    assert_eq!(
        f.calls().len(),
        2,
        "uncertain submissions must never automatically retry"
    );
    std::fs::write(f.dir.path().join("exit_code"), "0").unwrap();
    for response in [
        json!({"schema":"other"}),
        json!({"schema":"prismabuild.submission.v1", "action_key":"../escape",
        "transport":"pool", "status":"submitted"}),
    ] {
        f.response(response);
        assert!(matches!(
            f.client.submit(&action()).await,
            Err(Error::ToolResponse(_))
        ));
    }
}

#[tokio::test]
async fn invalid_submission_never_launches_the_tool() {
    let f = Fixture::new("http://127.0.0.1:9/");
    let mut request = action();
    request.progress.clear();
    assert!(matches!(
        f.client.submit(&request).await,
        Err(Error::InvalidInput(_))
    ));
    request = action();
    request.demand.mem_gb = f64::NAN;
    assert!(f.client.submit(&request).await.is_err());
    request = action();
    request.container_image = "repo:latest".into();
    assert!(f.client.submit(&request).await.is_err());
    assert!(!f.dir.path().join("calls.jsonl").exists());
    assert!("../escape".parse::<ActionKey>().is_err());
}

#[tokio::test]
async fn withdraw_is_repeatable_but_is_not_cleanup_confirmation() {
    let f = Fixture::new("http://127.0.0.1:9/");
    f.record("claimed", 3, None, None, 100.0);
    f.response(
        json!({"schema":"prismabuild.withdrawal.v1", "action_key":key(3),
        "state_before":"claimed", "withdrawn":true, "reason":"stopping"}),
    );
    for _ in 0..2 {
        let result = f.client.withdraw(&key(3), "stopping").await.unwrap();
        assert!(result.withdrawn);
        assert_eq!(result.state_before.as_deref(), Some("claimed"));
    }
    assert_eq!(
        f.calls()[0],
        [
            "--transport",
            "pool",
            "--withdraw",
            key(3).as_str(),
            "--reason",
            "stopping",
            "--json"
        ]
    );
    std::fs::write(f.dir.path().join("exit_code"), "2").unwrap();
    assert!(matches!(
        f.client.withdraw(&key(3), "stopping").await,
        Err(Error::Tool { code: Some(2) })
    ));
    std::fs::write(f.dir.path().join("exit_code"), "0").unwrap();
    f.response(
        json!({"schema":"prismabuild.withdrawal.v1", "action_key":key(4), "withdrawn":true}),
    );
    assert!(matches!(
        f.client.withdraw(&key(3), "stopping").await,
        Err(Error::ToolResponse(_))
    ));
}

#[tokio::test]
async fn withdrawal_of_finished_unknown_or_withdrawn_work_confirms_without_a_tool_call() {
    let f = Fixture::new("http://127.0.0.1:9/");
    f.record("done", 1, Some("executed"), Some(0), 100.0);
    f.record("failed", 2, Some("failed"), Some(5), 100.0);
    f.record("failed", 3, Some("lease_lost_max_attempts"), None, 100.0);
    f.record("withdrawn", 4, Some("withdrawn"), None, 100.0);
    for n in 1..=5 {
        let result = f.client.withdraw(&key(n), "stopping").await.unwrap();
        assert_eq!(result.withdrawn, n == 4);
        assert_eq!(result.key, key(n));
    }
    assert!(!f.dir.path().join("calls.jsonl").exists());
}

#[tokio::test]
async fn restart_reconciliation_maps_all_record_states_and_missing_keys() {
    let f = Fixture::new("http://127.0.0.1:9/");
    let cases = [
        ("ready", None, None, ActionState::Queued),
        ("claimed", None, None, ActionState::Claimed),
        (
            "done",
            Some("executed"),
            Some(0),
            ActionState::Finished { returncode: 0 },
        ),
        (
            "done",
            Some("cache_hit"),
            None,
            ActionState::Finished { returncode: 0 },
        ),
        (
            "failed",
            Some("failed"),
            Some(7),
            ActionState::Failed {
                status: "failed".into(),
                returncode: Some(7),
            },
        ),
        (
            "failed",
            Some("no_progress"),
            None,
            ActionState::Failed {
                status: "no_progress".into(),
                returncode: None,
            },
        ),
        ("failed", Some("lease_lost"), None, ActionState::LeaseLost),
        (
            "failed",
            Some("lease_lost_max_attempts"),
            None,
            ActionState::LeaseLost,
        ),
        ("withdrawn", Some("withdrawn"), None, ActionState::Withdrawn),
    ];
    let mut awaited = BTreeSet::from([key(999)]);
    for (i, (directory, status, code, _)) in cases.iter().enumerate() {
        let n = i as u32 + 1;
        f.record(directory, n, *status, *code, 100.0);
        awaited.insert(key(n));
    }
    let records = f.client.reconcile(&awaited).await.unwrap();
    assert_eq!(records.len(), 10);
    assert!(records[&key(999)].is_none());
    for (i, (_, _, _, expected)) in cases.iter().enumerate() {
        assert_eq!(
            &records[&key(i as u32 + 1)].as_ref().unwrap().state,
            expected
        );
    }
    assert!(
        !f.dir.path().join("calls.jsonl").exists(),
        "reads never launch submission"
    );
}

#[tokio::test]
async fn newer_publication_wins_and_an_old_lease_never_declares_loss() {
    let f = Fixture::new("http://127.0.0.1:9/");
    f.record("done", 1, Some("executed"), Some(0), 100.0);
    f.record("claimed", 1, None, None, 200.0);
    f.write(
        format!("claimed/{}.lease", key(1)),
        json!({"schema":"prismaquant.prismabuild.pool_lease.v1",
        "action_key":key(1), "heartbeat_unix":1.0, "owner":"worker:1", "host":"fake-worker",
        "progress_observation":{"units_completed":4}}),
    );
    let record = f.client.read(&key(1)).await.unwrap().unwrap();
    assert_eq!(record.state, ActionState::Claimed);
    assert_eq!(record.lease.unwrap().heartbeat_unix, 1.0);
    f.record("failed", 1, Some("lease_lost_max_attempts"), None, 200.0);
    assert_eq!(
        f.client.read(&key(1)).await.unwrap().unwrap().state,
        ActionState::LeaseLost
    );
    f.record("withdrawn", 1, Some("withdrawn"), None, 200.0);
    assert_eq!(
        f.client.read(&key(1)).await.unwrap().unwrap().state,
        ActionState::Withdrawn
    );
}

#[tokio::test]
async fn requeued_and_reclaimed_actions_preserve_lease_loss_attempts() {
    let f = Fixture::new("http://127.0.0.1:9/");
    let outcome = format!("attempts/{}/{}/00000001.json", key(1), "a".repeat(64));
    f.write(&outcome, json!({"schema":"prismaquant.prismabuild.pool_attempt.v1", "action_key":key(1),
        "published_unix":100.0, "attempt":1, "status":"lease_lost", "disposition":"requeued", "detail":{"lease_age_s":301.0}}));
    for directory in ["ready", "claimed"] {
        f.write(
            format!("{directory}/{}.json", key(1)),
            json!({"schema":"prismaquant.prismabuild.pool_item.v1",
            "action_key":key(1), "published_unix":100.0, "attempts":1, "status":"lease_lost",
            "attempt_history":[{"attempt":1, "outcome":outcome}]}),
        );
        let record = f.client.read(&key(1)).await.unwrap().unwrap();
        assert_eq!(
            record.state,
            if directory == "ready" {
                ActionState::Queued
            } else {
                ActionState::Claimed
            }
        );
        assert!(record.attempts[0].lease_lost());
        assert_eq!(record.attempts[0].disposition, "requeued");
    }
    f.write(
        format!("withdrawn/{}.json", key(1)),
        json!({"schema":"prismaquant.prismabuild.pool_outcome.v1",
        "action_key":key(1), "published_unix":100.0, "status":"withdrawn",
        "attempt_history_before_withdrawal":[{"attempt":1,"outcome":outcome}]}),
    );
    let withdrawn = f.client.read(&key(1)).await.unwrap().unwrap();
    assert_eq!(withdrawn.state, ActionState::Withdrawn);
    assert!(withdrawn.attempts[0].lease_lost());
}

#[tokio::test]
async fn corrupt_or_unavailable_evidence_is_an_error() {
    let f = Fixture::new("http://127.0.0.1:9/");
    f.record("done", 1, Some("executed"), None, 100.0);
    assert!(matches!(
        f.client.read(&key(1)).await,
        Err(Error::Record { .. })
    ));
    f.write(
        format!("done/{}.json", key(1)),
        json!({"schema":"prismaquant.prismabuild.pool_outcome.v1",
        "action_key":key(2), "published_unix":100.0}),
    );
    assert!(matches!(
        f.client.read(&key(1)).await,
        Err(Error::Record { .. })
    ));
    std::fs::write(f.dir.path().join(format!("done/{}.json", key(1))), "{").unwrap();
    assert!(matches!(
        f.client.read(&key(1)).await,
        Err(Error::Record { .. })
    ));
    std::fs::remove_file(f.dir.path().join(format!("done/{}.json", key(1)))).unwrap();
    f.write(format!("ready/{}.json", key(1)), json!({"schema":"prismaquant.prismabuild.pool_item.v1",
        "action_key":key(1), "published_unix":100.0, "attempt_history":[{"attempt":1,"outcome":"../../secret"}]}));
    assert!(matches!(
        f.client.read(&key(1)).await,
        Err(Error::Record { .. })
    ));
    std::fs::remove_dir(f.dir.path().join("claimed")).unwrap();
    assert!(matches!(
        f.client.read(&key(999)).await,
        Err(Error::Io { .. })
    ));
}

#[tokio::test]
async fn bell_reset_rereads_every_awaited_key_and_reconciles_before_waiting() {
    let server = MockServer::start().await;
    let f = Fixture::new(&format!("{}/", server.uri()));
    for n in [1, 2] {
        f.record("ready", n, None, None, 100.0);
    }
    Mock::given(method("GET"))
        .and(path("/v1/now"))
        .and(header(
            "X-PrismaBuild-Queue",
            f.dir.path().to_str().unwrap(),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"epoch":EPOCH,"seq":9})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/wait"))
        .and(query_param("since", format!("{EPOCH}:9")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"epoch":"a".repeat(32),"seq":0,"reset":true})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let awaited = BTreeSet::from([key(1), key(2), key(99)]);
    let mut watcher = f.client.watcher();
    let initial = watcher
        .next(&awaited, Duration::from_secs(1))
        .await
        .unwrap();
    assert!(initial.reset && initial.bell_available);
    assert_eq!(
        initial.records[&key(1)].as_ref().unwrap().state,
        ActionState::Queued
    );
    f.record("done", 1, Some("executed"), Some(0), 100.0);
    f.record("failed", 2, Some("lease_lost_max_attempts"), None, 100.0);
    let reset = watcher
        .next(&awaited, Duration::from_secs(1))
        .await
        .unwrap();
    assert!(reset.reset && reset.bell_available);
    assert_eq!(reset.records.len(), 3);
    assert_eq!(
        reset.records[&key(1)].as_ref().unwrap().state,
        ActionState::Finished { returncode: 0 }
    );
    assert_eq!(
        reset.records[&key(2)].as_ref().unwrap().state,
        ActionState::LeaseLost
    );
    assert!(reset.records[&key(99)].is_none());
}

#[tokio::test]
async fn bell_hints_and_empty_replies_both_reread_records() {
    let server = MockServer::start().await;
    let f = Fixture::new(&format!("{}/", server.uri()));
    Mock::given(path("/v1/now"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"epoch":EPOCH,"seq":1})))
        .mount(&server)
        .await;
    Mock::given(path("/v1/wait"))
        .and(query_param("since", format!("{EPOCH}:1")))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"epoch":EPOCH,"seq":2,
            "events":[{"seq":2,"dir":"attempts","key":key(1)}]})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/v1/wait"))
        .and(query_param("since", format!("{EPOCH}:2")))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"epoch":EPOCH,"seq":2,"events":[]})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let awaited = BTreeSet::from([key(1)]);
    let mut watcher = f.client.watcher();
    assert!(
        watcher
            .next(&awaited, Duration::from_secs(1))
            .await
            .unwrap()
            .records[&key(1)]
            .is_none()
    );
    f.record("claimed", 1, None, None, 100.0);
    let hinted = watcher
        .next(&awaited, Duration::from_secs(1))
        .await
        .unwrap();
    assert!(!hinted.reset && hinted.bell_available);
    assert_eq!(
        hinted.records[&key(1)].as_ref().unwrap().state,
        ActionState::Claimed
    );
    f.record("done", 1, Some("executed"), Some(0), 100.0);
    assert_eq!(
        watcher
            .next(&awaited, Duration::from_secs(1))
            .await
            .unwrap()
            .records[&key(1)]
            .as_ref()
            .unwrap()
            .state,
        ActionState::Finished { returncode: 0 }
    );
}

#[tokio::test]
async fn unavailable_bell_falls_back_to_records_without_losing_work() {
    let server = MockServer::start().await;
    let f = Fixture::new(&format!("{}/", server.uri()));
    Mock::given(path("/v1/now"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    f.record("claimed", 1, None, None, 100.0);
    let mut watcher = f.client.watcher();
    let changes = watcher
        .next(&BTreeSet::from([key(1)]), Duration::from_secs(1))
        .await
        .unwrap();
    assert!(!changes.bell_available && changes.reset);
    assert_eq!(
        changes.records[&key(1)].as_ref().unwrap().state,
        ActionState::Claimed
    );
}

#[tokio::test]
async fn failed_record_reads_do_not_advance_the_bell_cursor() {
    let server = MockServer::start().await;
    let f = Fixture::new(&format!("{}/", server.uri()));
    Mock::given(path("/v1/now"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"epoch":EPOCH,"seq":1})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/v1/wait"))
        .and(query_param("since", format!("{EPOCH}:1")))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"epoch":EPOCH,"seq":2,"events":[]})),
        )
        .expect(2)
        .mount(&server)
        .await;
    let awaited = BTreeSet::from([key(1)]);
    let mut watcher = f.client.watcher();
    watcher
        .next(&awaited, Duration::from_secs(1))
        .await
        .unwrap();
    f.record("done", 1, Some("executed"), None, 100.0);
    assert!(
        watcher
            .next(&awaited, Duration::from_secs(1))
            .await
            .is_err()
    );
    f.record("done", 1, Some("executed"), Some(0), 100.0);
    assert!(
        watcher
            .next(&awaited, Duration::from_secs(1))
            .await
            .unwrap()
            .bell_available
    );
}
