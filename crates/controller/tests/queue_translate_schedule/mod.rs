use crate::executor;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};
use tl3_controller::{Repository, executor::Observation};
use tl3_kernel::{Event, Fact, HeadId, NodeId, Reply, World};
use tl3_store::{Commit, RowChange, Snapshot};
#[path = "findings.rs"]
pub mod findings;

pub type Result<T> = std::result::Result<T, Error>;
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(&'static str),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Store(#[from] tl3_store::Error),
    #[error("{0}")]
    Worktree(#[from] tl3_worktrees::Error),
}
pub fn decode_stored<T: serde::de::DeserializeOwned>(value: Value) -> Result<T> {
    Ok(serde_json::from_value(value)?)
}
#[derive(Deserialize)]
pub struct Request {
    pub repo: Repository,
}
pub fn request(snapshot: &Snapshot, node: NodeId) -> Result<Request> {
    decode_stored(
        snapshot
            .rows
            .iter()
            .find(|r| r.table == "requests" && r.id == node.0.to_string())
            .ok_or(Error::Invalid("missing request binding"))?
            .value
            .clone(),
    )
}
fn put(table: &str, id: String, value: Value) -> RowChange {
    record("row.put", json!([table, id, value]));
    RowChange::Put {
        table: table.into(),
        id,
        value,
    }
}
fn evidence_key(observation: &Observation) -> Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(observation)?)
    ))
}
fn inbox_key(key: &str, index: usize, observation: &Observation) -> Result<String> {
    Ok(format!("{key}:{index:08}:{}", evidence_key(observation)?))
}
fn observation_upstream(
    _: &Snapshot,
    observation: &Observation,
) -> Result<Option<executor::Upstream>> {
    Ok(Some(match observation {
        Observation::Queue(_) => executor::Upstream::Mergify,
        Observation::GitHubComment { .. } => executor::Upstream::GitHub,
        _ => return Ok(None),
    }))
}
#[path = "ci.rs"]
pub mod ci_boundary;
pub mod adapters {
    pub use tl3_controller::adapters::prismabuild;
    pub mod mergify {
        pub use super::super::ci_boundary as ci_evidence;
        pub use tl3_controller::adapters::mergify::{Evidence, QueueFact, RequiredCheck};
    }
}
trait EvidenceExt {
    fn kind(&self) -> tl3_mergify::decide::translate::Kind;
}
impl EvidenceExt for adapters::mergify::Evidence {
    fn kind(&self) -> tl3_mergify::decide::translate::Kind {
        use adapters::mergify::Evidence as E;
        use tl3_mergify::decide::translate::Kind as K;
        match self {
            E::ChecksUnavailable { .. } => K::ChecksUnavailable,
            E::CheckMissing { .. } => K::CheckMissing,
            E::QueueBlocked { .. } => K::QueueBlocked,
            E::CheckReported { .. } => K::CheckReported,
            E::Merged { checks_passed, .. } => K::Merged {
                checks_passed: *checks_passed,
            },
            E::CiFailed {
                head_check, infra, ..
            } => K::CiFailed {
                head_check: *head_check,
                infra: *infra,
            },
            E::Conflict { .. } => K::Conflict,
            E::DequeuedByCommand { .. } => K::DequeuedByCommand,
            E::DequeuedByPerson { .. } => K::DequeuedByPerson,
            E::ClosedByPerson { .. } => K::ClosedByPerson,
            E::PushByOther { .. } => K::PushByOther,
        }
    }
}
trait QueueFactExt {
    fn node(&self, snapshot: &Snapshot) -> Result<Option<NodeId>>;
    fn setup(&self) -> bool;
    fn recorded(&self, snapshot: &Snapshot) -> Result<bool>;
    fn identity(&self) -> String;
}
#[path = "base-core.rs"]
pub mod base_core;
mod queue_fact;

pub mod looks {
    use super::*;
    pub fn missing_check_incident_id(
        node: NodeId,
        head: &str,
        required: &crate::adapters::mergify::RequiredCheck,
    ) -> String {
        use sha2::{Digest, Sha256};
        let gap = serde_json::to_string(&(head, required)).expect("serializable check");
        format!(
            "missing-check:{}:{:x}",
            node.0,
            Sha256::digest(gap.as_bytes())
        )
    }

    pub fn queue_blocked_incident_id(node: NodeId, head: &str, conditions: &[String]) -> String {
        use sha2::{Digest, Sha256};
        let gap = serde_json::to_string(&(head, conditions)).expect("serializable conditions");
        format!(
            "queue-blocked:{}:{:x}",
            node.0,
            Sha256::digest(gap.as_bytes())
        )
    }

    pub fn check_setup_incident_id(node: NodeId, permission: &str) -> String {
        use sha2::{Digest, Sha256};
        let gap = format!("{:x}", Sha256::digest(permission.as_bytes()));
        format!("required-checks:{}:{gap}", node.0)
    }
    #[derive(Clone, Debug, Serialize, Deserialize)]
    pub struct Incident {
        pub open: bool,
        #[serde(flatten)]
        pub extra: BTreeMap<String, Value>,
    }
}
fn check_run(
    snapshot: &Snapshot,
    node: NodeId,
    head: Option<&str>,
) -> Result<Option<tl3_controller::adapters::prismabuild::CheckRun>> {
    let Some(head) = head else {
        return Ok(None);
    };
    snapshot
        .rows
        .iter()
        .find(|r| r.table == "check_plans" && r.id == format!("{}:{head}", node.0))
        .map(|r| decode_stored(r.value.clone()))
        .transpose()
}
fn commit_sha(snapshot: &Snapshot, head: Option<HeadId>) -> Option<&str> {
    let head = head?;
    snapshot
        .interned
        .iter()
        .find(|i| i.kind == tl3_store::InternKind::Commit && i.id == head.0 as i64)
        .map(|i| i.value.as_str())
}

#[derive(Clone, Debug)]
struct Tape {
    reads: Vec<u8>,
    errors: Vec<bool>,
    snapshot: Snapshot,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Outcome {
    calls: Vec<(String, Value)>,
    persisted: BTreeMap<String, Vec<u8>>,
    writes: usize,
    result: Value,
}
struct Recording {
    tape: Tape,
    outcome: Outcome,
    reads: usize,
    cut: Option<(usize, bool)>,
}
thread_local! { static IO: RefCell<Option<Recording>> = const { RefCell::new(None) }; }
fn with<T>(f: impl FnOnce(&mut Recording) -> T) -> T {
    IO.with(|io| f(io.borrow_mut().as_mut().unwrap()))
}
fn record(op: &str, args: Value) {
    with(|r| r.outcome.calls.push((op.into(), args)));
}
fn read(op: &str, args: Value) -> Result<u8> {
    let (value, error) = with(|r| {
        let index = r.reads;
        r.reads += 1;
        (
            r.tape.reads.get(index).copied().unwrap_or(0),
            r.tape.errors.get(index).copied().unwrap_or(false),
        )
    });
    record(op, json!({"args":args,"result":value,"error":error}));
    if error {
        Err(Error::Invalid("scripted read error"))
    } else {
        Ok(value)
    }
}
#[derive(Debug)]
struct Crash;
fn write(op: &str, args: Value, change: impl FnOnce(&mut BTreeMap<String, Vec<u8>>)) {
    let stop = with(|r| {
        r.outcome.writes += 1;
        r.cut == Some((r.outcome.writes, false))
    });
    record(op, args);
    if stop {
        std::panic::resume_unwind(Box::new(Crash));
    }
    let stop = with(|r| {
        change(&mut r.outcome.persisted);
        r.cut == Some((r.outcome.writes, true))
    });
    if stop {
        std::panic::resume_unwind(Box::new(Crash));
    }
}
struct Store;
impl Store {
    fn load(&mut self) -> Result<Snapshot> {
        let word = read("store.load", Value::Null)?;
        let mut snapshot = with(|r| r.tape.snapshot.clone());
        snapshot.revision = i64::from(word).try_into()?;
        Ok(snapshot)
    }
    fn comment_retired(&mut self, id: &str) -> Result<bool> {
        Ok(read("store.comment_retired", json!(id))? % 2 != 0)
    }
    fn event_matches(&mut self, aliases: &[String], value: &Value) -> Result<bool> {
        Ok(read("store.event_matches", json!([aliases, value]))? % 2 != 0)
    }
    fn commit(&mut self, commit: &Commit) -> Result<()> {
        let bytes = json!({"event":commit.event,"rows":row_values(&commit.rows),"effects":effect_values(&commit.effects),"revision":commit.expected_revision.get(),"epoch":commit.epoch.get()});
        // All row writes are in one store transaction. Crash cuts before the
        // commit keep the old state; only the transaction completion persists it.
        for stage in [
            "event",
            "rows",
            "effects",
            "transitions",
            "credit",
            "metadata",
        ] {
            write(
                "store.transaction.write",
                json!([stage, commit.event_key, bytes]),
                |_| {},
            );
        }
        write("store.commit", json!([commit.event_key, bytes]), |state| {
            state.insert(
                format!("event:{}", commit.event_key),
                serde_json::to_vec(&bytes).unwrap(),
            );
            for row in &commit.rows {
                match row {
                    RowChange::Put { table, id, value } => {
                        state.insert(
                            format!("row:{table}:{id}"),
                            serde_json::to_vec(value).unwrap(),
                        );
                    }
                    RowChange::Delete { table, id } => {
                        state.remove(&format!("row:{table}:{id}"));
                    }
                }
            }
            for effect in &commit.effects {
                state.insert(
                    format!("effect:{}", effect.key),
                    serde_json::to_vec(&effect.payload).unwrap(),
                );
            }
        });
        Ok(())
    }
    fn intern_commit(&mut self, node: &str, head: &str) -> Result<u64> {
        let word = read("store.intern.lookup", json!([node, head]))?;
        let id = u64::from(word) + 1;
        if word.is_multiple_of(2) {
            write("store.intern.insert", json!([node, head, id]), |_| {});
            write("store.intern.commit", json!([node, head, id]), |state| {
                state.insert(
                    format!("intern:{node}:{head}"),
                    serde_json::to_vec(&id).unwrap(),
                );
            });
        }
        Ok(id)
    }
    fn worker_materials(&mut self, node: &str) -> Result<Option<Value>> {
        let word = read("store.worker_materials", json!(node))?;
        Ok(match word % 4 {
            0 => None,
            1 => Some(json!({"goals":[]})),
            2 => Some(json!({"maker":{"goals":[{"id":"g"}]}})),
            _ => Some(json!({"goals":false})),
        })
    }
    fn worker_document(&mut self, node: &str, kind: &str) -> Result<Option<Value>> {
        let word = read("store.worker_document", json!([node, kind]))?;
        Ok(match word % 4 {
            0 => None,
            1 => Some(json!([])),
            2 => Some(json!([{"prior":word}])),
            _ => Some(json!({})),
        })
    }
}
struct Shell {
    store: Store,
}
impl Shell {
    fn queue_message_reconciliation(
        &mut self,
        _: &Snapshot,
        _: &str,
        observation: &Observation,
    ) -> Result<bool> {
        assert!(
            !matches!(observation, Observation::GitHubComment { comment, .. } if comment.body.contains("tl3:"))
        );
        Ok(false)
    }
    #[allow(clippy::too_many_arguments)]
    fn input_failed(
        &mut self,
        _: &Snapshot,
        _: &str,
        _: Option<()>,
        _: &str,
        _: Option<()>,
        _: executor::Upstream,
        _: Error,
        _: &mut Commit,
    ) -> Result<()> {
        Err(Error::Invalid("TraceLogsCapped is outside Queue schedules"))
    }
    fn retry_held_effects(&mut self, source: executor::Upstream) {
        record("held.release", json!(format!("{source:?}")));
    }
    fn head(&mut self, node: NodeId, sha: &str) -> Result<HeadId> {
        tl3_worktrees::CommitId::new(sha)?;
        Ok(HeadId(self.store.intern_commit(&node.0.to_string(), sha)?))
    }
    fn priority_reserve(&self, _: &Snapshot, event: Event) -> Result<Event> {
        assert!(matches!(event, Event::Fact { .. }));
        Ok(event)
    }
    fn check_setup_incident(
        &self,
        _: &Snapshot,
        commit: &mut Commit,
        node: NodeId,
        permissions: &[String],
    ) -> Result<()> {
        for permission in permissions
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
        {
            commit.rows.push(put(
                "incidents",
                looks::check_setup_incident_id(node, permission),
                json!({"open":true}),
            ));
        }
        Ok(())
    }
    fn missing_check_incident(
        &self,
        _: &Snapshot,
        commit: &mut Commit,
        node: NodeId,
        head: &str,
        required: &adapters::mergify::RequiredCheck,
    ) -> Result<()> {
        commit.rows.push(put(
            "incidents",
            looks::missing_check_incident_id(node, head, required),
            json!({"open":true}),
        ));
        Ok(())
    }
    fn queue_blocked_incident(
        &self,
        _: &Snapshot,
        commit: &mut Commit,
        node: NodeId,
        head: &str,
        conditions: &[String],
        _: &[String],
    ) -> Result<()> {
        commit.rows.push(put(
            "incidents",
            looks::queue_blocked_incident_id(node, head, conditions),
            json!({"open":true}),
        ));
        Ok(())
    }
}

macro_rules! frozen_boundary {
    (reference) => {
        mod tl3_mergify {
            pub mod decide {
                pub use crate::schedule::base_core as translate;
            }
        }
        include!("reference-evidence.rs");
    };
    (current) => {};
}
macro_rules! shell {
    ($module:ident, $file:literal) => {
        mod $module {
            use super::*;
            pub(super) struct Shell(pub(super) super::Shell);
            impl std::ops::Deref for Shell {
                type Target = super::Shell;
                fn deref(&self) -> &Self::Target {
                    &self.0
                }
            }
            impl std::ops::DerefMut for Shell {
                fn deref_mut(&mut self) -> &mut Self::Target {
                    &mut self.0
                }
            }
            frozen_boundary!($module);
            include!($file);
        }
    };
}
shell!(reference, "reference.rs");
shell!(current, "current.rs");

fn base_snapshot() -> Snapshot {
    let row = |table: &str, id: &str, value| tl3_store::WorldRow {
        table: table.into(),
        id: id.into(),
        value,
        version: tl3_store::Revision::ZERO,
        arrival: 0,
    };
    Snapshot {
        revision: tl3_store::Revision::ZERO,
        epoch: tl3_store::Epoch::ZERO,
        rows: vec![
            row("pull_requests", "0", json!({"number":7})),
            row("requests", "0", json!({"repo":repository()})),
        ],
        pending_effects: vec![],
        credit: vec![],
        interned: vec![tl3_store::Interned {
            id: 1,
            kind: tl3_store::InternKind::Commit,
            scope: "0".into(),
            value: head(),
        }],
    }
}
fn repository() -> Repository {
    Repository {
        owner: "owner".into(),
        name: "sandbox".into(),
        installation: 3,
    }
}
fn head() -> String {
    "a".repeat(40)
}
fn observation(kind: u8, salt: u8) -> Observation {
    use adapters::mergify::{Evidence as E, RequiredCheck};
    let required = RequiredCheck {
        context: "ci".into(),
        app: Some(3),
    };
    let evidence = match kind % 11 {
        0 => E::ChecksUnavailable {
            permissions: vec!["statuses".into()],
        },
        1 => E::CheckMissing {
            head: head(),
            required,
        },
        2 => E::QueueBlocked {
            head: head(),
            conditions: vec!["ci".into()],
            failed: vec!["extra".into()],
        },
        3 => E::CheckReported {
            head: head(),
            required,
        },
        4 => E::Merged {
            head: head(),
            checks_passed: salt.is_multiple_of(2),
        },
        5 => E::CiFailed {
            check: 7,
            completed_at: Some(format!("t{salt}")),
            head: head(),
            infra: salt & 1 != 0,
            head_check: salt & 2 != 0,
            details: tl3_controller::findings::QueueCiEvidence {
                complete: true,
                checks: vec![],
                failed_tests: vec![tl3_controller::findings::FailedTest {
                    test: "tests/unit.py::test_x".into(),
                    message: "assertion".into(),
                }],
                changed_paths: Some(vec!["src/changed.py".into()]),
                decision: None,
            },
        },
        6 => E::Conflict {
            check: 7,
            completed_at: Some(format!("t{salt}")),
            head: head(),
        },
        7 => E::DequeuedByCommand {
            check: 7,
            completed_at: None,
            head: head(),
        },
        8 => E::DequeuedByPerson { event: 9 },
        9 => E::ClosedByPerson { event: 10 },
        _ => E::PushByOther { head: head() },
    };
    Observation::Queue(adapters::mergify::QueueFact {
        repository: repository(),
        number: 7,
        evidence,
    })
}
fn world(mode: u8) -> World {
    let mut world = World::new(&[1], 1);
    let mut node = tl3_kernel::Node::new(tl3_kernel::RepoId(0));
    node.phase = match mode % 4 {
        0 => tl3_kernel::Phase::Publishing,
        1 => tl3_kernel::Phase::Queued,
        2 => tl3_kernel::Phase::Closed(tl3_kernel::Outcome::Stopped),
        _ => tl3_kernel::Phase::Making,
    };
    if mode & 4 != 0 {
        node.effect = Some(tl3_kernel::InFlight {
            seq: 0,
            kind: tl3_kernel::EffectKind::Enqueue,
            cancelling: mode & 8 != 0,
        });
    }
    if mode & 16 != 0 {
        node.merged = Some(HeadId(1));
    }
    node.head = Some(HeadId(1));
    node.approved = Some(HeadId(1));
    world.nodes.push(node);
    world
}
fn replay(
    candidate: bool,
    tape: Tape,
    observations: &[(u8, u8)],
    mode: u8,
    cut: Option<(usize, bool)>,
) -> Outcome {
    IO.with(|io| {
        *io.borrow_mut() = Some(Recording {
            tape,
            outcome: Outcome::default(),
            reads: 0,
            cut,
        })
    });
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<Value> {
        let mut world = world(mode);
        let snapshot = with(|r| r.tape.snapshot.clone());
        let mut commit = Commit::new("consume:test", json!({"test":true}), &snapshot);
        let mut returns = vec![];
        macro_rules! execute {
            ($shell:expr) => {{
                let mut shell = $shell;
                for (index, (kind, salt)) in observations.iter().enumerate() {
                    let observation = observation(*kind, *salt);
                    shell.observe(&format!("delivery:{index}"), observation.clone())?;
                    shell.queue_observation(&snapshot, &mut commit, "look", index, &observation)?;
                    returns.push(json!(shell.translate(
                        &snapshot,
                        &mut world,
                        observation,
                        &mut commit
                    )?));
                }
                shell.store.commit(&commit)?;
            }};
        }
        if candidate {
            execute!(current::Shell(Shell { store: Store }));
        } else {
            execute!(reference::Shell(Shell { store: Store }));
        }
        Ok(
            json!({"returns":returns,"rows":row_values(&commit.rows),"effects":effect_values(&commit.effects)}),
        )
    }));
    let result = match result {
        Ok(Ok(value)) => json!({"ok":value}),
        Ok(Err(error)) => json!({"error":error.to_string()}),
        Err(error) if error.is::<Crash>() => json!({"crash":true}),
        Err(error) => std::panic::resume_unwind(error),
    };
    with(|r| {
        r.outcome.result = result;
        r.outcome.clone()
    })
}
fn compare(
    tape: Tape,
    observations: &[(u8, u8)],
    mode: u8,
) -> std::result::Result<(), proptest::test_runner::TestCaseError> {
    use proptest::prelude::*;
    let old = replay(false, tape.clone(), observations, mode, None);
    let new = replay(true, tape.clone(), observations, mode, None);
    prop_assert_eq!(&old, &new);
    for write in 1..=old.writes {
        for after in [false, true] {
            let old = replay(
                false,
                tape.clone(),
                observations,
                mode,
                Some((write, after)),
            );
            let new = replay(true, tape.clone(), observations, mode, Some((write, after)));
            prop_assert_eq!(old, new, "write {}, after {}", write, after);
        }
    }
    Ok(())
}
#[test]
fn changing_reads_and_every_write_crash_match() {
    use proptest::{
        prelude::*,
        test_runner::{Config, FileFailurePersistence, RngSeed, TestRunner},
    };
    let directory = std::env::var_os("TEST_UNDECLARED_OUTPUTS_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/queue_translate_schedule")
        });
    let regression = directory.join("io-schedule-regressions.txt");
    std::fs::write(&regression, include_str!("io-schedule-regressions.txt")).unwrap();
    let path = Box::leak(regression.to_string_lossy().into_owned().into_boxed_str());
    let mut runner = TestRunner::new(Config {
        cases: 64,
        rng_seed: RngSeed::Fixed(0x730cc7),
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(path))),
        ..Config::default()
    });
    runner
        .run(
            &(
                proptest::collection::vec(any::<u8>(), 1..24),
                proptest::collection::vec(any::<bool>(), 0..24),
                proptest::collection::vec((0u8..11, any::<u8>()), 1..5),
                any::<u8>(),
            ),
            |(reads, errors, observations, mode)| {
                compare(
                    Tape {
                        reads,
                        errors,
                        snapshot: base_snapshot(),
                    },
                    &observations,
                    mode,
                )
            },
        )
        .unwrap();
}
#[test]
fn every_queue_kind_is_exercised() {
    for kind in 0..11 {
        for mode in [0, 1, 2, 3, 4, 8, 12, 18] {
            compare(
                Tape {
                    reads: vec![0; 64],
                    errors: vec![],
                    snapshot: base_snapshot(),
                },
                &[(kind, 0)],
                mode,
            )
            .unwrap();
        }
    }
}
fn function<'a>(text: &'a str, name: &str) -> &'a str {
    let start = [
        format!("    fn {name}("),
        format!("    pub fn {name}("),
        format!("    pub(crate) fn {name}("),
    ]
    .iter()
    .find_map(|needle| text.find(needle))
    .unwrap();
    let end = text[start..].find("\n    }\n").unwrap() + start + 7;
    &text[start..end]
}
#[test]
fn compiled_shells_match_their_sources() {
    let current = include_str!("../../src/lib.rs");
    let compiled = include_str!("current.rs");
    for name in ["observe", "queue_observation"] {
        assert_eq!(function(current, name), function(compiled, name));
    }
    let start = "            Observation::Queue(mut fact) => {";
    let end = "            Observation::GitHubComment {";
    let arm = |source: &str| {
        let start = source.find(start).unwrap();
        let end = source[start..].find(end).map_or(
            source[start..]
                .find("            _ => return Ok(None),")
                .unwrap_or(0),
            |n| n,
        );
        source[start..start + end].to_owned()
    };
    assert_eq!(arm(current), arm(compiled));
    for (name, base) in [
        ("observe", include_str!("base-observe.rs.txt")),
        (
            "queue_observation",
            include_str!("base-queue_observation.rs.txt"),
        ),
    ] {
        assert_eq!(base, function(include_str!("reference.rs"), name));
    }
    assert!(include_str!("base-translate.rs.txt").contains(&arm(include_str!("reference.rs"))));
}

fn row_values(rows: &[RowChange]) -> Value {
    Value::Array(
        rows.iter()
            .map(|row| match row {
                RowChange::Put { table, id, value } => json!({"put":[table,id,value]}),
                RowChange::Delete { table, id } => json!({"delete":[table,id]}),
            })
            .collect(),
    )
}
fn effect_values(effects: &[tl3_store::NewEffect]) -> Value {
    Value::Array(
        effects
            .iter()
            .map(|effect| json!({"key":effect.key,"kind":effect.kind,"payload":effect.payload}))
            .collect(),
    )
}

#[test]
fn frozen_base_hashes() {
    for (bytes, expected) in [
        (
            include_bytes!("base-observe.rs.txt").as_slice(),
            "f72d836520d797e05e806f5b92c0550c3139c2cc7f670a63a4d5336330eb55ad",
        ),
        (
            include_bytes!("base-queue_observation.rs.txt").as_slice(),
            "03ada881c326a4a450f303ab0405f3b47a7e84ae9870c2d24e4a183d2567e4e1",
        ),
        (
            include_bytes!("base-translate.rs.txt").as_slice(),
            "72e3f42549eb6a09c17c343cd45df3d3c29f81a25f747f0ef53a262f62900320",
        ),
        (
            include_bytes!("base-adapter.rs.txt").as_slice(),
            "4c23cc4a17eb17793dfe1cfc4b911de49660267bbebb306269e8bc673eb255f9",
        ),
        (
            include_bytes!("base-core.rs").as_slice(),
            "e368ea619196e9fb58cc9a64981c7496a7b1aee3ac2658ac6ba9e1801595848f",
        ),
    ] {
        assert_eq!(format!("{:x}", Sha256::digest(bytes)), expected);
    }
}
