#![allow(
    dead_code,
    reason = "Full retained shells compile paths beyond each generated schedule"
)]
//! Full retained/current methods run through the same recorded I/O boundary.
//! Unchanged helpers are scripted boundary calls. The changed shells, queue
//! submission, transaction envelopes and real enum conversions are compiled.
use super::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};
use tl3_controller::executor::{Delivery, Observation, Output as DeliveryOutput, Upstream};
use tl3_kernel::{Event, NodeId};
use tl3_store::{Commit, NewEffect, RowChange, Snapshot, WorldRow};

type Result<T> = std::result::Result<T, Error>;
#[derive(Debug)]
enum Error {
    Invalid(&'static str),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Diagnostic {
    class: String,
}
mod publication_failure {
    pub(super) use super::Diagnostic;
    use super::*;
    pub fn handles(d: &Delivery) -> bool {
        matches!(
            d.output,
            DeliveryOutput::Effect {
                kind: tl3_kernel::EffectKind::OpenPr,
                ..
            }
        )
    }
}
impl Diagnostic {
    fn capture(_d: &Delivery, _e: Option<&Error>, class: &str) -> Result<Self> {
        Ok(Self {
            class: class.into(),
        })
    }
    fn is_body_refusal(&self) -> bool {
        self.class == "pr_policy"
    }
    fn render(&self) -> String {
        self.class.clone()
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Completion {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    diagnostic: Option<Diagnostic>,
    ack: Ack,
    failure_class: Option<String>,
    progress: Option<Value>,
    observations: Vec<Observation>,
}
trait Postmortem {
    fn is_postmortem(&self) -> bool;
}
impl Postmortem for Delivery {
    fn is_postmortem(&self) -> bool {
        matches!(
            self.output,
            DeliveryOutput::Comment {
                kind: tl3_controller::executor::CommentKind::Postmortem,
                ..
            }
        )
    }
}
fn decode_stored<T: serde::de::DeserializeOwned>(value: Value) -> Result<T> {
    serde_json::from_value(value).map_err(|_| Error::Invalid("decode"))
}
fn put(table: &str, id: String, value: Value) -> RowChange {
    RowChange::Put {
        table: table.into(),
        id,
        value,
    }
}
fn hydrate(effect: &Effect, _snapshot: &Snapshot) -> Result<Delivery> {
    decode_stored(effect.payload.clone())
}
fn evidence_key(o: &Observation) -> Result<String> {
    Ok(serde_json::to_string(o).unwrap())
}
mod snapshot {
    use super::*;
    pub fn decode(_s: &Snapshot) -> Result<()> {
        Ok(())
    }
}
mod approval_delivery {
    use super::*;
    pub const AUTHORIZATIONS: &str = "approval_retry_authorizations";
    pub const LINEAGE_SAFE: &str = "approval_lineage_admitted";
    pub const UNRESOLVED: &str = "approval_dispatch_recovery";
    pub const REPAIRS: &str = "approval_reconciliations";
    pub const AMBIGUOUS: &str = "approval_outcome_unknown";
    pub fn is_approval(d: &Delivery) -> bool {
        matches!(
            d.output,
            DeliveryOutput::Effect {
                kind: tl3_kernel::EffectKind::PostApproval,
                ..
            }
        )
    }
}
mod looks {
    use super::*;
    pub const CHECKS_PENDING: &str = "checks_pending";
    pub struct Signature {
        pub class: String,
    }
    pub struct Incident {
        pub signature: Signature,
        pub open: bool,
        pub effect: Option<String>,
    }
    pub fn outage_waiting(s: &Snapshot, _key: &str) -> bool {
        s.rows.iter().any(|r| r.table == "outage")
    }
    pub fn resolve_effect_incidents(
        _s: &Snapshot,
        c: &mut Commit,
        mut f: impl FnMut(&str, &str) -> Result<bool>,
    ) -> Result<()> {
        let matches = f("incident", "original")?;
        c.rows
            .push(put("resolved", "incident".into(), json!(matches)));
        Ok(())
    }
}
mod evidence {
    use super::*;
    pub fn retire_delivered(_s: &Snapshot, c: &mut Commit, d: &Delivery, ack: Ack) {
        c.rows
            .push(put("evidence_retired", d.key.clone(), json!(ack)));
    }
}
fn queue_retirement(c: &mut Commit, d: &Delivery) {
    if approval_delivery::is_approval(d) {
        for table in [
            approval_delivery::AUTHORIZATIONS,
            approval_delivery::LINEAGE_SAFE,
            "notice_incidents",
        ] {
            c.rows.push(RowChange::Delete {
                table: table.into(),
                id: d.key.clone(),
            });
        }
    }
    if executor::upstream(d.output) == Upstream::Mergify || approval_delivery::is_approval(d) {
        c.rows
            .push(put("retiring_effects", d.key.clone(), json!(d)));
    }
}
mod executor {
    pub use tl3_controller::executor::upstream;
    include!("conversions.rs");
}
mod input_errors {
    use super::*;
    pub const PENDING_KERNEL: &str = "pending_kernel_events";
    #[derive(Clone, Serialize, Deserialize)]
    pub enum PendingKernelInput {
        Completion { key: String, completion: Completion },
        CancelAbsent { key: String },
    }
}
#[derive(Clone, Debug, PartialEq)]
struct Recorded {
    calls: Vec<Value>,
    durable: BTreeMap<String, Vec<u8>>,
    writes: usize,
    reads: usize,
    crash: usize,
    samples: Vec<u8>,
}
type Io = Rc<RefCell<Recorded>>;
impl Recorded {
    fn call(io: &Io, name: &str, args: Value) -> u8 {
        let mut r = io.borrow_mut();
        let value = r.samples[r.reads % r.samples.len()];
        r.reads += 1;
        r.calls
            .push(json!({"read":name,"args":args,"result":value}));
        value
    }
    fn write(io: &Io, name: &str, payload: Value, persist: bool) {
        let mut r = io.borrow_mut();
        r.calls.push(json!({"write":name,"payload":payload}));
        r.writes += 1;
        if persist {
            let key = format!("{}:{name}", r.writes);
            r.durable.insert(key, serde_json::to_vec(&payload).unwrap());
        }
        if r.crash == r.writes {
            std::panic::panic_any("scheduled durable cut");
        }
    }
}
struct Store {
    io: Io,
    pending: Vec<WorldRow>,
    active: bool,
    staged: Vec<(String, Vec<u8>)>,
}
impl Store {
    fn load(&mut self) -> Result<Snapshot> {
        let sample = Recorded::call(&self.io, "store.load", json!(null));
        let mut effect = super::effect("original", super::states()[(sample as usize / 2) % 7]);
        if sample & 1 != 0 {
            effect.key = "other".into();
        }
        let output = if sample & 4 == 0 {
            json!({"Effect":{"node":0,"seq":1,"kind":"PostApproval","head":null}})
        } else {
            json!({"Comment":{"node":0,"kind":"Clarify"}})
        };
        effect.payload = json!({"key":effect.key,"epoch":1,"output":output,"generation":0,"head_sha":null,
            "repository":{"owner":"owner","name":"sandbox","installation":3},"issue":12,"pull_request":null,"paths":[],"attempt":0,"round":0});
        let mut rows = self.pending.clone();
        if sample & 8 != 0 {
            rows.push(WorldRow {
                table: "saved_scratch".into(),
                id: "scratch".into(),
                value: json!({"status":"saved"}),
                version: effect.created_revision,
                arrival: 0,
            });
        }
        if sample & 16 != 0 {
            rows.push(WorldRow {
                table: approval_delivery::REPAIRS.into(),
                id: "original".into(),
                value: effect.payload.clone(),
                version: effect.created_revision,
                arrival: 1,
            });
        }
        if sample & 32 != 0 {
            rows.push(WorldRow {
                table: "outage".into(),
                id: "original".into(),
                value: json!(true),
                version: effect.created_revision,
                arrival: 2,
            });
        }
        Ok(Snapshot {
            revision: tl3_store::Revision::try_from(1 + (sample as i64)).unwrap(),
            epoch: tl3_store::Epoch::try_from(1).unwrap(),
            rows,
            pending_effects: vec![effect],
            credit: vec![],
            interned: vec![],
        })
    }
    fn commit(&mut self, c: &Commit) -> Result<()> {
        let rows: Vec<_> = c
            .rows
            .iter()
            .map(|r| match r {
                RowChange::Put { table, id, value } => json!({"put":[table,id,value]}),
                RowChange::Delete { table, id } => json!({"delete":[table,id]}),
            })
            .collect();
        let effects: Vec<_> = c
            .effects
            .iter()
            .map(|e| json!({"key":e.key,"kind":e.kind,"payload":e.payload}))
            .collect();
        let transitions: Vec<_> = c
            .transitions
            .iter()
            .map(|e| json!({"key":e.key,"from":e.from,"to":e.to}))
            .collect();
        let payload = json!({"event_key":c.event_key,"event":c.event,"expected_revision":c.expected_revision.get(),"epoch":c.epoch.get(),
            "rows":rows,"effects":effects,"transitions":transitions,"result":c.result,
            "credit":format!("{:?}",c.credit),"postmortems":format!("{:?}",c.postmortems),"grades":format!("{:?}",c.grades),
            "postmortem_updates":format!("{:?}",c.postmortem_updates),"next_epoch":c.next_epoch.map(|e|e.get())});
        Recorded::write(&self.io, "store.commit", payload.clone(), !self.active);
        if self.active {
            self.staged.push((
                format!("{}:store.commit", self.io.borrow().writes),
                serde_json::to_vec(&payload).unwrap(),
            ));
        }
        for r in &c.rows {
            match r {
                RowChange::Put { table, id, value } if table == input_errors::PENDING_KERNEL => {
                    self.pending.push(WorldRow {
                        table: table.clone(),
                        id: id.clone(),
                        value: value.clone(),
                        version: c.expected_revision,
                        arrival: self.pending.len() as i64,
                    })
                }
                RowChange::Delete { table, id } => {
                    self.pending.retain(|r| r.table != *table || r.id != *id)
                }
                _ => {}
            }
        }
        Ok(())
    }
    fn approval_dispatch_count(&mut self, key: &str) -> Result<i64> {
        Ok(Recorded::call(&self.io, "dispatch.count", json!(key)) as i64)
    }
}
enum Reconciliation {
    Found(Box<Completion>),
    Pending,
}
struct Adapters {
    io: Io,
}
impl Adapters {
    fn adapter(&mut self, _d: &Delivery) -> Result<&mut Self> {
        Ok(self)
    }
    fn retire_delivery(&mut self, d: &Delivery) -> Result<()> {
        Recorded::write(&self.io, "adapter.retire", json!(d), true);
        Ok(())
    }
    fn observations(&mut self, d: &Delivery) -> Result<Vec<Observation>> {
        let sample = Recorded::call(&self.io, "adapter.observations", json!(d));
        Ok(if sample & 1 == 0 {
            vec![]
        } else {
            vec![Observation::ScratchPruned {
                reference: "scratch".into(),
            }]
        })
    }
    fn repair_approval(&mut self, d: &Delivery) -> Result<(Reconciliation, Option<usize>)> {
        let sample = Recorded::call(&self.io, "adapter.repair", json!(d));
        Ok((
            if sample & 1 == 0 {
                Reconciliation::Found(Box::new(completion(0)))
            } else {
                Reconciliation::Pending
            },
            if sample & 2 == 0 {
                Some(sample as usize)
            } else {
                None
            },
        ))
    }
}
fn completion(sample: u8) -> Completion {
    Completion {
        ack: super::ack(sample).0,
        diagnostic: None,
        failure_class: if sample & 4 == 0 {
            None
        } else {
            Some("pr_policy".into())
        },
        progress: if sample & 8 == 0 {
            None
        } else {
            Some(json!("progress"))
        },
        observations: vec![
            Observation::ScratchSaved {
                node: NodeId(0),
                reference: "scratch".into(),
            },
            Observation::ScratchPruned {
                reference: "scratch".into(),
            },
        ],
    }
}
mod old {
    use super::*;
    include!("fixture.rs");
    include!("old-shell.rs");
}
mod current {
    use super::*;
    include!("fixture.rs");
    include!("current-shell.rs");
}

fn run(reference: bool, samples: Vec<u8>, crash: usize, scenario: u8) -> (Recorded, String) {
    let io = Rc::new(RefCell::new(Recorded {
        calls: vec![],
        durable: BTreeMap::new(),
        writes: 0,
        reads: 0,
        crash,
        samples,
    }));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if reference {
            old::run(io.clone(), scenario)
        } else {
            current::run(io.clone(), scenario)
        }
    }));
    let outcome = match result {
        Ok(Ok(())) => "ok".into(),
        Ok(Err(Error::Invalid(e))) => e.into(),
        Err(_) => "crash".into(),
    };
    let recorded = io.borrow().clone();
    (recorded, outcome)
}
#[test]
fn full_shell_differential_varying_reads_and_every_write_cut() {
    let output = std::path::PathBuf::from(
        std::env::var_os("TEST_UNDECLARED_OUTPUTS_DIR")
            .unwrap_or_else(|| std::env::temp_dir().into_os_string()),
    )
    .join("full-shell-regressions.txt");
    std::fs::write(&output, include_bytes!("full-shell-regressions.txt")).unwrap();
    let path = Box::leak(output.to_string_lossy().into_owned().into_boxed_str());
    let mut runner = TestRunner::new(Config {
        cases: 96,
        rng_seed: RngSeed::Fixed(73007),
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(path))),
        ..Config::default()
    });
    runner
        .run(
            &(proptest::collection::vec(any::<u8>(), 1..10), 0u8..6),
            |(samples, scenario)| {
                let uninterrupted = run(true, samples.clone(), 0, scenario);
                for cut in 0..=uninterrupted.0.writes {
                    prop_assert_eq!(
                        run(true, samples.clone(), cut, scenario),
                        run(false, samples.clone(), cut, scenario)
                    );
                }
                Ok(())
            },
        )
        .unwrap();
}
#[test]
fn compiled_shells_and_boundary_conversions_are_verbatim() {
    let current = include_str!("../../src/lib.rs");
    let start = current.find("    fn retire_effect(").unwrap();
    let end = current[start..].find("    fn apply(").unwrap() + start;
    assert!(include_str!("current-shell.rs").contains(&current[start..end]));
    let base = include_str!("base-owned-shell.rs.txt");
    let end = base.find("    fn apply(").unwrap();
    assert!(include_str!("old-shell.rs").contains(&base[..end]));
    let production = include_str!("../../src/executor.rs");
    assert!(production.contains(include_str!("conversions.rs")));
}

/// R4: a later missing-key read cannot undo the already durable enqueue.
#[test]
fn queued_input_survives_a_later_missing_pending_key() {
    let (recorded, _) = run(false, vec![0, 0, 0, 1], 0, 0);
    assert!(
        recorded
            .durable
            .values()
            .any(|bytes| String::from_utf8_lossy(bytes).contains("queue:completion:original:"))
    );
}
