#![allow(dead_code, unused_imports)] // Verbatim reference functions keep their original imports/API.
use ::std::{collections::BTreeMap, time::UNIX_EPOCH};
use executor::{Completion, Delivery, Observation, Reconciliation};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tl3_controller::executor::Output as Out;
use tl3_controller::{
    Repository,
    adapters::mergify::{Evidence, Progress, RequiredCheck},
};
use tl3_github::{CheckRun, IssueActivity, PullRequest};
use tl3_kernel::{Ack, EffectKind, NodeId};
use tl3_mergify::{
    QUEUE_LABEL, QueueState,
    decide::{enqueue as lifecycle, facts},
};
mod io;
use io::{Clock, GitHub, Mergify, Path, Runtime, Snapshot};
mod records;
use records::Records;
#[path = "client.rs"]
mod client;

pub type Result<T> = ::std::result::Result<T, Error>;
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(&'static str),
    #[error("required head checks are still pending")]
    ChecksPending,
    #[error("{0}")]
    Io(#[from] ::std::io::Error),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    GitHub(#[from] ::tl3_github::Error),
    #[error("{0}")]
    Adapter(Box<dyn ::std::error::Error + Send + Sync>),
}
impl Error {
    fn is_github_not_found(&self) -> bool {
        matches!(
            self,
            Self::GitHub(::tl3_github::Error::Http { status: 404 })
        )
    }
}
pub fn decode_stored<T: serde::de::DeserializeOwned>(value: Value) -> Result<T> {
    Ok(serde_json::from_value(value)?)
}
pub mod executor {
    use super::*;
    pub use tl3_controller::executor::{Delivery, Output};
    #[derive(Clone, Debug, Serialize, Deserialize)]
    pub enum Observation {
        Queue(QueueFact),
        Kernel(tl3_kernel::Event),
    }
    #[derive(Clone, Debug, Serialize, Deserialize)]
    pub enum ProgressToken {
        Mergify(Progress),
    }
    #[derive(Clone, Debug, Serialize, Deserialize)]
    pub struct Completion {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub diagnostic: Option<Value>,
        pub ack: Ack,
        pub failure_class: Option<String>,
        pub progress: Option<ProgressToken>,
        pub observations: Vec<Observation>,
    }
    impl Completion {
        pub fn confirmed(observations: Vec<Observation>) -> Self {
            Self {
                diagnostic: None,
                ack: Ack::Confirmed,
                failure_class: None,
                progress: None,
                observations,
            }
        }
        pub fn permanent(class: impl Into<String>) -> Self {
            Self {
                diagnostic: None,
                ack: Ack::Permanent,
                failure_class: Some(class.into()),
                progress: None,
                observations: vec![],
            }
        }
    }
    pub enum Reconciliation {
        Found(Box<Completion>),
        Absent,
        Pending,
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueueFact {
    pub repository: Repository,
    pub number: u64,
    pub evidence: Evidence,
}
impl QueueFact {
    fn unresolved_hold(&self, snapshot: &Snapshot) -> Result<bool> {
        Ok(snapshot.held())
    }
}
fn person_facts(
    repository: &Repository,
    number: u64,
    events: &[IssueActivity],
    _: bool,
) -> Vec<QueueFact> {
    events
        .iter()
        .filter(|e| e.actor.as_ref().is_some_and(|a| a.login == "person"))
        .map(|e| QueueFact {
            repository: repository.clone(),
            number,
            evidence: Evidence::DequeuedByPerson { event: e.id },
        })
        .collect()
}
fn full_name(repo: &Repository) -> String {
    format!("{}/{}", repo.owner, repo.name)
}
fn enqueue(delivery: &Delivery) -> Result<(u64, &str)> {
    Ok((
        delivery
            .pull_request
            .ok_or(Error::Invalid("enqueue needs PR"))?,
        delivery
            .head_sha
            .as_deref()
            .ok_or(Error::Invalid("enqueue needs head"))?,
    ))
}
fn adapter_error(error: ::tl3_mergify::Error) -> Error {
    match error {
        ::tl3_mergify::Error::GitHub(e) => e.into(),
        e => Error::Adapter(Box::new(e)),
    }
}
fn checks_settled(state: &tl3_mergify::Observation) -> bool {
    facts::checks_settled(&tl3_mergify::views(
        &state.check_runs,
        &state.pull_request.head.sha,
        |_| false,
    ))
}
mod tl3_store {
    pub(super) use super::io::Store;
}

macro_rules! adapter {
    () => {
        struct Adapter {
            clients: BTreeMap<u64, GitHub>,
            runtime: Runtime,
            records: Records,
            epoch: i64,
            clock: Clock,
            store: Option<io::PathBuf>,
        }
        impl Adapter {
            fn new() -> Self {
                Self {
                    clients: [(3, GitHub)].into(),
                    runtime: Runtime,
                    records: Records::new(&io::PathBuf::from("root")),
                    epoch: 0,
                    clock: Clock,
                    store: io::has_store().then(|| io::PathBuf::from("store")),
                }
            }
            // These unchanged snapshot projections are supplied by the shared
            // boundary. They sample no I/O; the upstream/file calls remain below.
            fn repaired_head(&self, _: &Delivery, snapshot: Option<&Snapshot>) -> Result<bool> {
                Ok(snapshot.is_some_and(|s| s.repaired))
            }
            fn unresolved_human_hold(
                &self,
                _: &Delivery,
                snapshot: Option<&Snapshot>,
            ) -> Result<bool> {
                Ok(snapshot.is_some_and(Snapshot::held))
            }
            fn human_hold_in_events(
                &self,
                d: &Delivery,
                snapshot: Option<&Snapshot>,
                events: &[IssueActivity],
            ) -> Result<bool> {
                Ok(events
                    .iter()
                    .any(|e| e.actor.as_ref().is_some_and(|a| a.login == "person"))
                    || self.unresolved_human_hold(d, snapshot)?)
            }
            fn read(
                &self,
                d: &Delivery,
                _: Option<&Snapshot>,
            ) -> Result<(tl3_mergify::Observation, Vec<Observation>)> {
                io::read_queue(&self.clients[&3], d)
            }
        }
    };
}
macro_rules! run_entry {
    ($a:ident, $entry:ident, $d:ident, $waiting:expr) => {
        match $entry {
            io::Entry::Completion => io::reconciliation($a.completion($d, None)),
            io::Entry::Execute => io::completed($a.execute($d)),
            io::Entry::Reconcile => io::reconciliation($a.reconcile($d)),
            io::Entry::Waiting => io::reconciliation((|| {
                let snapshot = $a.snapshot()?;
                $a.recover_waiting($d, snapshot.as_ref())
            })()),
            io::Entry::Command => io::reconciliation($a.queue_command($d, $waiting)),
            io::Entry::Rerun => match (|| -> Result<()> {
                let (state, _) = $a.read($d, None)?;
                $a.rerun_head_checks($d, &state)
            })() { Ok(()) => json!("done"), Err(error) => json!({"error": error.to_string()}) },
        }
    };
}
// The mock store returns this module's Error directly; production converts StoreError.
#[allow(clippy::needless_question_mark)]
mod reference {
    adapter!();
    include!("reference.rs");
    mod waiting {
        include!("reference-waiting.rs");
    }
    mod head_checks {
        include!("reference-head.rs");
    }
    pub(super) fn run(entry: io::Entry, d: &Delivery) -> Value {
        let mut a = Adapter::new();
        run_entry!(a, entry, d, false)
    }
}
#[allow(clippy::needless_question_mark)]
mod current {
    adapter!();
    include!("current.rs");
    mod waiting {
        include!("current-waiting.rs");
    }
    mod head_checks {
        include!("current-head.rs");
    }
    pub(super) fn run(entry: io::Entry, d: &Delivery) -> Value {
        let mut a = Adapter::new();
        run_entry!(a, entry, d, None)
    }
}

mod properties {
    use super::*;
    use io::{Entry, Frame, Profile, Schedule};
    use proptest::{
        prelude::*,
        test_runner::{Config, TestRunner},
    };
    const CASES: u32 = 256;
    fn tape() -> impl Strategy<Value = Vec<Frame>> {
        let frame =
            (0u16..512, 0u32..300_002).prop_map(|(word, elapsed_ms)| Frame { word, elapsed_ms });
        prop_oneof![
            2 => Just(vec![]),
            3 => (0usize..48, frame.clone()).prop_map(|(prefix, frame)| {
                let mut tape = vec![Frame { word: 0, elapsed_ms: 0 }; prefix];
                tape.push(frame);
                tape
            }),
            3 => prop::collection::vec(frame, 0..48),
        ]
    }
    fn check(entry: Entry, profile: Profile) {
        let mut runner = TestRunner::new(Config {
            cases: CASES,
            max_shrink_iters: 2048,
            failure_persistence: None,
            ..Config::default()
        });
        let strategy = (any::<u8>(), tape(), prop::collection::vec(0u8..7, 0..8)).prop_map(
            move |(initial, frames, write_results)| Schedule {
                profile,
                initial,
                frames,
                appears_at: usize::MAX,
                write_results,
            },
        );
        let runs = ::std::cell::Cell::new(0);
        runner
            .run(&strategy, |s| {
                runs.set(runs.get() + io::compare(&s, entry).map_err(TestCaseError::fail)?);
                Ok(())
            })
            .unwrap();
        eprintln!(
            "{entry:?}: {CASES} schedules; {} complete/crash comparisons",
            runs.get()
        );
    }
    /// #760: Waiting + queue label + Ready intent must sample recovery twice.
    #[test]
    fn recovery_file_can_appear_between_reads() {
        let mut runner = TestRunner::new(Config {
            cases: CASES,
            max_shrink_iters: 2048,
            failure_persistence: None,
            ..Config::default()
        });
        runner
            .run(&(1usize..4), |appears_at| {
                let s = Schedule {
                    profile: Profile::Flip,
                    initial: 0,
                    frames: vec![],
                    appears_at,
                    write_results: vec![],
                };
                io::compare(&s, Entry::Completion).map_err(TestCaseError::fail)?;
                Ok(())
            })
            .unwrap();
    }
    /// #751 extraction: reads, outcomes and every file/API write crash boundary.
    #[test]
    fn reconcile_schedules() {
        check(Entry::Reconcile, Profile::Plain)
    }
    /// #751/#760: enqueue intent, final head check and durable crash inputs.
    #[test]
    fn execute_schedules() {
        check(Entry::Execute, Profile::Label)
    }
    /// #608: historical command adoption and unknown upstream write outcomes.
    #[test]
    fn command_schedules() {
        check(Entry::Command, Profile::Command)
    }
    /// #608/#760: recovery clock sampling, grace and durable timer resets.
    #[test]
    fn waiting_schedules() {
        check(Entry::Waiting, Profile::Waiting)
    }
    /// #447/#751: workflow receipt ordering and write acceptance.
    #[test]
    fn rerun_schedules() {
        check(Entry::Rerun, Profile::Rerun)
    }

    fn function<'a>(source: &'a str, name: &str, after: &str) -> &'a str {
        let offset = if after.is_empty() {
            0
        } else {
            source.find(after).unwrap()
        };
        let s = &source[offset..];
        let start = s
            .find(&format!("    fn {name}("))
            .or_else(|| s.find(&format!("    pub(super) fn {name}(")))
            .unwrap();
        let end = s[start..].find("\n    }\n").unwrap() + start + 7;
        &s[start..end]
    }
    /// #760: the checked shell copies compile verbatim production function bodies.
    #[test]
    fn current_functions_are_the_production_source() {
        for (actual, copy, names) in [
            (
                include_str!("../../src/adapters/mergify.rs"),
                include_str!("current.rs"),
                vec![
                    "completion",
                    "execute",
                    "reconcile",
                    "read_enqueue",
                    "baseline",
                    "dequeue_pending",
                    "snapshot",
                    "fence",
                ],
            ),
            (
                include_str!("../../src/adapters/mergify/waiting_requeue.rs"),
                include_str!("current-waiting.rs"),
                vec![
                    "recovery_key",
                    "recovery_path",
                    "retain_refused_command",
                    "retain_legacy_command",
                    "recovery",
                    "has_recovery",
                    "save_recovery",
                    "recovery_eligible",
                    "command_budget",
                    "reconcile_command",
                    "recover_waiting",
                    "queue_command",
                    "recovery_decide",
                    "recovery_now_ms",
                ],
            ),
        ] {
            for name in names {
                let after = if ["execute", "reconcile"].contains(&name) {
                    "impl Executor for Adapter {"
                } else {
                    ""
                };
                assert_eq!(
                    function(actual, name, after),
                    function(copy, name, ""),
                    "production copy drifted: {name}"
                );
            }
        }
        assert_eq!(
            include_str!("../../src/adapters/mergify/head_checks.rs"),
            include_str!("current-head.rs")
        );
    }
    /// #760: reference files are pinned to the reviewed pre-extraction commit.
    #[test]
    fn frozen_source_hashes() {
        for (source, expected) in [
            (
                include_str!("base-adapter.rs.txt"),
                "df2eb0d93fe7bf3833a5b9f509c4be466ce4ded5f21809913b715f16f52a966d",
            ),
            (
                include_str!("base-client.rs.txt"),
                "4b768e50d04ae04c02312dafdb5f8e3f8362a5cb6089867a97126642c6cb3a3d",
            ),
            (
                include_str!("base-head.rs.txt"),
                "343dad812ad98c865784f98d52a71998b25b6776913158384995a2f744dd53c7",
            ),
            (
                include_str!("base-records.rs.txt"),
                "6c18ed51bcf362d32a342645c6c830b7c0dd2536b56033b6738dea1211d2aaa4",
            ),
            (
                include_str!("base-waiting.rs.txt"),
                "3a908526a2c29f2435521572e572f03771151555ee5a396b431308ec4cc7a6e5",
            ),
        ] {
            assert_eq!(format!("{:x}", Sha256::digest(source.as_bytes())), expected);
        }
    }
    #[test]
    fn reference_functions_are_the_frozen_source() {
        for (actual, copy, names) in [
            (
                include_str!("base-adapter.rs.txt"),
                include_str!("reference.rs"),
                vec![
                    "completion",
                    "execute",
                    "reconcile",
                    "read_enqueue",
                    "baseline",
                    "dequeue_pending",
                    "snapshot",
                    "fence",
                ],
            ),
            (
                include_str!("base-waiting.rs.txt"),
                include_str!("reference-waiting.rs"),
                vec![
                    "recovery_key",
                    "recovery_path",
                    "retain_refused_command",
                    "retain_legacy_command",
                    "recovery",
                    "has_recovery",
                    "save_recovery",
                    "recovery_eligible",
                    "command_budget",
                    "reconcile_command",
                    "recover_waiting",
                    "queue_command",
                ],
            ),
        ] {
            for name in names {
                let after = if ["execute", "reconcile"].contains(&name) {
                    "impl Executor for Adapter {"
                } else {
                    ""
                };
                assert_eq!(
                    function(actual, name, after),
                    function(copy, name, ""),
                    "reference copy drifted: {name}"
                );
            }
        }
        assert_eq!(
            include_str!("base-head.rs.txt"),
            include_str!("reference-head.rs")
        );
    }
}
