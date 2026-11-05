#![allow(dead_code, unused_imports)] // Verbatim shells retain their original API.
use executor::{Completion, Delivery, Look, Observation, Output as Out, Upstream, upstream};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, time::Duration};
use tl3_controller::{Error, Repository};
use tl3_controller_core::looks as decisions;
use tl3_kernel::{Ack, Event, Fact, NodeId, Phase, World};
use tl3_store::{Commit, Effect, EffectState, RowChange, Snapshot};
type Result<T> = std::result::Result<T, Error>;
mod executor {
    pub use tl3_controller::executor::*;
}
mod adapters {
    pub mod mergify {
        pub const REQUEUE_PROGRESS_PREFIX: &str = "mergify:awaiting-requeue:";
    }
    pub mod prismabuild {
        pub const HARNESS: &str = "harness";
    }
}
mod approval_delivery {
    pub const AMBIGUOUS: &str = "approval_delivery_ambiguous";
}
mod dispatch_liveness {
    pub const MISSING: &str = "worker_dispatch_missing";
}
mod input_errors {
    use super::*;
    pub type InputFailure = Value;
    pub const COUNTER_HEADROOM: &str = "kernel_counter_headroom";
    pub fn recoverable(error: &Error) -> bool {
        !matches!(
            error,
            Error::Store(_) | Error::Config(_) | Error::StoredJson(_)
        )
    }
}
mod worker_inputs {
    use super::*;
    pub type Context = Value;
    pub const CLASS: &str = "worker_input_schema";
    pub fn of(_: &Error) -> Option<&Context> {
        None
    }
    pub fn append(_: &mut String, _: Option<&Context>) {}
}
mod publication_failure {
    use super::*;
    #[derive(Serialize)]
    pub struct Diagnostic(pub String);
    impl Diagnostic {
        pub fn render(&self) -> String {
            self.0.clone()
        }
    }
    pub fn retained(_: &Snapshot, _: &Commit, key: &str) -> Result<Option<Diagnostic>> {
        let word = io::read("publication.retained", json!(key))?;
        Ok((word & 1 != 0).then(|| Diagnostic("recorded refusal".into())))
    }
}
mod postmortems {
    use super::*;
    pub const PAUSED: &str = "postmortems_paused";
    pub fn stalled(mut c: Completion) -> Completion {
        for o in &mut c.observations {
            if let Observation::Postmortem {
                reply: None,
                failure,
                judge_failure,
                ..
            } = o
            {
                *failure = Some("no_progress".into());
                *judge_failure = None;
            }
        }
        c
    }
}
mod snapshot {
    use super::*;
    pub fn decode(_: &Snapshot) -> Result<World> {
        Ok(World::new(&[1], 1))
    }
}
fn ended_effect_look(_: &Snapshot, _: &World, _: &Delivery) -> bool {
    false
}
fn decode_stored<T: serde::de::DeserializeOwned>(v: Value) -> Result<T> {
    serde_json::from_value(v).map_err(Error::StoredJson)
}
fn put(table: &str, id: String, value: Value) -> RowChange {
    RowChange::Put {
        table: table.into(),
        id,
        value,
    }
}
struct Request {
    repo: Repository,
    issue: u64,
    title: String,
}
fn request(_: &Snapshot, _: NodeId) -> Result<Request> {
    Ok(Request {
        repo: repository(),
        issue: 1,
        title: "Request".into(),
    })
}
trait LocalDelivery {
    fn into_effect(self) -> tl3_store::NewEffect;
}
impl LocalDelivery for Delivery {
    fn into_effect(self) -> tl3_store::NewEffect {
        tl3_store::NewEffect {
            key: self.key.clone(),
            kind: upstream(self.output).kind().into(),
            payload: json!(self),
        }
    }
}
trait LocalError {
    fn is_github_not_found(&self) -> bool;
}
impl LocalError for Error {
    fn is_github_not_found(&self) -> bool {
        matches!(self, Error::Adapter(e) if matches!(e.downcast_ref::<tl3_github::Error>(), Some(tl3_github::Error::Http { status: 404 })))
    }
}
trait LocalCompletion {
    fn releases_holds(&self) -> bool;
}
impl LocalCompletion for Completion {
    fn releases_holds(&self) -> bool {
        if self.progress.is_some() && self.observations.is_empty() {
            return false;
        }
        !self.observations.iter().any(|o| {
            matches!(
                o,
                Observation::Progress { .. }
                    | Observation::Quota { .. }
                    | Observation::Kernel(Event::Fact {
                        fact: Fact::UpstreamStalled,
                        ..
                    })
            )
        })
    }
}
const LADDER_OUT_MS: u64 = 30 * 60 * 1000;
const CHECKS_PENDING: &str = "checks_pending";
include!("looks_io/types.rs");
#[path = "looks_io/io.rs"]
mod io;

macro_rules! controller {
    () => {
        struct Controller {
            store: io::Store,
            adapters: io::Adapters,
            bootstrap: tl3_controller::Bootstrap,
            allowances: tl3_config::Allowances,
        }
        impl Controller {
            fn new() -> Self {
                let mut allowances = tl3_config::Allowances::default();
                for a in [
                    &mut allowances.agent_worker,
                    &mut allowances.github_mergify,
                    &mut allowances.prismabuild,
                    &mut allowances.worktrees,
                ] {
                    a.silence_secs = 1;
                    a.no_progress_secs = 3;
                }
                Self {
                    store: io::Store,
                    adapters: io::Adapters,
                    bootstrap: tl3_controller::Bootstrap {
                        repos: vec![],
                        rob: 42,
                        slots: 1,
                        pool: 1,
                        fail_fast: false,
                        operations: None,
                    },
                    allowances,
                }
            }
            fn allowance(&self, d: &Delivery) -> &tl3_config::Allowance {
                match upstream(d.output) {
                    Upstream::Workers => &self.allowances.agent_worker,
                    Upstream::PrismaBuild => &self.allowances.prismabuild,
                    Upstream::GitHub | Upstream::Mergify => &self.allowances.github_mergify,
                    Upstream::Worktrees => &self.allowances.worktrees,
                }
            }
            fn now_ms(&self) -> u64 {
                io::now()
            }
            fn retry_held_effects(&self, u: Upstream) {
                io::record("retry_held", json!(u));
            }
            fn record_publication_refusal(&self, d: &Delivery, e: &Error) -> Result<()> {
                io::write("publication.refusal", json!([d.key, e.to_string()]), |r| {
                    r.files.insert("refusal".into(), e.to_string().into_bytes());
                })
            }
            fn observe(&self, key: &str, o: Observation) -> Result<()> {
                io::write("observe", json!([key, o]), |_| {})
            }
            fn retire_wait(&self, _: &Snapshot, d: &Delivery, c: &mut Commit) -> Result<()> {
                io::record("retire_wait", json!(d.key));
                c.rows.push(RowChange::Delete { table: "waits".into(), id: d.key.clone() });
                Ok(())
            }
            fn queue_observation(
                &self,
                _: &Snapshot,
                c: &mut Commit,
                key: &str,
                index: usize,
                o: &Observation,
            ) -> Result<()> {
                io::record("queue_observation", json!([key, index, o]));
                c.rows
                    .push(put("observations", format!("{key}:{index}"), json!(o)));
                Ok(())
            }
            fn retry_notice(&self, _: &Snapshot, i: &Incident, _: &mut Commit) -> Result<()> {
                io::record("retry_notice", json!(i.id));
                Ok(())
            }
            fn world(&self) -> Result<World> {
                io::read("world", Value::Null)?;
                Ok(World::new(&[1], 1))
            }
            fn complete(&self, key: &str, c: Completion) -> Result<()> {
                io::write("complete", json!([key, c]), |_| {})
            }
            fn expire_model_quotas(&self, now: u64) -> Result<()> {
                io::record("expire_quotas", json!(now));
                Ok(())
            }
            fn ladder_out_note(&self, s: &AllowanceState, resume: u64, now: u64) -> Result<()> {
                io::write("ladder_note", json!([s, resume, now]), |_| {})
            }
            fn remove_hold(&self, d: &Delivery) {
                io::record("remove_hold", json!(d.key));
            }
            fn hold_class(&self, _: &Delivery) -> Option<&str> {
                io::checks().then_some(CHECKS_PENDING)
            }
            fn is_held(&self, _: &Delivery) -> bool {
                io::held()
            }
        }
        pub(super) fn run(
            entry: io::Entry,
            state: &mut AllowanceState,
            s: &Snapshot,
        ) -> Result<()> {
            let mut c = Controller::new();
            match entry {
                io::Entry::Failed(code) => c.look_failed(state, io::error(code)),
                io::Entry::State => c.look_state(s, state, io::now()),
                io::Entry::Alive(changed) => c.look_alive(state, changed).map(|_| ()),
                io::Entry::Probe => c.probe_passed(Some(state), state.incident.as_deref()),
                io::Entry::Stall => c.finish_stall(state),
                io::Entry::Progress(code) => {
                    let mut commit = Commit::new("progress", json!({"entry":"progress"}), s);
                    c.progress_allowance(s, &state.delivery.key, format!("checkpoint-{code}"), &mut commit)?;
                    c.store.commit(&commit)
                }
                io::Entry::Quota(code) => {
                    let mut commit = Commit::new("quota", json!({"entry":"quota"}), s);
                    c.quota_allowance(s, &state.delivery.key, 2_000_000 + u64::from(code / 2), (code & 1 != 0).then_some("model-a"), &mut commit)?;
                    c.store.commit(&commit)
                }
            }
        }
    };
}
fn observation_upstream(_: &Snapshot, _: &Observation) -> Result<Option<Upstream>> {
    Ok(None)
}
#[allow(clippy::needless_question_mark)]
mod reference {
    include!("looks_io/reference.rs");
}
#[allow(clippy::needless_question_mark)]
mod current {
    include!("looks_io/current.rs");
}
fn repository() -> Repository {
    Repository {
        owner: "owner".into(),
        name: "sandbox".into(),
        installation: 3,
    }
}
fn delivery(key: &str) -> Delivery {
    serde_json::from_value(json!({"key":key,"epoch":1,"output":{"Effect":{"node":0,"seq":0,"kind":"Enqueue","head":null}},"generation":0,"head_sha":"head","repository":repository(),"issue":1,"pull_request":100,"paths":[],"attempt":0,"round":0})).unwrap()
}
#[path = "looks_io/properties.rs"]
mod properties;

trait LocalQueueFact {
    fn setup(&self) -> bool;
    fn recorded(&self, s: &Snapshot) -> Result<bool>;
}
impl LocalQueueFact for tl3_controller::adapters::mergify::QueueFact {
    fn setup(&self) -> bool {
        false
    }
    fn recorded(&self, _: &Snapshot) -> Result<bool> {
        Ok(false)
    }
}
#[path = "looks_io/d2.rs"]
mod d2;
