//! #730 H15: real HTTP conformance with changing reads and lost delete responses.
use proptest::prelude::*;
use proptest::test_runner::{Config, FileFailurePersistence, RngSeed, TestRunner};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use tl3_github::{AppConfig, ClientOptions, Comment, GitHub};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};
mod legacy_marker;

#[derive(Clone, Debug)]
struct Schedule {
    reads: Vec<Vec<(u8, bool, bool)>>,
    failed_read: Option<usize>,
}
#[derive(Default)]
struct HttpState {
    trace: Vec<Value>,
    persisted: BTreeMap<u64, Value>,
    reads: usize,
    writes: usize,
}
struct Http {
    state: Arc<Mutex<HttpState>>,
    schedule: Schedule,
    fault: Option<(usize, bool)>,
}
fn comment(id: u8, own: bool, marked: bool) -> Value {
    json!({"id": u64::from(id)+100, "body": if marked {GitHub::marker("schedule")} else {"other".into()},
        "user": {"id":99, "login":if own {"factory[bot]"} else {"someone"}},
        "issue_url":"https://api.github.test/repos/owner/sandbox/issues/12",
        "created_at":"2026-10-30T00:00:00Z", "updated_at":"2026-10-30T00:00:00Z"})
}
impl Respond for Http {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let mut state = self.state.lock().unwrap();
        let path = request.url.path();
        // Never record Authorization. It contains a disposable JWT which varies by time.
        state.trace.push(json!([
            request.method.as_str(),
            path,
            request.url.query(),
            serde_json::from_slice::<Value>(&request.body).ok()
        ]));
        if path == "/app/installations/3/access_tokens" {
            return ResponseTemplate::new(201).set_body_json(json!({
                "token":"disposable-schedule-token", "expires_at":"2099-01-01T00:00:00Z"}));
        }
        if request.method == "GET" {
            let index = state.reads;
            state.reads += 1;
            if self.schedule.failed_read == Some(index) {
                return ResponseTemplate::new(503);
            }
            let values: Vec<_> = self.schedule.reads[index % self.schedule.reads.len()]
                .iter()
                .map(|&(id, own, marked)| comment(id, own, marked))
                .collect();
            return ResponseTemplate::new(200).set_body_json(values);
        }
        assert_eq!(request.method, "DELETE");
        let index = state.writes;
        state.writes += 1;
        let fault = self.fault.filter(|(at, _)| *at == index);
        if fault == Some((index, false)) {
            // A crash before the remote mutation lands.
            return ResponseTemplate::new(500);
        }
        let id = path.rsplit('/').next().unwrap().parse::<u64>().unwrap();
        state.persisted.remove(&id);
        if fault == Some((index, true)) {
            // Mutation landed, but its successful response was lost.
            return ResponseTemplate::new(503);
        }
        ResponseTemplate::new(204)
    }
}
#[derive(Debug, PartialEq)]
struct Run {
    trace: Vec<Value>,
    results: Vec<Value>,
    checkpoints: Vec<Vec<u8>>,
    writes: usize,
}
async fn run(schedule: &Schedule, fault: Option<(usize, bool)>, old: bool) -> Run {
    let server = MockServer::start().await;
    let mut initial = HttpState::default();
    for read in &schedule.reads {
        for &(id, own, marked) in read {
            initial
                .persisted
                .insert(u64::from(id) + 100, comment(id, own, marked));
        }
    }
    let state = Arc::new(Mutex::new(initial));
    Mock::given(wiremock::matchers::any())
        .respond_with(Http {
            state: state.clone(),
            schedule: schedule.clone(),
            fault,
        })
        .mount(&server)
        .await;
    let client = GitHub::new(
        AppConfig {
            id: 123,
            slug: "factory".into(),
            pem: include_str!("../../../mergify/tests/fixtures/throwaway-key.pem").into(),
            webhook_secret: "unused-fixture".into(),
        },
        3,
        ClientOptions {
            api_url: format!("{}/", server.uri()).parse().unwrap(),
            mutation_interval: std::time::Duration::ZERO,
            max_retries: 0,
            ..ClientOptions::default()
        },
    )
    .unwrap();
    let mut results = vec![];
    let mut checkpoints = vec![];
    for _ in 0..schedule.reads.len() {
        let result = if old {
            legacy_marker::reconcile(&client).await
        } else {
            client
                .reconcile_comments("owner/sandbox", 12, "schedule")
                .await
        };
        results.push(match result {
            Ok(comment) => json!({"ok":comment}),
            Err(error) => json!({"error":error.to_string()}),
        });
        checkpoints.push(serde_json::to_vec(&state.lock().unwrap().persisted).unwrap());
    }
    let mut state = state.lock().unwrap();
    Run {
        trace: std::mem::take(&mut state.trace),
        results,
        checkpoints,
        writes: state.writes,
    }
}
fn schedules() -> impl Strategy<Value = Schedule> {
    (
        prop::collection::vec(
            prop::collection::vec((0u8..8, any::<bool>(), any::<bool>()), 0..8),
            1..4,
        ),
        prop::option::of(0usize..3),
    )
        .prop_map(|(reads, failed_read)| Schedule { reads, failed_read })
}
fn compare(runtime: &tokio::runtime::Runtime, schedule: &Schedule) -> Result<(), TestCaseError> {
    let old = runtime.block_on(run(schedule, None, true));
    let new = runtime.block_on(run(schedule, None, false));
    prop_assert_eq!(&old, &new, "uninterrupted schedule {:?}", schedule);
    for write in 0..old.writes {
        for landed in [false, true] {
            let old = runtime.block_on(run(schedule, Some((write, landed)), true));
            let new = runtime.block_on(run(schedule, Some((write, landed)), false));
            prop_assert_eq!(
                old,
                new,
                "fault at write {}, landed={}, schedule {:?}",
                write,
                landed,
                schedule
            );
        }
    }
    Ok(())
}
#[test]
fn differential_http_schedule() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    // Mandatory witness reaches a duplicate deletion and a varying second read.
    compare(
        &runtime,
        &Schedule {
            reads: vec![
                vec![(3, true, true), (1, true, true), (2, true, true)],
                vec![(1, true, true), (4, true, true)],
            ],
            failed_read: None,
        },
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("io_schedule-regressions.txt");
    std::fs::write(&path, include_str!("io_schedule-regressions.txt")).unwrap();
    let path = Box::leak(path.to_string_lossy().into_owned().into_boxed_str());
    let mut runner = TestRunner::new(Config {
        cases: 24,
        rng_seed: RngSeed::Fixed(73015),
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(path))),
        ..Config::default()
    });
    runner
        .run(&schedules(), |schedule| compare(&runtime, &schedule))
        .unwrap();
}
#[test]
fn reference_is_verbatim_base_loop() {
    let reference = include_str!("legacy_marker.rs");
    let original = include_str!("legacy_marker_body.txt");
    assert!(reference.contains(original));
    assert!(original.contains("for duplicate in matches.iter().skip(1)"));
}

mod controller;
#[path = "../../src/adapters/github/decide.rs"]
mod decide;
mod fixture;
mod legacy_controller;
mod production_controller;
pub use tl3_controller::{
    Error, evidence, executor, publication_failure, redaction, steering, worker_inputs,
};
pub type Result<T, E = Error> = std::result::Result<T, E>;
fn decode_stored<T: serde::de::DeserializeOwned>(value: Value) -> Result<T> {
    serde_json::from_value(value).map_err(Error::StoredJson)
}
mod workers {
    pub mod writer {
        pub use tl3_controller::adapters::workers::writer::*;
    }
}
mod approval_delivery {
    pub fn is_approval(delivery: &tl3_controller::executor::Delivery) -> bool {
        matches!(
            delivery.output,
            tl3_controller::executor::Output::Effect {
                kind: tl3_kernel::EffectKind::PostApproval,
                ..
            }
        )
    }
}
mod review_stops {
    use serde_json::Value;
    pub fn outcome(context: &Value) -> Option<String> {
        let step = context["review_stop"]["text"].as_str()?;
        Some(format!(
            "Work on this request stopped so that TL3's maintainers can review a failure before anything is retried: {step}."
        ))
    }
    pub fn findings(context: &Value) -> Option<Value> {
        context["review_stop"]
            .get("findings")
            .filter(|f| f.is_object())
            .cloned()
    }
}

use fixture::{
    CloseReason, Delivery, MessageKind, NoteKind, Notice, OptionDraft, Out, Outcome, QKind,
    WriteRequest,
};
