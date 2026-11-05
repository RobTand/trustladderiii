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
            d.evidence.as_ref().is_none_or(|e| e.goals.is_empty()),
            "schedules have no publication goal obligations"
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
            io::Entry::Webhook | io::Entry::CatchUp | io::Entry::QueueFacts => $a.routing($entry),
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
                    "check_changed",
                    "unresolved_hold",
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

mod a1;

mod a1_schedules {
    use super::*;
    use io::{Entry, Frame, Profile, Schedule};
    use proptest::{
        prelude::*,
        test_runner::{Config, FileFailurePersistence, RngSeed, TestRunner},
    };

    /// #608/#760: exercise completion directly across every retained profile,
    /// changing each leaf read and crashing at every attempted durable write.
    #[test]
    fn mandatory_a1_completion_schedules() {
        let mut runner = TestRunner::new(Config {
            cases: 128,
            rng_seed: RngSeed::Fixed(0x0075_1760_2026),
            failure_persistence: Some(Box::new(FileFailurePersistence::Direct(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/mergify_io/proptest-regressions.txt"
            )))),
            ..Config::default()
        });
        let runs = ::std::cell::Cell::new(0);
        runner
            .run(
                &(
                    any::<u8>(),
                    prop::collection::vec((any::<u16>(), 0u32..300_002), 0..128),
                ),
                |(initial, tape)| {
                    for profile in [
                        Profile::Plain,
                        Profile::Flip,
                        Profile::Body,
                        Profile::Active,
                        Profile::Terminal,
                        Profile::Merged,
                        Profile::MergeFalse,
                        Profile::Refusal,
                    ] {
                        let schedule = Schedule {
                            profile,
                            initial,
                            frames: tape
                                .iter()
                                .map(|&(word, elapsed_ms)| Frame { word, elapsed_ms })
                                .collect(),
                            appears_at: usize::MAX,
                            write_results: vec![],
                            overrides: vec![],
                            file_results: vec![],
                        };
                        runs.set(
                            runs.get()
                                + io::compare(&schedule, Entry::Completion)
                                    .map_err(TestCaseError::fail)?,
                        );
                    }
                    Ok(())
                },
            )
            .unwrap();
        eprintln!(
            "Completion: 1024 schedules; {} complete/crash comparisons",
            runs.get()
        );
    }
}

mod a2;
// A1 r1: the changed hold decision is implemented separately in each shell.
// Node resolution is an unchanged recorded boundary; identity uses real source.
impl QueueFact {
    fn node(&self, snapshot: &Snapshot) -> Result<Option<NodeId>> {
        io::projection("node");
        if let Some(row) = snapshot.rows.iter().find(|row| {
            row.table == "pull_requests" && row.value["number"].as_u64() == Some(self.number)
        }) {
            return Ok(Some(NodeId(
                row.id
                    .parse()
                    .map_err(|_| Error::Invalid("invalid PR binding"))?,
            )));
        }
        Ok(None)
    }
    pub(crate) fn identity(&self) -> String {
        let mut evidence = match &self.evidence {
            Evidence::ChecksUnavailable { permissions } => {
                let mut gaps = permissions.clone();
                gaps.sort();
                gaps.dedup();
                format!(
                    "required-checks-setup:{}",
                    serde_json::to_string(&gaps).unwrap()
                )
            }
            Evidence::CheckMissing { head, required } => {
                format!(
                    "missing-check:{head}:{}",
                    serde_json::to_string(required).unwrap()
                )
            }
            Evidence::CheckReported { head, required } => {
                format!(
                    "check-reported:{head}:{}",
                    serde_json::to_string(required).unwrap()
                )
            }
            Evidence::QueueBlocked {
                head, conditions, ..
            } => format!(
                "queue-blocked:{head}:{}",
                serde_json::to_string(conditions).unwrap()
            ),
            Evidence::Merged { head, .. } => format!("merged:{head}"),
            Evidence::CiFailed {
                check, head_check, ..
            } => {
                format!("{}:{check}", if *head_check { "head-ci" } else { "ci" })
            }
            Evidence::Conflict { check, .. } => format!("conflict:{check}"),
            Evidence::DequeuedByCommand { check, .. } => format!("command-dequeued:{check}"),
            Evidence::DequeuedByPerson { event } => format!("dequeued:{event}"),
            Evidence::ClosedByPerson { event } => format!("closed:{event}"),
            Evidence::PushByOther { head } => format!("push:{head}"),
        };
        // Mergify can reuse one check ID across successive queue attempts.
        if let Evidence::CiFailed {
            completed_at: Some(time),
            ..
        }
        | Evidence::Conflict {
            completed_at: Some(time),
            ..
        }
        | Evidence::DequeuedByCommand {
            completed_at: Some(time),
            ..
        } = &self.evidence
        {
            evidence.push(':');
            evidence.push_str(time);
        }
        format!(
            "{}/{}/{}/{}/{evidence}",
            self.repository.owner.to_ascii_lowercase(),
            self.repository.name.to_ascii_lowercase(),
            self.repository.installation,
            self.number
        )
    }
}

mod a1_r1;

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

// Group B1 r1: real snapshot projection bodies, independently source-pinned.
mod recovery_projections;

// ctrl-core C1: isolated direct evaluators reuse the entire pinned head-check shell.
mod required_c1 {
    use super::*;
    use io::{Entry, Frame, Profile, Schedule};
    use proptest::{
        prelude::*,
        test_runner::{Config, FileFailurePersistence, RngSeed, TestRunner},
    };

    macro_rules! evaluator {
        ($module:ident, $source:literal) => {
            mod $module {
                use super::*;
                use crate::mergify_io::io::mock_std as std;
                adapter!();
                impl Adapter {
                    fn fence(&mut self, delivery: &Delivery) -> Result<()> {
                        if delivery.epoch < self.epoch {
                            return Err(Error::Invalid("Mergify effect epoch is fenced"));
                        }
                        self.records.fence(delivery.epoch)?;
                        self.epoch = delivery.epoch;
                        Ok(())
                    }
                }
                fn lifecycle_head(current: bool) -> tl3_mergify::decide::Head {
                    if current { tl3_mergify::decide::Head::Current } else { tl3_mergify::decide::Head::Other }
                }
                fn lifecycle_error(failure: lifecycle::Failure) -> Error { Error::Invalid(failure.class()) }
                mod head_checks {
                    include!($source);
                    pub(super) fn run(required: &[(String, Option<u64>)], checks: &[CheckRun], unavailable: &[String]) -> Value {
                        let pull = io::c1_pull();
                        let rules = tl3_github::RequiredChecks {
                            checks: required.iter().map(|(context, app)| tl3_github::RequiredStatusCheck {
                                context: context.clone(), integration_id: *app,
                            }).collect(),
                            unavailable: unavailable.to_vec(),
                        };
                        match Runtime.block_on(evaluate(&GitHub::new(), "owner/sandbox", &pull, rules, checks)) {
                            Ok(result) => json!({"ready":result.ready, "unavailable":result.unavailable, "missing":result.missing,
                                "reported":result.reported, "evidence":result.evidence(&pull), "infra":result.infrastructure()}),
                            Err(error) => json!({"error":error.to_string()}),
                        }
                    }
                    pub(super) fn annotations(values: &[CheckAnnotation]) -> bool { runner_failure(values) }
                }
                pub(super) fn run(required: &[(String, Option<u64>)], checks: &[CheckRun], unavailable: &[String]) -> Value {
                    head_checks::run(required, checks, unavailable)
                }
                pub(super) fn annotations(values: &[tl3_github::CheckAnnotation]) -> bool { head_checks::annotations(values) }
            }
        };
    }
    evaluator!(old, "reference-head.rs");
    evaluator!(new, "current-head.rs");

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
    fn evaluate(s: &Schedule, rules: &[(String, Option<u64>)], checks: &[CheckRun]) -> Value {
        io::c1_evaluate(s, |current| {
            if current {
                new::run(rules, checks, &[])
            } else {
                old::run(rules, checks, &[])
            }
        })
    }
    fn runner() -> TestRunner {
        TestRunner::new(Config {
            cases: 128,
            max_shrink_iters: 2048,
            rng_seed: RngSeed::Fixed(0x0075_1760_2026),
            failure_persistence: Some(Box::new(FileFailurePersistence::Direct(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/mergify_io/proptest-regressions.txt"
            )))),
            ..Config::default()
        })
    }

    /// Q52 / #447 / #2705: both refused status reads reach unavailable, without claiming absence.
    #[test]
    fn mandatory_required_statuses_403_and_404() {
        for (word, status) in [(256, 403), (288, 404)] {
            let mut s = stable(Profile::Classic);
            over(&mut s, "github.statuses", "@head", 0, word);
            io::compare(&s, Entry::Read).unwrap();
            io::require_ops(&s, Entry::Read, &["github.statuses"]);
            let result = evaluate(&s, &[("CI".into(), None)], &[]);
            assert_eq!(
                result["unavailable"],
                json!([format!("Commit statuses: read unavailable (HTTP {status})")])
            );
            assert_eq!(result["missing"], json!([]));
            // Existing base behavior, retained for Q27: unknown absence stays ready.
            assert_eq!(result["ready"], true);
        }
    }
    /// Q52 / #447: passing classic statuses count; error/failure count as Tests.
    #[test]
    fn mandatory_required_classic_status() {
        let s = stable(Profile::Classic);
        let result = evaluate(&s, &[("CI".into(), None)], &[]);
        assert_eq!(result["ready"], false);
        assert_eq!(result["infra"], false);
        assert_eq!(result["evidence"]["CiFailed"]["check"], 1);
        let mut s = s;
        over(&mut s, "github.statuses", "@head", 0, 2);
        assert_eq!(evaluate(&s, &[("CI".into(), None)], &[])["ready"], true);
    }
    /// Q47 / #2705: Actions annotations are read only on completed failure.
    #[test]
    fn mandatory_required_runner_failure_and_annotation_gate() {
        let s = stable(Profile::Rerun);
        let check = io::c1_check(10, "CI", Some((1, "github-actions")), Some("failure"), true);
        let result = evaluate(&s, &[("CI".into(), Some(1))], &[check]);
        assert_eq!(result["infra"], true);
        io::compare(&s, Entry::Rerun).unwrap();
        io::require_ops(&s, Entry::Rerun, &["github.annotations", "github.rerun"]);
        for (slug, conclusion, pending) in [
            ("other", Some("failure"), false),
            ("github-actions", Some("success"), false),
            ("github-actions", Some("failure"), true),
        ] {
            let mut s = stable(Profile::Plain);
            over(&mut s, "github.annotations", "", 0, 256);
            let mut check = io::c1_check(10, "CI", Some((1, slug)), conclusion, true);
            if pending {
                check.status = "in_progress".into();
            }
            let result = evaluate(&s, &[("CI".into(), Some(1))], &[check]);
            assert!(
                result.get("error").is_none(),
                "annotations were read despite gate: {result}"
            );
        }
    }
    /// Q47 / #2705: a code failure wins, independently of sorted app order.
    #[test]
    fn mandatory_required_tests_over_infra() {
        for (tests, infra) in [(1, 2), (2, 1)] {
            let checks = [
                io::c1_check(11, "CI", Some((tests, "other")), Some("failure"), true),
                io::c1_check(
                    12,
                    "CI",
                    Some((infra, "github-actions")),
                    Some("timed_out"),
                    true,
                ),
            ];
            let result = evaluate(&stable(Profile::Plain), &[("CI".into(), None)], &checks);
            assert_eq!(result["infra"], false);
            assert_eq!(result["evidence"]["CiFailed"]["check"], 11);
        }
    }
    /// Q52: keys, first equal ID, head filter and last failing payload are observable.
    #[test]
    fn mandatory_required_newest_per_key() {
        let checks = [
            io::c1_check(10, "CI", Some((1, "other")), Some("failure"), true),
            io::c1_check(11, "CI", Some((1, "other")), Some("success"), true),
            io::c1_check(11, "CI", Some((1, "other")), Some("failure"), true),
            io::c1_check(12, "CI", Some((1, "other")), Some("failure"), false),
        ];
        let result = evaluate(&stable(Profile::Plain), &[("CI".into(), Some(1))], &checks);
        assert_eq!(result["ready"], true);
        let other_app = io::c1_check(1, "CI", Some((2, "other")), Some("failure"), true);
        let result = evaluate(
            &stable(Profile::Plain),
            &[("CI".into(), None)],
            &[checks[1].clone(), other_app],
        );
        assert_eq!(result["evidence"]["CiFailed"]["check"], 1);
    }
    /// #2705: no matching suite, stale final read and the fence are all reached.
    #[test]
    fn mandatory_required_rerun_no_suite_writable_and_fence() {
        let mut s = stable(Profile::Rerun);
        over(&mut s, "github.runs", "@head", 0, 2);
        io::compare(&s, Entry::Rerun).unwrap();
        io::c1_require_error(
            &s,
            Entry::Rerun,
            "required check has no matching head workflow run",
        );
        io::require_count(&s, Entry::Rerun, "github.rerun", 0);
        for word in [1, 2, 4] {
            let mut s = stable(Profile::Rerun);
            over(&mut s, "github.pull", "#100", 2, word);
            io::compare(&s, Entry::Rerun).unwrap();
            io::c1_require_error(&s, Entry::Rerun, "PR changed before workflow rerun");
            io::require_count(&s, Entry::Rerun, "github.rerun", 0);
        }
        let s = stable(Profile::Rerun);
        io::compare(&s, Entry::Rerun).unwrap();
        io::c1_require_order(
            &s,
            Entry::Rerun,
            &[
                "github.runs",
                "file.read",
                "github.pull",
                "file.read",
                "file.create",
                "file.fsync",
                "file.rename",
                "file.fsync",
                "github.rerun",
                "file.create",
                "file.rename",
                "file.fsync",
            ],
        );
        let mut waiting = s.clone();
        over(&mut waiting, "github.runs", "@head", 0, 1);
        io::compare(&waiting, Entry::Rerun).unwrap();
        io::require_count(&waiting, Entry::Rerun, "github.rerun", 0);
        io::c1_require_read_count(&waiting, Entry::Rerun, ".fence", 0);
        io::c1_require_read_count(&s, Entry::Rerun, ".fence", 1);
    }
    /// #2705: read/execute entry points retain their head_checks::read gates.
    #[test]
    fn mandatory_required_read_and_execute_paths() {
        let s = stable(Profile::Label);
        io::compare(&s, Entry::Execute).unwrap();
        io::require_ops(
            &s,
            Entry::Execute,
            &["github.requirements", "github.add_label"],
        );
        let mut s = stable(Profile::Rerun);
        over(&mut s, "github.annotations", "/10", 0, 1);
        io::compare(&s, Entry::Execute).unwrap();
        io::require_ops(
            &s,
            Entry::Execute,
            &["github.requirements", "github.checks", "github.annotations"],
        );
    }

    /// Parser conformance covers arbitrary Unicode, truncations, suffixes, mixed
    /// levels and multiple annotations against the pinned production parser.
    #[test]
    fn required_annotation_parser_proptest() {
        let prefixes = [
            "The runner has lost communication with the server",
            "The job was not acquired by Runner of type",
            "The job was abandoned as the runner did not pick it up",
            "The job was not started because recent account payments have failed",
        ];
        let text = prop_oneof![
            any::<String>(),
            (0usize..4, any::<String>())
                .prop_map(move |(i, suffix)| format!("{}{suffix}", prefixes[i])),
            (0usize..4, 0usize..80)
                .prop_map(move |(i, len)| prefixes[i].chars().take(len).collect::<String>())
        ];
        let annotation = (
            prop_oneof![
                Just("failure".to_owned()),
                Just("warning".to_owned()),
                any::<String>()
            ],
            text,
        )
            .prop_map(|(annotation_level, message)| tl3_github::CheckAnnotation {
                annotation_level,
                message,
            });
        runner()
            .run(&prop::collection::vec(annotation, 0..32), |annotations| {
                prop_assert_eq!(
                    old::annotations(&annotations),
                    new::annotations(&annotations)
                );
                for annotation in &annotations {
                    let single = std::slice::from_ref(annotation);
                    prop_assert_eq!(old::annotations(single), new::annotations(single));
                }
                Ok(())
            })
            .unwrap();
    }
    /// Independent varying upstream status/annotation tapes plus arbitrary
    /// required rules and check payloads; full result and recorded trace agree.
    #[test]
    fn required_evaluate_schedules() {
        let check = (
            0u64..8,
            0u8..3,
            0u8..4,
            0u8..12,
            any::<bool>(),
            any::<bool>(),
        )
            .prop_map(|(id, name, app, conclusion, current, pending)| {
                let names = ["CI", "other", ""];
                let conclusions = [
                    None,
                    Some("success"),
                    Some("failure"),
                    Some("neutral"),
                    Some("skipped"),
                    Some("cancelled"),
                    Some("timed_out"),
                    Some("startup_failure"),
                    Some("action_required"),
                    Some("stale"),
                    Some("new_conclusion"),
                    Some(""),
                ];
                let app = match app {
                    0 => None,
                    1 => Some((1, "github-actions")),
                    2 => Some((2, "other")),
                    _ => Some((1, "other")),
                };
                let mut check = io::c1_check(
                    id,
                    names[usize::from(name)],
                    app,
                    conclusions[usize::from(conclusion)],
                    current,
                );
                if pending {
                    check.status = "in_progress".into();
                }
                check
            });
        let rule = (
            0usize..3,
            prop_oneof![Just(None), Just(Some(1)), Just(Some(2))],
        )
            .prop_map(|(name, app)| (["CI", "other", ""][name].to_owned(), app));
        let strategy = (
            prop::collection::vec(rule, 0..8),
            prop::collection::vec(check, 0..24),
            prop::collection::vec(any::<u16>(), 0..32),
        );
        runner()
            .run(&strategy, |(rules, checks, words)| {
                let mut s = stable(Profile::Classic);
                s.frames = words
                    .into_iter()
                    .map(|word| Frame {
                        word,
                        elapsed_ms: 0,
                    })
                    .collect();
                evaluate(&s, &rules, &checks);
                Ok(())
            })
            .unwrap();
    }
}

// ctrl-core C1 r1: whole-shell payload selection and rerun response coverage.
#[cfg(test)]
mod required_payload_c1 {
    use super::*;
    use io::c1_payloads::{self as payload, Outcome, Payloads};
    use io::{Profile, Schedule};
    use proptest::{
        prelude::*,
        test_runner::{Config, FileFailurePersistence, RngSeed, TestRunner},
    };

    macro_rules! shell {
        ($module:ident, $source:literal) => {
            mod $module {
                use super::*;
                use payload::GitHub;
                use crate::mergify_io::io::mock_std as std;
                adapter!();
                impl Adapter {
                    fn fence(&mut self, delivery: &Delivery) -> Result<()> {
                        if delivery.epoch < self.epoch { return Err(Error::Invalid("Mergify effect epoch is fenced")); }
                        self.records.fence(delivery.epoch)?;
                        self.epoch = delivery.epoch;
                        Ok(())
                    }
                }
                fn lifecycle_head(current: bool) -> tl3_mergify::decide::Head {
                    if current { tl3_mergify::decide::Head::Current } else { tl3_mergify::decide::Head::Other }
                }
                fn lifecycle_error(failure: lifecycle::Failure) -> Error { Error::Invalid(failure.class()) }
                mod head_checks {
                    include!($source);
                    pub(super) fn evaluate_payload() -> Value {
                        let pull = io::c1_pull();
                        let rules = tl3_github::RequiredChecks { checks:vec![tl3_github::RequiredStatusCheck {
                            context:"CI".into(), integration_id:None }], unavailable:vec![] };
                        match Runtime.block_on(evaluate(&GitHub::new(), "owner/sandbox", &pull, rules, &[])) {
                            Ok(r) => json!({"ready":r.ready,"evidence":r.evidence(&pull),"missing":r.missing}),
                            Err(e) => json!({"error":e.to_string()}),
                        }
                    }
                }
                pub(super) fn evaluate(_: &Delivery) -> Value { head_checks::evaluate_payload() }
                pub(super) fn rerun(d: &Delivery) -> Value {
                    let state = tl3_mergify::Observation { pull_request:io::c1_pull(), state:QueueState::Unknown,
                        check:None, check_runs:vec![], unapproved_merge:false, dequeue_rule_concluded:false };
                    match Adapter::new().rerun_head_checks(d, &state) {
                        Ok(()) => json!("done"), Err(e) => json!({"error":e.to_string()}),
                    }
                }
            }
        };
    }
    shell!(old, "reference-head.rs");
    shell!(new, "current-head.rs");
    fn tape(profile: Profile) -> Payloads {
        Payloads {
            schedule: Schedule {
                profile,
                initial: 0,
                frames: vec![],
                appears_at: usize::MAX,
                write_results: vec![],
                overrides: vec![],
                file_results: vec![],
            },
            statuses: vec![],
            runs: vec![],
            outcome: Outcome::Accepted,
        }
    }
    fn status(id: u64, state: &str, at: &str) -> Value {
        json!({"id":id,"context":"CI","state":state,"updated_at":at})
    }
    fn evaluate(p: &Payloads) -> payload::Report {
        payload::compare(p, |current, d| {
            if current {
                new::evaluate(d)
            } else {
                old::evaluate(d)
            }
        })
    }
    fn rerun(p: &Payloads) -> payload::Report {
        payload::compare(p, |current, d| {
            if current {
                new::rerun(d)
            } else {
                old::rerun(d)
            }
        })
    }
    fn runner() -> TestRunner {
        TestRunner::new(Config {
            cases: 128,
            max_shrink_iters: 2048,
            rng_seed: RngSeed::Fixed(0x0075_1760_2026),
            failure_persistence: Some(Box::new(FileFailurePersistence::Direct(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/mergify_io/proptest-regressions.txt"
            )))),
            ..Config::default()
        })
    }
    /// Q52 / #447: status IDs select winners, ties keep the first payload, timestamps do not sort.
    #[test]
    fn mandatory_required_status_payload_winners_and_ties() {
        let early = "2026-11-01T00:00:00Z";
        let late = "2026-11-02T00:00:00Z";
        let cases = [
            (
                vec![
                    status(1, "error", late),
                    status(3, "pending", early),
                    status(2, "success", late),
                ],
                false,
                None,
            ),
            (
                vec![
                    status(1, "success", late),
                    status(3, "error", early),
                    status(2, "pending", late),
                ],
                false,
                Some((3, early)),
            ),
            (
                vec![status(3, "success", early), status(3, "error", late)],
                true,
                None,
            ),
            (
                vec![status(3, "error", early), status(3, "success", early)],
                false,
                Some((3, early)),
            ),
            (
                vec![status(3, "pending", early), status(3, "success", early)],
                false,
                None,
            ),
            (
                vec![status(1, "success", early), status(2, "error", early)],
                false,
                Some((2, early)),
            ),
        ];
        for (mut statuses, ready, evidence) in cases {
            let mut unrelated = status(99, "failure", late);
            unrelated["context"] = json!("unrequired");
            statuses.push(unrelated);
            let mut p = tape(Profile::Classic);
            p.statuses = vec![statuses];
            for side in evaluate(&p).sides {
                assert_eq!(side.result["ready"], ready);
                assert_eq!(side.result["missing"], json!([]));
                if let Some((id, at)) = evidence {
                    assert_eq!(side.result["evidence"]["CiFailed"]["check"], id);
                    assert_eq!(
                        side.result["evidence"]["CiFailed"]["completed_at"],
                        at.replace('Z', "+00:00")
                    );
                    assert_eq!(side.result["evidence"]["CiFailed"]["infra"], false);
                } else {
                    assert_eq!(side.result["evidence"], Value::Null);
                }
            }
        }
    }
    /// #2705: workflow selection ignores event/conclusion and keeps the last maximum eligible ID.
    #[test]
    fn mandatory_required_workflow_payload_newest_and_last_equal() {
        let first = payload::workflow(9, "push", "failure", 2);
        let last = payload::workflow(9, "workflow_dispatch", "success", 3);
        for (ties, winner) in [
            (vec![first.clone(), last.clone()], last.clone()),
            (vec![last, first.clone()], first),
        ] {
            let mut wrong_head = payload::workflow(99, "pull_request", "cancelled", 1);
            wrong_head.head_sha = "other".into();
            let mut wrong_suite = payload::workflow(100, "push", "failure", 1);
            wrong_suite.check_suite_id = 99;
            let mut runs = vec![
                payload::workflow(2, "pull_request", "timed_out", 1),
                wrong_head,
                ties[0].clone(),
                wrong_suite,
                payload::workflow(7, "push", "neutral", 1),
                ties[1].clone(),
                payload::workflow(1, "pull_request", "failure", 1),
            ];
            for newest in [false, true] {
                let expected = if newest {
                    payload::workflow(10, "schedule", "cancelled", 4)
                } else {
                    winner.clone()
                };
                if newest {
                    runs.insert(1, expected.clone());
                }
                let mut p = tape(Profile::Rerun);
                p.runs = vec![runs.clone()];
                let report = rerun(&p);
                assert_eq!(report.receipt_crashes, 2);
                for side in report.sides {
                    assert_eq!(side.result, json!("done"));
                    assert_eq!(side.posts, vec![format!("owner/sandbox/{}", expected.id)]);
                    let receipt = side.receipt.unwrap();
                    assert_eq!(receipt[0]["run"], serde_json::to_value(&expected).unwrap());
                    assert_eq!(receipt[0]["accepted"], true);
                }
            }
        }
    }
    /// #2705: 408 keeps an unknown intent; every non-progress Actions refusal clears it.
    #[test]
    fn mandatory_required_rerun_408_and_actions_receipts() {
        for outcome in [
            Outcome::Http408,
            Outcome::AppAccess,
            Outcome::RunExpired,
            Outcome::OtherRefusal,
        ] {
            let mut p = tape(Profile::Rerun);
            p.runs = vec![vec![payload::workflow(8, "pull_request", "failure", 1)]];
            p.outcome = outcome;
            let report = rerun(&p);
            assert_eq!(report.receipt_crashes, 2);
            assert!(
                report.comparisons >= 23,
                "both atomic writes and POST must be crash-tested"
            );
            for side in report.sides {
                let error = match outcome {
                    Outcome::Http408 => "GitHub returned HTTP 408",
                    Outcome::AppAccess => "resource not accessible by integration",
                    Outcome::RunExpired => "workflow run is older than 30 days",
                    Outcome::OtherRefusal => "workflow or repository restrictions",
                    Outcome::Accepted => unreachable!(),
                };
                assert!(
                    side.result["error"].as_str().unwrap().contains(error),
                    "{:?}",
                    side.result
                );
                assert_eq!(side.posts, vec!["owner/sandbox/8"]);
                let receipt = side.receipt.unwrap();
                if matches!(outcome, Outcome::Http408) {
                    assert_eq!(receipt[0]["accepted"], false);
                    assert_eq!(receipt[0]["waiting_for_completion"], false);
                    assert_eq!(receipt[0]["run"]["id"], 8);
                } else {
                    assert_eq!(receipt, json!([]));
                }
            }
            eprintln!(
                "C1 r1 {outcome:?}: {} comparisons, {} receipt rename crash points",
                report.comparisons, report.receipt_crashes
            );
        }
    }
    #[test]
    fn required_status_payload_tapes_proptest() {
        let item = (0u64..8, 0usize..4, any::<bool>(), any::<bool>()).prop_map(
            |(id, state, late, other)| {
                let mut s = status(
                    id,
                    ["success", "failure", "pending", "error"][state],
                    if late {
                        "2026-11-02T00:00:00Z"
                    } else {
                        "2026-11-01T00:00:00Z"
                    },
                );
                if other {
                    s["context"] = json!("other");
                }
                s
            },
        );
        runner()
            .run(
                &(
                    prop::collection::vec(item.clone(), 0..16),
                    prop::collection::vec(item, 0..16),
                ),
                |(first, second)| {
                    let mut p = tape(Profile::Classic);
                    p.statuses = vec![first, second];
                    payload::compare(&p, |current, d| {
                        json!([
                            if current {
                                new::evaluate(d)
                            } else {
                                old::evaluate(d)
                            },
                            if current {
                                new::evaluate(d)
                            } else {
                                old::evaluate(d)
                            }
                        ])
                    });
                    Ok(())
                },
            )
            .unwrap();
    }
    #[test]
    fn required_workflow_payload_tapes_proptest() {
        let item = (
            1u64..8,
            0usize..3,
            0usize..4,
            any::<bool>(),
            any::<bool>(),
            1u64..4,
        )
            .prop_map(|(id, event, conclusion, head, suite, attempt)| {
                let mut r = payload::workflow(
                    id,
                    ["push", "pull_request", "workflow_dispatch"][event],
                    ["success", "failure", "cancelled", "timed_out"][conclusion],
                    attempt,
                );
                if !head {
                    r.head_sha = "other".into();
                }
                if !suite {
                    r.check_suite_id = 99;
                }
                r
            });
        let comparisons = ::std::cell::Cell::new(0);
        runner()
            .run(
                &(prop::collection::vec(item, 0..16), 0usize..5),
                |(runs, outcome)| {
                    let mut p = tape(Profile::Rerun);
                    p.runs = vec![runs];
                    p.outcome = [
                        Outcome::Accepted,
                        Outcome::Http408,
                        Outcome::AppAccess,
                        Outcome::RunExpired,
                        Outcome::OtherRefusal,
                    ][outcome];
                    comparisons.set(comparisons.get() + rerun(&p).comparisons);
                    Ok(())
                },
            )
            .unwrap();
        eprintln!(
            "C1 r1 workflow payloads: 128 schedules; {} complete/crash comparisons",
            comparisons.get()
        );
    }
}

// ctrl-core A2: independent source inclusions for configurable shell witnesses.
#[derive(Clone, Copy, Default)]
struct A2Config {
    epoch: i64,
    no_policy: bool,
    no_branches: bool,
    foreign: bool,
    empty_reruns: bool,
    inactive: bool,
    saved_heal: Option<(bool, bool)>,
}
macro_rules! a2_entry {
    ($waiting:expr) => {
        pub(super) fn a2_run(entry: io::Entry, d: &Delivery, config: A2Config, heal: bool) -> Value {
            if let Some((same_head, reran)) = config.saved_heal { io::a2_seed_heal(same_head, reran); }
            let mut a = Adapter::new();
            a.epoch = config.epoch;
            if config.no_policy { a.policies.clear(); }
            if config.no_branches { a.branches = None; }
            if config.empty_reruns { a.policies[0].1.reruns.clear(); }
            if heal {
                let mut pull = io::c1_pull();
                if config.foreign { pull.head.branch = "foreign".into(); }
                let snapshot = Snapshot { rows: vec![io::Row { table: "nodes", id: "0".into(), value: json!({"inactive":config.inactive}), version: io::Version(0) }], repaired: false };
                match a.heal_body(d, &pull, Some(&snapshot)) {
                    Ok(edited) => json!({"edited":edited}),
                    Err(error) => json!({"error":error.to_string()}),
                }
            } else {
                run_entry!(a, entry, d, $waiting)
            }
        }
    }
}
#[allow(clippy::needless_question_mark)]
mod a2_reference {
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
    a2_entry!(false);
    pub(super) fn a2_reruns(
        runs: &[tl3_github::WorkflowRun],
        head: &str,
        workflows: &[String],
    ) -> Vec<u64> {
        body_edit_reruns(runs, head, workflows)
    }
    pub(super) fn a2_phase(phase: Option<tl3_kernel::Phase>) -> bool {
        heal_phase(phase)
    }
}
#[allow(clippy::needless_question_mark)]
mod a2_current {
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
    a2_entry!(None);
    pub(super) fn a2_reruns(
        runs: &[tl3_github::WorkflowRun],
        head: &str,
        workflows: &[String],
    ) -> Vec<u64> {
        body_edit_reruns(runs, head, workflows)
    }
    pub(super) fn a2_phase(phase: Option<tl3_kernel::Phase>) -> bool {
        heal_phase(phase)
    }
}
#[test]
fn a2_boundary_helpers_are_production_source() {
    fn free<'a>(source: &'a str, name: &str) -> &'a str {
        let a = source.find(&format!("fn {name}(")).unwrap();
        let b = a + source[a..].find("\n}\n").unwrap() + 3;
        &source[a..b]
    }
    for name in ["execution_receipt", "heal_record"] {
        assert_eq!(
            free(include_str!("../../src/adapters/mergify.rs"), name),
            free(include_str!("current.rs"), name)
        );
    }
}

// A3 ingress routing uses independently compiled real old/current shell bodies.
mod a3;

// A3: C1 focused head fixtures inherit the same production route namespace.
use tl3_mergify::decide::route;
