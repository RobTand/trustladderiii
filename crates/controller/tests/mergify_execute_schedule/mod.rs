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
use tl3_mergify::decide::{App, CheckView, Conclusion, Head, Status, Time};
use tl3_mergify::{
    QUEUE_LABEL, QueueState,
    decide::{enqueue as lifecycle, facts},
};
#[path = "../mergify_io/io.rs"]
mod io;
use io::{Clock, GitHub, Mergify, Path, Runtime, Snapshot};
use tl3_github::Comment;
use tl3_mergify::is_merge_queue_pr;
#[path = "../mergify_io/records.rs"]
mod records;
use records::Records;
#[path = "../mergify_io/client.rs"]
mod client;
#[path = "../mergify_io/current-comments.rs"]
mod comments;

pub type Result<T> = ::std::result::Result<T, Error>;
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(&'static str),
    #[error("{0}")]
    Production(#[from] tl3_controller::Error),
    #[error("{source}")]
    Permanent {
        class: &'static str,
        source: io::Message,
    },
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
    fn recorded(&self, snapshot: &Snapshot) -> Result<bool> {
        Ok(snapshot.rows.iter().any(|row| row.table == "queue_facts"))
    }
    fn unresolved_hold(&self, snapshot: &Snapshot) -> Result<bool> {
        Ok(snapshot.held())
    }
}
fn actor(actor: Option<&tl3_github::Actor>) -> facts::Actor {
    facts::Actor::parse(actor.map(|actor| actor.login.as_str()))
}

fn event_kind(event: &str, label: Option<&str>) -> facts::EventKind {
    match event {
        "unlabeled" => facts::EventKind::Unlabeled {
            queue_label: label == Some(QUEUE_LABEL),
        },
        "closed" => facts::EventKind::Closed,
        "merged" => facts::EventKind::Merged,
        _ => facts::EventKind::Other,
    }
}

fn person_facts(
    repo: &Repository,
    number: u64,
    events: &[tl3_github::IssueActivity],
    merged: bool,
) -> Vec<QueueFact> {
    let events: Vec<_> = events
        .iter()
        .map(|event| facts::IssueEvent {
            id: event.id,
            actor: actor(event.actor.as_ref()),
            kind: event_kind(
                &event.event,
                event.label.as_ref().map(|label| label.name.as_str()),
            ),
        })
        .collect();
    facts::person_facts(&events, merged)
        .map(|fact| QueueFact {
            repository: repo.clone(),
            number,
            evidence: match fact {
                facts::PersonFact::Dequeued { event } => Evidence::DequeuedByPerson { event },
                facts::PersonFact::Closed { event } => Evidence::ClosedByPerson { event },
            },
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

struct BodyPolicy {
    rules: Vec<tl3_config::BodyLine>,
    reruns: Vec<String>,
}
struct Branches;
impl Branches {
    fn matches(&self, _: &Delivery, branch: &str) -> Result<bool> {
        Ok(branch == "topic")
    }
}
trait CheckMatches {
    fn matches(&self, check: &CheckRun) -> bool;
}
impl CheckMatches for RequiredCheck {
    fn matches(&self, check: &CheckRun) -> bool {
        self.context == check.name
            && self
                .app
                .is_none_or(|id| check.app.as_ref().is_some_and(|app| app.id == id))
    }
}
pub mod snapshot {
    use super::*;
    pub struct Node {
        pub phase: tl3_kernel::Phase,
    }
    pub struct World {
        pub nodes: Vec<Node>,
    }
    pub(crate) fn decode(s: &Snapshot) -> Result<World> {
        Ok(World {
            nodes: vec![Node {
                phase: if s.rows[0].value["inactive"] == true {
                    tl3_kernel::Phase::Closed(tl3_kernel::Outcome::Stopped)
                } else {
                    tl3_kernel::Phase::Publishing
                },
            }],
        })
    }
}
pub mod evidence {
    use super::*;
    pub use tl3_controller::evidence::publication;
    pub fn validate_publication(d: &Delivery, _: &str, _: bool, _: &str) -> Result<()> {
        assert!(
            d.evidence.is_none(),
            "schedules have no publication obligations"
        );
        Ok(())
    }
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
            policies: Vec<(Repository, BodyPolicy)>,
            branches: Option<Branches>,
        }
        impl Adapter {
            fn new() -> Self {
                Self {
                    clients: [(3, GitHub::new())].into(),
                    runtime: Runtime,
                    records: Records::new(&io::PathBuf::from("root")),
                    epoch: 0,
                    clock: Clock,
                    store: io::has_store().then(|| io::PathBuf::from("store")),
                    policies: vec![(
                        Repository {
                            owner: "owner".into(),
                            name: "sandbox".into(),
                            installation: 3,
                        },
                        BodyPolicy {
                            rules: vec![tl3_config::BodyLine {
                                label: "Change type".into(),
                                values: vec!["fix".into()],
                                default: "fix".into(),
                            }],
                            reruns: vec!["gate.yml".into()],
                        },
                    )],
                    branches: Some(Branches),
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
        }
    };
}
macro_rules! run_entry {
    ($a:ident, $entry:ident, $d:ident, $waiting:expr) => {
        match $entry {
            io::Entry::Read => {
                match (|| { let snapshot=$a.snapshot()?; $a.read($d, snapshot.as_ref()) })() {
                    Ok((state, facts)) => json!({"state":format!("{:?}",state.state),"pull":state.pull_request,"facts":facts}),
                    Err(e) => json!({"error":e.to_string()}),
                }
            },
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
#[path = "base-enqueue.rs.txt"]
mod frozen;

#[allow(clippy::needless_question_mark)]
mod reference {
    use super::frozen as lifecycle;
    adapter!();
    include!("reference.rs");
    mod ci_evidence {
        use crate::mergify_io::io::mock_std as std;
        include!("../mergify_io/current-ci.rs");
    }
    mod waiting {
        include!("../mergify_io/current-waiting.rs");
    }
    mod head_checks {
        include!("../mergify_io/current-head.rs");
    }
    pub(super) fn run(entry: io::Entry, d: &Delivery) -> Value {
        let mut a = Adapter::new();
        run_entry!(a, entry, d, None)
    }
}
#[allow(clippy::needless_question_mark)]
mod current {
    adapter!();
    include!("current.rs");
    mod ci_evidence {
        use crate::mergify_io::io::mock_std as std;
        include!("../mergify_io/current-ci.rs");
    }
    mod waiting {
        include!("../mergify_io/current-waiting.rs");
    }
    mod head_checks {
        include!("../mergify_io/current-head.rs");
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
        test_runner::{Config, FileFailurePersistence, RngSeed, TestRunner},
    };

    fn check(entry: Entry, profile: Profile) {
        let directory = ::std::env::var_os("TEST_UNDECLARED_OUTPUTS_DIR")
            .map(::std::path::PathBuf::from)
            .unwrap_or_else(|| {
                ::std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/mergify_execute_schedule")
            });
        let path = directory.join("proptest-regressions.txt");
        let committed = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/mergify_execute_schedule/proptest-regressions.txt"
        );
        if path != ::std::path::Path::new(committed) {
            ::std::fs::write(&path, include_str!("proptest-regressions.txt")).unwrap();
        }
        let path = Box::leak(path.to_string_lossy().into_owned().into_boxed_str());
        let mut runner = TestRunner::new(Config {
            cases: 128,
            max_shrink_iters: 2048,
            rng_seed: RngSeed::Fixed(0x0730_cc01),
            failure_persistence: Some(Box::new(FileFailurePersistence::Direct(path))),
            ..Config::default()
        });
        let frame =
            (any::<u16>(), 0u32..300_002).prop_map(|(word, elapsed_ms)| Frame { word, elapsed_ms });
        let tape = prop_oneof![
            2 => Just(vec![]),
            3 => (0usize..128, frame.clone()).prop_map(|(prefix, frame)| {
                let mut frames = vec![Frame { word: 0, elapsed_ms: 0 }; prefix];
                frames.push(frame);
                frames
            }),
            3 => prop::collection::vec(frame, 0..128),
        ];
        let strategy = (
            any::<u8>(),
            tape,
            prop::collection::vec(0u8..7, 0..8),
            prop::collection::vec(0u8..4, 0..48),
        )
            .prop_map(
                move |(initial, frames, write_results, file_results)| Schedule {
                    profile,
                    initial,
                    frames,
                    appears_at: usize::MAX,
                    write_results,
                    file_results,
                    overrides: vec![],
                },
            );
        let comparisons = ::std::cell::Cell::new(0);
        runner
            .run(&strategy, |schedule| {
                comparisons.set(
                    comparisons.get()
                        + io::compare(&schedule, entry).map_err(TestCaseError::fail)?,
                );
                Ok(())
            })
            .unwrap();
        eprintln!(
            "CC-M1 {entry:?}/{profile:?}: 128 schedules, {} complete/crash comparisons",
            comparisons.get()
        );
    }

    /// State-table Effects: all reads, writes and crash-persisted bytes match cdacce8f99.
    #[test]
    fn execute_schedules() {
        for profile in [
            Profile::Label,
            Profile::Command,
            Profile::Refusal,
            Profile::Active,
            Profile::Terminal,
            Profile::Merged,
        ] {
            check(Entry::Execute, profile);
        }
    }

    /// Publishing step 4 and human holds: repeated file and upstream reads may change.
    #[test]
    fn completion_schedules() {
        for profile in [
            Profile::Plain,
            Profile::Flip,
            Profile::Active,
            Profile::Terminal,
            Profile::MergeFalse,
            Profile::Refusal,
        ] {
            check(Entry::Completion, profile);
        }
    }

    /// #760: the second recovery-file read remains a fresh sample.
    #[test]
    fn recovery_appears_between_samples() {
        for appears_at in 1..4 {
            let schedule = Schedule {
                profile: Profile::Flip,
                initial: 0,
                frames: vec![],
                appears_at,
                write_results: vec![],
                file_results: vec![],
                overrides: vec![],
            };
            io::compare(&schedule, Entry::Completion).unwrap();
        }
    }

    fn function<'a>(source: &'a str, name: &str, production: bool) -> &'a str {
        let offset = if production && ["execute", "reconcile"].contains(&name) {
            source.find("impl Executor for Adapter {").unwrap()
        } else {
            0
        };
        let start = source[offset..].find(&format!("    fn {name}(")).unwrap() + offset;
        let end = source[start..].find("\n    }\n").unwrap() + start + 7;
        &source[start..end]
    }

    /// CC-M1: both compiled shells are verbatim bodies, with a frozen legacy core.
    #[test]
    fn compiled_shells_match_their_sources() {
        for (source, compiled) in [
            (
                include_str!("../../src/adapters/mergify.rs"),
                include_str!("current.rs"),
            ),
            (
                include_str!("base-adapter.rs.txt"),
                include_str!("reference.rs"),
            ),
        ] {
            for name in [
                "read",
                "heal_body",
                "phase",
                "completion",
                "execute",
                "reconcile",
                "read_enqueue",
                "baseline",
                "dequeue_pending",
                "snapshot",
                "fence",
            ] {
                assert_eq!(
                    function(source, name, true),
                    function(compiled, name, false),
                    "{name}"
                );
            }
        }
        assert_eq!(
            format!(
                "{:x}",
                Sha256::digest(include_bytes!("base-adapter.rs.txt"))
            ),
            "4c23cc4a17eb17793dfe1cfc4b911de49660267bbebb306269e8bc673eb255f9"
        );
        assert_eq!(
            format!(
                "{:x}",
                Sha256::digest(include_bytes!("base-enqueue.rs.txt"))
            ),
            "522ef8320a2210882c79bf978db461de8430307802b186a4fa48b4a5606a813a"
        );
    }
}
