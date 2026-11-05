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
use tl3_github::Comment;
use tl3_mergify::is_merge_queue_pr;
mod records;
use records::Records;
#[path = "client.rs"]
mod client;
#[path = "current-comments.rs"]
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
mod reference {
    adapter!();
    include!("reference.rs");
    mod ci_evidence {
        use crate::mergify_io::io::mock_std as std;
        include!("reference-ci.rs");
    }
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
    mod ci_evidence {
        use crate::mergify_io::io::mock_std as std;
        include!("current-ci.rs");
    }
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
        test_runner::{Config, FileFailurePersistence, RngSeed, TestRunner},
    };
    const CASES: u32 = 128;
    fn tape() -> impl Strategy<Value = Vec<Frame>> {
        let frame =
            (any::<u16>(), 0u32..300_002).prop_map(|(word, elapsed_ms)| Frame { word, elapsed_ms });
        prop_oneof![
            2 => Just(vec![]),
            3 => (0usize..128, frame.clone()).prop_map(|(prefix, frame)| {
                let mut tape = vec![Frame { word: 0, elapsed_ms: 0 }; prefix];
                tape.push(frame);
                tape
            }),
            3 => prop::collection::vec(frame, 0..128),
        ]
    }
    fn check(entry: Entry, profile: Profile) {
        let mut runner = TestRunner::new(Config {
            cases: CASES,
            max_shrink_iters: 2048,
            rng_seed: RngSeed::Fixed(0x0075_1760_2026),
            failure_persistence: Some(Box::new(FileFailurePersistence::Direct(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/mergify_io/proptest-regressions.txt"
            )))),
            ..Config::default()
        });
        let strategy = (
            any::<u8>(),
            tape(),
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
                    overrides: vec![],
                    file_results,
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
    /// #760 batch 5: a reread can lose the timer after grace already passed.
    #[test]
    fn waiting_budget_disappears_after_grace() {
        let s: Schedule =
            serde_json::from_str(include_str!("waiting-budget-disappears.json")).unwrap();
        io::compare(&s, Entry::Waiting).unwrap();
    }
    /// #760: Waiting + queue label + Ready intent must sample recovery twice.
    #[test]
    fn recovery_file_can_appear_between_reads() {
        let mut runner = TestRunner::new(Config {
            cases: CASES,
            max_shrink_iters: 2048,
            rng_seed: RngSeed::Fixed(0x0075_1760_2026),
            failure_persistence: Some(Box::new(FileFailurePersistence::Direct(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/mergify_io/proptest-regressions.txt"
            )))),
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
                    overrides: vec![],
                    file_results: vec![],
                };
                io::compare(&s, Entry::Completion).map_err(TestCaseError::fail)?;
                Ok(())
            })
            .unwrap();
    }
    fn stable(profile: Profile) -> Schedule {
        Schedule {
            profile,
            initial: 0,
            frames: vec![],
            appears_at: usize::MAX,
            write_results: vec![],
            file_results: vec![],
            overrides: vec![],
        }
    }
    fn over(s: &mut Schedule, op: &str, suffix: &str, nth: usize, word: u16) {
        s.overrides.push(io::ReadOverride {
            op: op.into(),
            suffix: suffix.into(),
            nth,
            frame: Frame {
                word,
                elapsed_ms: 0,
            },
        });
    }
    /// #760: keep the same disappearance at its leaf even as more I/O is recorded.
    #[test]
    fn waiting_timer_reread_does_not_revoke_grace() {
        let mut s = stable(Profile::Waiting);
        s.initial = 48;
        over(&mut s, "file.read", ".requeue", 2, 1);
        io::compare(&s, Entry::Waiting).unwrap();
        io::require_ops(&s, Entry::Waiting, &["github.command"]);
    }
    /// #760 review: every named generator branch must actually be reached.
    #[test]
    fn mandatory_queue_states_and_classic_status() {
        for (profile, state) in [
            (Profile::Active, "Queued"),
            (Profile::Terminal, "Dequeued"),
            (Profile::Merged, "Merged"),
            (Profile::Classic, "Waiting"),
            (Profile::MergeFalse, "Waiting"),
            (Profile::Draft, "Waiting"),
            (Profile::Missing, "Waiting"),
        ] {
            let s = stable(profile);
            io::compare(&s, Entry::Read).unwrap();
            io::require_field(&s, Entry::Read, "state", json!(state));
            if profile == Profile::Classic {
                io::require_ops(&s, Entry::Read, &["github.statuses"]);
            }
            if profile == Profile::Merged {
                io::require_ops(&s, Entry::Read, &["github.checks"]);
            }
            if profile == Profile::Missing {
                io::require_ops(&s, Entry::Read, &["github.suites"]);
            }
            if profile == Profile::MergeFalse {
                io::require_pull_flag(&s, "mergeable", false);
            }
        }
    }
    /// #447/#760: real body intent, PATCH and stale-workflow reruns stay recorded.
    #[test]
    fn mandatory_body_heal_and_queue_evidence() {
        let s = stable(Profile::Body);
        io::compare(&s, Entry::Read).unwrap();
        io::require_ops(
            &s,
            Entry::Read,
            &[
                "file.read",
                "file.write",
                "github.issue",
                "github.patch_body",
                "github.runs",
                "github.rerun_failed",
            ],
        );
        let s = stable(Profile::Rerun);
        io::compare(&s, Entry::Read).unwrap();
        io::require_ops(
            &s,
            Entry::Read,
            &[
                "github.requirements",
                "github.annotations",
                "github.job_log",
            ],
        );
        let mut s = stable(Profile::Terminal);
        over(&mut s, "github.checks", "@head", 0, 4 << 10);
        io::compare(&s, Entry::Read).unwrap();
        io::require_ops(&s, Entry::Read, &["github.paths"]);
    }
    /// #608/#760: duplicate DELETEs, refusal replies and the POST preflight read.
    #[test]
    fn mandatory_real_comment_helpers() {
        let s = stable(Profile::Duplicates);
        io::compare(&s, Entry::Command).unwrap();
        io::require_ops(&s, Entry::Command, &["github.delete_comment"]);
        let mut s = stable(Profile::Refusal);
        s.initial = 66;
        io::compare(&s, Entry::Completion).unwrap();
        io::require_ops(&s, Entry::Completion, &["github.comments"]);
        io::require_ack(&s, Entry::Completion, "Permanent");
        let s = stable(Profile::Command);
        io::compare(&s, Entry::Command).unwrap();
        io::require_ops(&s, Entry::Command, &["github.command"]);
        io::require_count(&s, Entry::Command, "github.comments", 2);
    }
    /// #760 review: errors return normally and cannot stand in for crash injection.
    #[test]
    fn mandatory_file_errors() {
        for word in [1024, 2048, 4096] {
            let mut s = stable(Profile::Plain);
            over(&mut s, "file.read", ".json", 0, word);
            io::compare(&s, Entry::Completion).unwrap();
            io::require_error(&s, Entry::Completion);
        }
        for failure in [1, 2, 3] {
            let mut s = stable(Profile::Label);
            s.file_results = vec![failure];
            io::compare(&s, Entry::Execute).unwrap();
            io::require_error(&s, Entry::Execute);
            io::require_ops(&s, Entry::Execute, &["file.create"]);
        }
    }
    #[test]
    fn read_schedules() {
        check(Entry::Read, Profile::Body);
        check(Entry::Read, Profile::Classic);
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
                    "command_stage",
                    "legacy_command",
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
    #[test]
    fn expanded_helpers_are_verbatim() {
        for (source, copy) in [
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
                "read_queue",
                "queue_facts",
                "queue_blocked",
                "body_edit_reruns",
                "heal_phase",
                "suite_statuses",
                "checks_passed",
            ] {
                assert_eq!(
                    free_function(source, name),
                    free_function(copy, name),
                    "{name}"
                );
            }
        }
        for (source, copy) in [
            (
                include_str!("../../../github/src/client.rs"),
                include_str!("current-comments.rs"),
            ),
            (
                include_str!("base-github-client.rs.txt"),
                include_str!("reference-comments.rs"),
            ),
        ] {
            for name in [
                "owns_comment",
                "post_command_comment",
                "post_formatted_comment",
                "reconcile_comments",
                "delete_comment",
            ] {
                assert_eq!(
                    async_function(source, name),
                    async_function(copy, name),
                    "{name}"
                );
            }
        }
        assert_eq!(
            include_str!("reference-comments.rs"),
            include_str!("current-comments.rs")
        );
        assert!(
            include_str!("../../src/adapters/mergify/ci_evidence.rs")
                .starts_with(include_str!("current-ci.rs").trim_end())
        );
        assert!(
            include_str!("base-ci-evidence.rs.txt")
                .starts_with(include_str!("reference-ci.rs").trim_end())
        );
    }
    fn free_function<'a>(s: &'a str, name: &str) -> &'a str {
        let at = s.find(&format!("fn {name}(")).unwrap();
        &s[at..at + s[at..].find("\n}\n").unwrap() + 3]
    }
    fn async_function<'a>(s: &'a str, name: &str) -> &'a str {
        let at = s.find(&format!("fn {name}(")).unwrap();
        &s[at..at + s[at..].find("\n    }\n").unwrap() + 7]
    }
    /// #760: reference files are pinned to the reviewed pre-extraction commit.
    #[test]
    fn frozen_source_hashes() {
        for (source, expected) in [
            (
                include_str!("base-ci-evidence.rs.txt"),
                "6c5536439762f6d7a7db7db03bbee0945a9e2829691e16c1e209519e6f44c016",
            ),
            (
                include_str!("base-github-client.rs.txt"),
                "39c8ff8fd6789eb53d701176f8a77d60744718d6f9c4231ccc74d10e9c98f3d8",
            ),
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

// Group B1: append-only recovery-protocol reachability.
mod recovery_reachability {
    use super::*;
    use io::{Entry, Frame, Profile, ReadOverride, Schedule};
    fn stable(profile: Profile) -> Schedule {
        Schedule {
            profile,
            initial: 0,
            frames: vec![],
            appears_at: usize::MAX,
            write_results: vec![],
            overrides: vec![],
            file_results: vec![],
        }
    }
    fn over(s: &mut Schedule, op: &str, suffix: &str, nth: usize, word: u16) {
        s.overrides.push(ReadOverride {
            op: op.into(),
            suffix: suffix.into(),
            nth,
            frame: Frame {
                word,
                elapsed_ms: 0,
            },
        });
    }
    /// #608: every spent head budget prevents another command or archive read.
    #[test]
    fn mandatory_budget_exhausted() {
        for initial in [128, 136, 152, 160, 168] {
            let mut s = stable(Profile::Command);
            s.initial = initial;
            io::compare(&s, Entry::Command).unwrap();
            io::require_no_op(&s, Entry::Command, "github.command");
            io::require_no_op(&s, Entry::Command, "store.effects");
        }
    }
    /// #608: effect-keyed Sending and Refused receipts are adopted, never resent.
    #[test]
    fn mandatory_legacy_adoption() {
        for (initial, command) in [(66, "Sending"), (68, "Refused")] {
            let mut s = stable(Profile::Command);
            s.initial = initial;
            io::compare(&s, Entry::Command).unwrap();
            io::require_recovery(&s, Entry::Command, command, None);
            io::require_no_op(&s, Entry::Command, "github.command");
        }
    }
    /// #608: both the per-head and legacy effect marker establish Sent.
    #[test]
    fn mandatory_sent_detection() {
        for word in [1, 9] {
            let mut s = stable(Profile::Command);
            over(&mut s, "github.comments", "#100", 0, word);
            io::compare(&s, Entry::Command).unwrap();
            io::require_recovery(&s, Entry::Command, "Sent", None);
            io::require_no_op(&s, Entry::Command, "github.command");
        }
    }
    /// #608/#2705: success, 403, other 4xx, rate limit, and unknown outcomes.
    #[test]
    fn mandatory_command_results() {
        for (mode, command) in [
            (0, "Sent"),
            (1, "PermissionDenied"),
            (2, "Rejected"),
            (3, "Sending"),
            (4, "Sending"),
            (6, "Rejected"),
        ] {
            let mut s = stable(Profile::Command);
            s.write_results = vec![mode];
            io::compare(&s, Entry::Command).unwrap();
            io::require_ops(&s, Entry::Command, &["github.command"]);
            io::require_recovery(&s, Entry::Command, command, None);
            if mode == 1 {
                io::require_ack(&s, Entry::Command, "Permanent");
            }
            if mode > 1 {
                io::require_error(&s, Entry::Command);
            }
        }
    }
    /// #447/#608: an ineligible tick resets the existing timer before returning.
    #[test]
    fn mandatory_ineligible_tick() {
        let mut s = stable(Profile::Waiting);
        over(&mut s, "github.issue", "#100", 0, 1);
        io::compare(&s, Entry::Waiting).unwrap();
        io::require_recovery(&s, Entry::Waiting, "Ready", None);
        io::require_no_op(&s, Entry::Waiting, "clock.now");
        io::require_no_op(&s, Entry::Waiting, "github.command");
    }
    /// #608/#751: the Sending save is fenced before POST; held commands never fence.
    #[test]
    fn mandatory_fence_on_sending() {
        let s = stable(Profile::Command);
        io::compare(&s, Entry::Command).unwrap();
        io::require_command_fence(&s);
        let mut s = stable(Profile::Command);
        over(&mut s, "github.events", "#100", 0, 1);
        io::compare(&s, Entry::Command).unwrap();
        io::require_no_op(&s, Entry::Command, "github.command");
        io::require_no_op(&s, Entry::Command, "file.exists");
    }
}
