//! #830: SHA-pinned direct delivery behind the kernel's existing Enqueue effect.
pub mod decide;
mod events;
pub use events::Translator;

use super::record::Records;
use crate::outcome::Outcome as DeliveryOutcome;
use crate::{
    Error, Repository, Result,
    executor::{Completion, Delivery, Executor, Look, Observation, Output, Reconciliation},
};
use decide::{Checks, Facts, Head, Outcome, Plan, Step};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tl3_github::checks::{self, required};
use tl3_github::{GitHub, MergeMethod, PullRequest};
use tl3_kernel::{EffectKind, Phase};
use tl3_store::{Snapshot, Store};

fn head_failure(
    head: &str,
    infra: bool,
    check: u64,
    completed_at: Option<String>,
) -> (DeliveryOutcome, String) {
    let cause = crate::findings::CauseEvidence::Change {
        fingerprint: format!(
            "head:{head}:{}",
            completed_at.as_deref().unwrap_or("unknown")
        ),
    };
    let suffix = format!(
        "head-ci:{check}{}",
        completed_at
            .map(|time| format!(":{time}"))
            .unwrap_or_default()
    );
    (
        DeliveryOutcome::CiFailed {
            head: head.to_owned(),
            infra,
            failed_tests: vec![],
            evidence: crate::findings::CiEvidence {
                cause: Some(Box::new(cause)),
                ..Default::default()
            },
        },
        suffix,
    )
}

pub const VISIBILITY: &str = "delivered directly";

#[derive(Serialize, Deserialize)]
struct Intent {
    head: String,
    attempt: u64,
    action: String,
    method: MergeMethod,
    base_before_merge: String,
    consumed_through: Option<u64>,
}
/// A fence refusal. Only `Unready` leaves a never-accepted handoff unexplained.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Refusal {
    /// The node is paused, waits on an incident, or has another effect's open incident.
    Held,
    /// The stored node or handoff is not deliverable as recorded.
    Unready,
    /// Only this handoff's own open incident refuses; it is not a deliberate hold.
    Own,
}
#[derive(Serialize, Deserialize)]
struct Accepted {
    key: String,
    head: String,
    generation: u64,
    waiting_through: Option<u64>,
}
#[derive(Serialize, Deserialize)]
struct Merged {
    head: String,
    checks_passed: bool,
    evidence: Box<crate::outcome::MergeEvidence>,
}
struct Read {
    pull: PullRequest,
    facts: Facts,
    failure: Option<(DeliveryOutcome, String)>,
    method: Option<MergeMethod>,
    required_count: usize,
}

/// Sleeping effects are invalidated only by relevant webhook/catch-up events.
/// Recovery reads may observe outcomes, but cannot reuse an earlier authorization.
#[derive(Clone, Default)]
pub struct Wakes(Arc<Mutex<WakeState>>);
#[derive(Default)]
struct WakeState {
    sleeping: BTreeSet<String>,
    events: BTreeMap<String, u64>,
    journal: Option<Journal>,
}
impl Wakes {
    fn sleeping(&self, key: &str) -> bool {
        self.0.lock().unwrap().sleeping.contains(key)
    }
    fn sleep(&self, key: &str) {
        let mut state = self.0.lock().unwrap();
        state.sleeping.insert(key.to_owned());
        // Events preceding this attempt cannot authorize a later retry.
        state.events.remove(key);
    }
    fn wake(&self, key: &str, sequence: u64) {
        let mut state = self.0.lock().unwrap();
        state.sleeping.remove(key);
        state.events.insert(key.to_owned(), sequence);
    }
    fn event(&self, key: &str) -> Option<u64> {
        self.0.lock().unwrap().events.get(key).copied()
    }
    fn journal(&self) -> Option<Journal> {
        self.0.lock().unwrap().journal.clone()
    }
    fn bind(&self, journal: Journal) {
        self.0.lock().unwrap().journal = Some(journal);
    }
    fn forget(&self, key: &str) {
        let mut state = self.0.lock().unwrap();
        state.sleeping.remove(key);
        state.events.remove(key);
    }
}

/// Durable and upstream write boundaries exposed for process-crash verification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Write {
    Acceptance,
    Fence,
    Intent,
    Annotation,
    Merge,
    Receipt,
    History,
    Reclaim,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WriteBoundary {
    pub write: Write,
    pub after: bool,
}

#[derive(Clone)]
struct Journal {
    epoch: i64,
    records_root: PathBuf,
    store: tl3_config::Store,
}
impl Journal {
    fn receipt_path(&self, d: &Delivery) -> Result<PathBuf> {
        let identity = format!(
            "{}:{}/{}#{}",
            d.repository.installation,
            d.repository.owner.to_ascii_lowercase(),
            d.repository.name.to_ascii_lowercase(),
            number(d)?
        );
        let key = format!("direct:{:x}", Sha256::digest(identity.as_bytes()));
        Ok(Records::new(&self.records_root)
            .path(&key)
            .with_extension("merged"))
    }
    fn remember_merged(
        &self,
        d: &Delivery,
        merged: &Merged,
        boundary: impl Fn(Write, bool),
    ) -> Result<()> {
        self.remember_delivery(d, merged, |after| boundary(Write::History, after))?;
        let path = self.receipt_path(d)?;
        let bytes = serde_json::to_vec(merged)?;
        let records = Records::new(&self.records_root);
        records.fence(self.epoch)?;
        boundary(Write::Receipt, false);
        records.write(&path, &bytes)?;
        boundary(Write::Receipt, true);
        Ok(())
    }
    fn remember_delivery(
        &self,
        d: &Delivery,
        merged: &Merged,
        boundary: impl Fn(bool),
    ) -> Result<()> {
        let records = Records::new(&self.records_root);
        // Intent metadata may be unreadable; that must not suppress the merged receipt.
        let intent: Option<Intent> = match read_file(&records.path(&d.key).with_extension("intent")) {
            Ok(intent) => intent,
            Err(_) => return Ok(()),
        };
        if !intent.is_some_and(|intent| intent.action == "Merge" && intent.head == merged.head) {
            return Ok(());
        }
        let mut store = Store::open(&self.store)?;
        let snapshot = store.load()?;
        let id = d.node().0.to_string();
        let value = serde_json::json!({"delivery":d.key,"head":merged.head,"mode":"direct","number":number(d)?});
        if snapshot
            .rows
            .iter()
            .any(|r| r.table == "delivery_history" && r.id == id && r.value == value)
        {
            return Ok(());
        }
        if snapshot.epoch.get() != self.epoch || d.epoch > self.epoch {
            return Err(Error::Invalid("direct history epoch is fenced"));
        }
        records.fence(self.epoch)?;
        let mut commit = tl3_store::Commit::new(
            format!("direct-history:{}:{}", d.key, snapshot.revision.get()),
            serde_json::json!({"direct_delivery":d.key}),
            &snapshot,
        );
        commit.rows.push(tl3_store::RowChange::Put {
            table: "delivery_history".into(),
            id,
            value,
        });
        boundary(false);
        store.commit(&commit)?;
        boundary(true);
        Ok(())
    }
}

// The ingress writer serializes sequence allocation/commit. Include consumer
// positions because fully consumed rows may already have been reclaimed.
fn ingress_highwater(settings: &tl3_config::Store) -> Result<u64> {
    let mut reader = tl3_store::Reader::open(settings)?;
    let mut tx = reader.transaction()?;
    let row = tx.query_one("SELECT GREATEST(COALESCE((SELECT MAX(sequence) FROM tl3_ingress), 0), COALESCE((SELECT MAX(sequence) FROM tl3_ingress_consumers), 0))", &[])
        .map_err(tl3_store::Error::from)?;
    let sequence: i64 = row.get(0);
    u64::try_from(sequence).map_err(|_| Error::Invalid("negative direct ingress sequence"))
}
impl Journal {
    fn event_sequence(&self, delivery: &crate::ingress::GitHubDelivery) -> Result<Option<u64>> {
        let mut reader = tl3_store::Reader::open(&self.store)?;
        let mut tx = reader.transaction()?;
        let row = match delivery {
            crate::ingress::GitHubDelivery::Webhook(webhook) => tx.query_opt(
                "SELECT sequence FROM tl3_ingress WHERE source = 'github-webhook' AND delivery_key = $1", &[&webhook.id]),
            crate::ingress::GitHubDelivery::CatchUp { .. } => tx.query_opt(
                "SELECT sequence FROM tl3_ingress WHERE event_name = 'catch-up' AND payload = $1 ORDER BY sequence DESC LIMIT 1", &[&serde_json::to_vec(delivery)?]),
        }.map_err(tl3_store::Error::from)?;
        row.map(|row| {
            u64::try_from(row.get::<_, i64>(0))
                .map_err(|_| Error::Invalid("negative direct event sequence"))
        })
        .transpose()
    }
}

pub struct Adapter {
    recovery_through: u64,
    records_root: PathBuf,
    boundary: Option<Arc<dyn Fn(WriteBoundary) + Send + Sync>>,
    wakes: Wakes,
    clients: BTreeMap<u64, GitHub>,
    epoch: i64,
    records: Records,
    store: tl3_config::Store,
    runtime: &'static tokio::runtime::Runtime,
}
impl Adapter {
    pub fn new(
        clients: BTreeMap<u64, GitHub>,
        epoch: i64,
        records: &Path,
        store: tl3_config::Store,
    ) -> Result<Self> {
        fs::create_dir_all(records)?;
        let recovery_through = ingress_highwater(&store)?;
        let wakes = Wakes::default();
        wakes.bind(Journal {
            epoch,
            records_root: records.to_owned(),
            store: store.clone(),
        });
        Ok(Self {
            recovery_through,
            records_root: records.to_owned(),
            boundary: None,
            wakes,
            clients,
            epoch,
            records: Records::new(records),
            store,
            runtime: crate::executor::runtime()?,
        })
    }
    pub fn with_wakes(mut self, wakes: Wakes) -> Self {
        wakes.bind(self.journal());
        self.wakes = wakes;
        self
    }
    pub fn with_write_boundary(
        mut self,
        boundary: impl Fn(WriteBoundary) + Send + Sync + 'static,
    ) -> Self {
        self.boundary = Some(Arc::new(boundary));
        self
    }
    fn boundary(&self, write: Write, after: bool) {
        if let Some(hook) = &self.boundary {
            hook(WriteBoundary { write, after });
        }
    }
    fn write(&self, kind: Write, path: &Path, bytes: &[u8]) -> Result<()> {
        self.boundary(kind, false);
        self.records.write(path, bytes)?;
        self.boundary(kind, true);
        Ok(())
    }
    fn client(&self, d: &Delivery) -> Result<&GitHub> {
        self.clients
            .get(&d.repository.installation)
            .ok_or(Error::Invalid(
                "missing direct delivery installation client",
            ))
    }
    fn receipt_path(&self, d: &Delivery) -> Result<PathBuf> {
        self.journal().receipt_path(d)
    }
    fn recorded(&self, d: &Delivery) -> Result<Option<Merged>> {
        read_file(&self.receipt_path(d)?)
    }
    fn snapshot(&self) -> Result<Snapshot> {
        Ok(Store::open(&self.store)?.load()?)
    }
    /// Why the stored world refuses this delivery now, if it does.
    fn refusal(&self, d: &Delivery) -> Result<Option<Refusal>> {
        let snapshot = self.snapshot()?;
        if d.epoch < self.epoch || snapshot.epoch.get() != d.epoch {
            return Err(Error::Invalid("direct delivery epoch is fenced"));
        }
        let world = crate::snapshot::decode(&snapshot)?;
        let n = world
            .nodes
            .get(d.node().0 as usize)
            .ok_or(Error::Invalid("missing direct delivery node"))?;
        let (mut own, mut foreign) = (false, false);
        for row in &snapshot.rows {
            if row.table == "incidents"
                && row.value["open"] == true
                && row.value["node"].as_u64() == Some(d.node().0)
            {
                if row.value["effect"] == d.key.as_str() {
                    own = true;
                } else {
                    foreign = true;
                }
            }
        }
        Ok(if n.paused || n.waiting_incident || foreign {
            Some(Refusal::Held)
        } else if n.head != n.approved
            || crate::commit_sha(&snapshot, n.approved) != d.head_sha.as_deref()
            || n.checked.is_none()
            || n.checked != n.approved
            || !delivery_authority(n, d)
        {
            Some(Refusal::Unready)
        } else if own {
            Some(Refusal::Own)
        } else {
            None
        })
    }
    fn fence(&mut self, d: &Delivery) -> Result<()> {
        if self.refusal(d)?.is_some() {
            return Err(Error::ChecksPending);
        }
        self.boundary(Write::Fence, false);
        self.records.fence(d.epoch)?;
        self.boundary(Write::Fence, true);
        self.epoch = d.epoch;
        Ok(())
    }
    fn accept(&mut self, d: &Delivery) -> Result<()> {
        number(d)?;
        self.fence(d)?;
        let head = d
            .head_sha
            .as_deref()
            .ok_or(Error::Invalid("direct acceptance needs a head"))?;
        let path = self.records.path(&d.key).with_extension("accepted");
        if let Some(accepted) = read_file::<Accepted>(&path)? {
            if accepted.key != d.key || accepted.head != head || accepted.generation != d.generation
            {
                return Err(Error::Invalid("direct acceptance identity changed"));
            }
            return Ok(());
        }
        self.write(
            Write::Acceptance,
            &path,
            &serde_json::to_vec(&Accepted {
                key: d.key.clone(),
                head: head.into(),
                generation: d.generation,
                waiting_through: None,
            })?,
        )
    }
    fn wait(&self, d: &Delivery) -> Result<()> {
        let path = self.records.path(&d.key).with_extension("accepted");
        let mut accepted: Accepted =
            read_file(&path)?.ok_or(Error::Invalid("Direct wait has no durable acceptance"))?;
        accepted.waiting_through = Some(decide::consume(
            self.wakes.event(&d.key).unwrap_or(0),
            ingress_highwater(&self.store)?,
            accepted.waiting_through.unwrap_or(0),
        ));
        self.write(Write::Acceptance, &path, &serde_json::to_vec(&accepted)?)?;
        self.wakes.sleep(&d.key);
        Ok(())
    }
    fn read(&self, d: &Delivery, write_authorized: bool) -> Result<Read> {
        let approved = head(
            d.head_sha
                .as_deref()
                .ok_or(Error::Invalid("direct delivery requires an approved head"))?,
        )?;
        let github = self.client(d)?;
        let repo = full_name(&d.repository);
        let mut pull = self
            .runtime
            .block_on(github.pull_request(&repo, number(d)?))?;
        let mut failure = None;
        let mut checks = Checks::Pending;
        let mut approval = false;
        let mut method = None;
        let mut required_count = 0;
        if pull.merged_at.is_none() && head(&pull.head.sha)? == approved && pull.state == "open" {
            crate::evidence::validate_publication(
                d,
                &pull.head.branch,
                pull.draft,
                pull.body.as_deref().unwrap_or_default(),
            )?;
            let settings = self.runtime.block_on(github.merge_settings(&repo))?;
            method = settings.method();
            if method.is_none() {
                return Err(Error::Permanent {
                    class: "direct_merge_method",
                    source: "repository allows no merge method".into(),
                });
            }
            let read = read_checks(&self.runtime, github, &repo, &pull)?;
            checks = read.checks;
            approval = read.approval;
            failure = read.failure;
            required_count = read.count;
            let checked_base = pull.base.branch.clone();
            // A later read can differ. Never cache the first PR read across check/settings I/O.
            pull = self
                .runtime
                .block_on(github.pull_request(&repo, number(d)?))?;
            if pull.base.branch != checked_base {
                checks = Checks::Pending;
            }
        }
        let snapshot = self.snapshot()?;
        let world = crate::snapshot::decode(&snapshot)?;
        let n = world
            .nodes
            .get(d.node().0 as usize)
            .ok_or(Error::Invalid("missing direct delivery node"))?;
        let approved_id = n.approved;
        let approved_now = crate::commit_sha(&snapshot, approved_id) == d.head_sha.as_deref();
        let checked = crate::commit_sha(&snapshot, n.checked)
            .map(head)
            .transpose()?
            .unwrap_or(Head { upper: 0, lower: 0 });
        let held = n.paused
            || n.waiting_incident
            || !delivery_authority(n, d)
            || snapshot.rows.iter().any(|row| {
                row.table == "incidents"
                    && row.value["open"] == true
                    && row.value["node"].as_u64() == Some(d.node().0)
            });
        Ok(Read {
            facts: Facts {
                approved,
                write_authorized,
                observed: head(&pull.head.sha)?,
                checked,
                checking: n.checked.is_some(),
                approval: approval && approved_now && !pull.draft,
                open: pull.state == "open",
                merged: pull.merged_at.is_some(),
                recorded_merge: false,
                held,
                conflict: pull.mergeable == Some(false),
                checks,
                visible: pull
                    .body
                    .as_deref()
                    .unwrap_or_default()
                    .lines()
                    .any(|line| line == VISIBILITY),
            },
            pull,
            failure,
            method,
            required_count,
        })
    }
    fn fact(&self, d: &Delivery, outcome: DeliveryOutcome) -> Result<Completion> {
        let suffix = match &outcome {
            DeliveryOutcome::Merged { head, .. } => format!("merged:{head}"),
            DeliveryOutcome::PushByOther { head } => format!("push:{head}"),
            DeliveryOutcome::Conflict { .. } => {
                return Ok(Completion::delivery_accepted(vec![Observation::Outcome {
                    node: d.node(),
                    outcome,
                    receipt: None,
                }]));
            }
            DeliveryOutcome::ClosedByPerson => "closed".into(),
            DeliveryOutcome::CiFailed { .. } => {
                return Err(Error::Invalid("CI outcome needs a check receipt"));
            }
        };
        self.fact_with_receipt(d, outcome, &suffix)
    }
    fn fact_with_receipt(
        &self,
        d: &Delivery,
        outcome: DeliveryOutcome,
        suffix: &str,
    ) -> Result<Completion> {
        let identity = crate::outcome::identity(&d.repository, number(d)?, suffix);
        let observations = if crate::outcome::recorded(&self.snapshot()?, &identity) {
            vec![]
        } else {
            vec![Observation::Outcome {
                node: d.node(),
                outcome,
                receipt: Some(identity),
            }]
        };
        Ok(Completion::delivery_accepted(observations))
    }
    fn journal(&self) -> Journal {
        Journal {
            epoch: self.epoch,
            records_root: self.records_root.clone(),
            store: self.store.clone(),
        }
    }
    fn finish_merge(
        &self,
        d: &Delivery,
        pull: &PullRequest,
        head: String,
        checks_passed: bool,
        submitted: Option<SubmittedMerge>,
    ) -> Result<Completion> {
        let mut merged = Merged::pending(head, checks_passed);
        self.journal()
            .remember_merged(d, &merged, |write, after| self.boundary(write, after))?;
        let mut evidence = crate::outcome::MergeEvidence::empty(merged.head.clone());
        let intent = match read_file::<Intent>(&self.records.path(&d.key).with_extension("intent")) {
            Ok(intent) => {
                intent.filter(|intent| intent.action == "Merge" && intent.head == merged.head)
            }
            Err(error) => {
                evidence.error("merge_intent", evidence_error_class(&error));
                None
            }
        };
        let request = MergeRead::new(
            pull,
            submitted.as_ref(),
            intent.as_ref(),
            evidence.evidence_errors.is_empty(),
        );
        match (self.client(d), self.snapshot()) {
            (Ok(github), Ok(snapshot)) => collect_merge_evidence(
                self.runtime,
                github,
                &snapshot,
                d.node(),
                request,
                &mut evidence,
            ),
            (client, snapshot) => {
                if let Err(error) = client {
                    evidence.error("client", evidence_error_class(&error));
                }
                if let Err(error) = snapshot {
                    evidence.error("snapshot", evidence_error_class(&error));
                }
            }
        }
        merged.evidence = Box::new(evidence);
        self.merged(d, merged)
    }
    fn merged(&self, d: &Delivery, mut merged: Merged) -> Result<Completion> {
        if let Err(error) = self
            .journal()
            .remember_merged(d, &merged, |write, after| self.boundary(write, after))
        {
            merged
                .evidence
                .error("evidence_receipt", evidence_error_class(&error));
        }
        self.fact(
            d,
            DeliveryOutcome::Merged {
                head: merged.head,
                checks_passed: merged.checks_passed,
                evidence: Some(merged.evidence),
            },
        )
    }
    fn result(&self, d: &Delivery, read: &Read, plan: &Plan) -> Result<Option<Completion>> {
        Ok(Some(match plan.outcome {
            Outcome::Merged => {
                let matched = read.facts.observed == read.facts.approved
                    && read.facts.checking
                    && read.facts.checked == read.facts.approved;
                self.finish_merge(d, &read.pull, read.pull.head.sha.clone(), matched, None)?
            }
            Outcome::Moved => self.fact(
                d,
                DeliveryOutcome::PushByOther {
                    head: read.pull.head.sha.clone(),
                },
            )?,
            Outcome::Conflict => self.fact(
                d,
                DeliveryOutcome::Conflict {
                    head: read.pull.head.sha.clone(),
                },
            )?,
            Outcome::Failed => {
                let (outcome, receipt) = read
                    .failure
                    .clone()
                    .ok_or(Error::Invalid("direct check failure lacks evidence"))?;
                self.fact_with_receipt(d, outcome, &receipt)?
            }
            Outcome::Closed => self.fact(d, DeliveryOutcome::ClosedByPerson)?,
            Outcome::Wait => {
                self.wait(d)?;
                return Err(Error::ChecksPending);
            }
            Outcome::Held => return Err(Error::ChecksPending),
            Outcome::Ready | Outcome::Observe => return Ok(None),
        }))
    }
    /// `consume` is false for a look: it reports a permanent refusal without
    /// consuming the merge authorization, so the drive's reconcile raises it.
    fn attempt(&mut self, d: &Delivery, consume: bool) -> Result<Completion> {
        self.accept(d)?;
        match self.attempt_accepted(d) {
            Ok(mut completion) => {
                if completion.ack == tl3_kernel::Ack::Permanent && completion.diagnostic.is_none() {
                    completion.diagnostic = Some(crate::publication_failure::Diagnostic::capture(
                        d,
                        None,
                        completion
                            .failure_class
                            .as_deref()
                            .unwrap_or("direct_merge_refused"),
                    )?);
                }
                completion.ack = tl3_kernel::Ack::DeliveryAccepted;
                Ok(completion)
            }
            Err(Error::ChecksPending) => Ok(Completion::delivery_accepted(vec![])),
            Err(error) => self.accepted_refusal(d, error, consume),
        }
    }
    /// After durable acceptance, only a permanent refusal becomes an accepted
    /// diagnostic. A fatal stored-state error and every recoverable read or
    /// write failure return unchanged: Q7 read accounting counts them, and the
    /// merge authorization is not consumed.
    fn accepted_refusal(&self, d: &Delivery, error: Error, consume: bool) -> Result<Completion> {
        // Fatal classes are checked on their own, not only through the permanent filter.
        if !crate::input_errors::recoverable(&error) || !matches!(error, Error::Permanent { .. }) {
            return Err(error);
        }
        if consume {
            self.wait(d)?;
        }
        let class = crate::looks::failure_class(&error);
        let mut completion = Completion::delivery_accepted(vec![]);
        completion.failure_class = Some(class.into());
        completion.diagnostic = Some(crate::publication_failure::Diagnostic::capture(
            d,
            Some(&error),
            class,
        )?);
        Ok(completion)
    }
    /// State-table "Looks and stalls": a handoff Direct never durably accepted
    /// is an unacknowledged operation, so the look is pending and Q7 read
    /// accounting escalates it. A deliberate hold (pause, incident wait, another
    /// effect's open incident) already has an owner and a signal, so it stays a
    /// healthy wait, as does any refusal after acceptance. The handoff's own
    /// incident is not a hold: an unready handoff stays pending under it.
    fn refused_look(&self, d: &Delivery) -> Result<Look> {
        let path = self.records.path(&d.key).with_extension("accepted");
        if read_file::<Accepted>(&path)?.is_some() {
            return Ok(Look::Healthy);
        }
        Ok(match self.refusal(d)? {
            Some(Refusal::Unready) => Look::Pending,
            Some(Refusal::Held | Refusal::Own) | None => Look::Healthy,
        })
    }
    fn attempt_accepted(&mut self, d: &Delivery) -> Result<Completion> {
        number(d)?;
        if let Some(merged) = self.recorded(d)? {
            return self.merged(d, merged);
        }
        if self.wakes.sleeping(&d.key) {
            return Err(Error::ChecksPending);
        }
        let intent: Option<Intent> =
            read_file(&self.records.path(&d.key).with_extension("intent"))?;
        let event = self.wakes.event(&d.key).unwrap_or(0);
        let accepted: Accepted = read_file(&self.records.path(&d.key).with_extension("accepted"))?
            .ok_or(Error::Invalid("Direct attempt has no durable acceptance"))?;
        let attempted = accepted.waiting_through.is_some()
            || intent
                .as_ref()
                .is_some_and(|i| i.action == "Merge" || i.consumed_through.is_some());
        let consumed = intent
            .as_ref()
            .and_then(|i| i.consumed_through)
            .unwrap_or(0)
            .max(accepted.waiting_through.unwrap_or(0));
        let authorized = decide::authorized(attempted, event, consumed, self.recovery_through);
        // At most one annotation and one merge per call. A pending check never loops.
        for _ in 0..2 {
            let read = self.read(d, authorized)?;
            let plan = decide::decide(read.facts);
            if let Some(completion) = self.result(d, &read, &plan)? {
                return Ok(completion);
            }
            if plan.outcome == Outcome::Observe {
                self.wait(d)?;
                return Err(Error::ChecksPending);
            }
            let github = self.client(d)?.clone();
            let repo = full_name(&d.repository);
            let sha = format!("{:032x}{:08x}", plan.sha.upper, plan.sha.lower);
            let mut merge_base = None;
            for step in [plan.first, plan.second, plan.third] {
                match step {
                    Step::None => {}
                    Step::Fence => self.fence(d)?,
                    Step::Intent => {
                        let path = self.records.path(&d.key).with_extension("intent");
                        let previous: Option<Intent> = read_file(&path)?;
                        let consumed_through = if plan.third == Step::Merge {
                            Some(decide::consume(
                                event,
                                ingress_highwater(&self.store)?,
                                consumed,
                            ))
                        } else if attempted {
                            // Annotation preserves the consumed authorization fence.
                            Some(consumed)
                        } else {
                            None
                        };
                        let attempt = previous.map_or(Ok(1), |intent| {
                            intent
                                .attempt
                                .checked_add(1)
                                .ok_or(Error::Invalid("direct intent counter exhausted"))
                        })?;
                        let base_before_merge = if plan.third == Step::Merge {
                            live_merge_base(&self.runtime, &github, &repo, &read.pull)?
                        } else {
                            read.pull.base.sha.clone()
                        };
                        merge_base = Some(base_before_merge.clone());
                        self.write(
                            Write::Intent,
                            &path,
                            &serde_json::to_vec(&Intent {
                                head: sha.clone(),
                                attempt,
                                action: format!("{:?}", plan.third),
                                consumed_through,
                                base_before_merge,
                                method: read
                                    .method
                                    .ok_or(Error::Invalid("missing merge method"))?,
                            })?,
                        )?;
                    }
                    Step::Annotate => {
                        self.fence(d)?;
                        let body = format!(
                            "{}\n\n{VISIBILITY}\n",
                            read.pull.body.as_deref().unwrap_or_default().trim_end()
                        );
                        self.boundary(Write::Annotation, false);
                        self.runtime.block_on(github.update_pull_request_body(
                            &repo,
                            number(d)?,
                            &body,
                        ))?;
                        self.boundary(Write::Annotation, true);
                    }
                    Step::Merge => {
                        self.fence(d)?;
                        // No timer/repeated controller drive may authorize the next PUT.
                        // The intent also consumes all already-received events durably.
                        self.wakes.sleep(&d.key);
                        self.boundary(Write::Merge, false);
                        let result = self.runtime.block_on(github.merge_pull_request(
                            &repo,
                            number(d)?,
                            &sha,
                            read.method.ok_or(Error::Invalid("missing merge method"))?,
                        ));
                        self.boundary(Write::Merge, true);
                        match result {
                            Ok(result) if result.merged => {
                                return self.finish_merge(
                                    d,
                                    &read.pull,
                                    sha.clone(),
                                    true,
                                    Some(SubmittedMerge {
                                        commit: result.sha,
                                        base: merge_base.clone(),
                                        checks: crate::outcome::RequiredChecks {
                                            passed: true,
                                            count: read.required_count,
                                        },
                                        method: read.method,
                                    }),
                                );
                            }
                            Ok(_) | Err(tl3_github::Error::Http { status: 405 }) => {
                                // Already-merged refusals converge; classify a genuine refusal from fresh evidence.
                                let current = self.read(d, true)?;
                                if let Some(completion) =
                                    self.result(d, &current, &decide::decide(current.facts))?
                                {
                                    return Ok(completion);
                                }
                                return Ok(Completion::permanent("direct_merge_refused"));
                            }
                            Err(tl3_github::Error::Http { status: 409 }) => {
                                let current = self
                                    .runtime
                                    .block_on(github.pull_request(&repo, number(d)?))?;
                                if current.merged_at.is_some() {
                                    return self.finish_merge(
                                        d,
                                        &current,
                                        current.head.sha.clone(),
                                        current.head.sha == sha,
                                        None,
                                    );
                                }
                                return self.fact(
                                    d,
                                    DeliveryOutcome::PushByOther {
                                        head: current.head.sha,
                                    },
                                );
                            }
                            Err(error) => {
                                // A landed PUT with a lost reply must never cause another PUT.
                                let current = self
                                    .runtime
                                    .block_on(github.pull_request(&repo, number(d)?))?;
                                if current.merged_at.is_some() {
                                    return self.finish_merge(
                                        d,
                                        &current,
                                        current.head.sha.clone(),
                                        current.head.sha == sha,
                                        None,
                                    );
                                }
                                return Err(error.into());
                            }
                        }
                    }
                }
            }
        }
        Err(Error::ChecksPending)
    }
}
impl Executor for Adapter {
    fn execute(&mut self, d: &Delivery) -> Result<Completion> {
        self.attempt(d, true)
    }
    fn reconcile(&mut self, d: &Delivery) -> Result<Reconciliation> {
        // Recovery observes outcomes; a consumed attempt needs a new post-recovery event.
        match self.attempt(d, true) {
            Ok(completion) => Ok(Reconciliation::Found(Box::new(completion))),
            Err(Error::ChecksPending) => Ok(Reconciliation::Pending),
            Err(error) => Err(error),
        }
    }
    fn observations(&mut self, d: &Delivery) -> Result<Vec<Observation>> {
        if let Some(merged) = self.recorded(d)? {
            return Ok(self.merged(d, merged)?.observations);
        }
        if self.wakes.sleeping(&d.key) {
            return Ok(vec![]);
        }
        let read = self.read(d, false)?;
        let plan = decide::decide(read.facts);
        match self.result(d, &read, &plan) {
            Ok(Some(completion)) => Ok(completion.observations),
            Ok(None) | Err(Error::ChecksPending) => Ok(vec![]),
            Err(error) => Err(error),
        }
    }
    fn look(&mut self, d: &Delivery) -> Result<Look> {
        // Only the authenticated upstream translator grants this capability.
        // Silence/allowance looks without that capability are observation-only.
        if self.wakes.event(&d.key).is_some() {
            return match self.attempt(d, false) {
                Ok(completion) => Ok(Look::Found(Box::new(completion))),
                // `attempt` is pending only when the fence refused acceptance.
                Err(Error::ChecksPending) => self.refused_look(d),
                Err(error) => Err(error),
            };
        }
        match self.accept(d) {
            Err(Error::ChecksPending) => return self.refused_look(d),
            Err(error) => return Err(error),
            Ok(()) => {}
        }
        let completion = match self.observations(d) {
            Ok(observations) => Completion::delivery_accepted(observations),
            Err(error) => self.accepted_refusal(d, error, false)?,
        };
        Ok(Look::Found(Box::new(completion)))
    }
    fn reconcile_cancellation(&mut self, d: &Delivery) -> Result<Reconciliation> {
        // An authenticated merge receipt can arrive before the handoff is
        // acknowledged; like `attempt`, prefer it to a fresh GitHub read.
        if let Some(merged) = self.recorded(d)? {
            return Ok(Reconciliation::Found(Box::new(self.merged(d, merged)?)));
        }
        let github = self.client(d)?;
        let pull = self
            .runtime
            .block_on(github.pull_request(&full_name(&d.repository), number(d)?))?;
        let completion = if pull.merged_at.is_some() {
            self.finish_merge(
                d,
                &pull,
                pull.head.sha.clone(),
                Some(pull.head.sha.as_str()) == d.head_sha.as_deref(),
                None,
            )?
        } else {
            Completion::confirmed(vec![])
        };
        Ok(Reconciliation::Found(Box::new(completion)))
    }
    fn retire_delivery(&mut self, d: &Delivery) -> Result<()> {
        let snapshot = self.snapshot()?;
        // Retirement stores the original effect payload, before PR hydration.
        // Recover its binding from the same durable snapshot as terminal evidence.
        let mut hydrated = d.clone();
        hydrated.pull_request = hydrated.pull_request.or_else(|| {
            snapshot
                .rows
                .iter()
                .find(|r| r.table == "pull_requests" && r.id == d.node().0.to_string())
                .and_then(|r| r.value["number"].as_u64())
        });
        let d = &hydrated;
        let world = crate::snapshot::decode(&snapshot)?;
        let n = world
            .nodes
            .get(d.node().0 as usize)
            .ok_or(Error::Invalid("missing retired direct node"))?;
        // The controller invokes this hook after the terminal effect's dispatch/watch
        // retirement is durable. The consumed merged fact protects against late retries.
        let retiring = snapshot
            .rows
            .iter()
            .any(|r| r.table == "retiring_effects" && r.id == d.key);
        let unsettled = snapshot
            .pending_effects
            .iter()
            .any(|effect| effect.key == d.key)
            || snapshot
                .rows
                .iter()
                .any(|r| matches!(r.table.as_str(), "watches" | "dispatches") && r.id == d.key);
        if retiring
            && !unsettled
            && matches!(
                n.phase,
                Phase::Stopping(tl3_kernel::Outcome::Merged)
                    | Phase::Closed(tl3_kernel::Outcome::Merged)
            )
        {
            if snapshot.epoch.get() != self.epoch || d.epoch > self.epoch {
                return Err(Error::Invalid("direct retirement epoch is fenced"));
            }
            crate::commit_sha(&snapshot, n.merged)
                .ok_or(Error::Invalid("missing consumed merged head"))?;
            // History is saved before the merged fact, including webhook facts.
            // Retirement must not advance the caller's store revision.
            self.records.fence(self.epoch)?;
            for path in [
                self.records.path(&d.key).with_extension("intent"),
                self.records.path(&d.key).with_extension("accepted"),
                self.receipt_path(d)?,
            ] {
                self.boundary(Write::Reclaim, false);
                match fs::remove_file(&path) {
                    Ok(()) => std::fs::File::open(path.parent().unwrap())?.sync_all()?,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
                self.boundary(Write::Reclaim, true);
            }
        }
        self.wakes.forget(&d.key);
        Ok(())
    }
    fn probe(&mut self, d: &Delivery) -> Result<bool> {
        self.runtime.block_on(
            self.client(d)?
                .pull_request(&full_name(&d.repository), number(d)?),
        )?;
        // A readable PR does not repair a never-accepted handoff that no
        // deliberate hold explains; its incident stays open until it is accepted.
        Ok(!matches!(self.refused_look(d)?, Look::Pending))
    }
    fn app_login(&self, installation: u64) -> Option<String> {
        self.clients
            .get(&installation)
            .map(|client| format!("{}[bot]", client.app_slug()))
    }
}
fn delivery_authority(n: &tl3_kernel::Node, d: &Delivery) -> bool {
    match n.phase {
        Phase::Publishing => {
            n.generation == d.generation
                && matches!(
                    (n.effect, d.output),
                    (Some(effect), Output::Effect { seq, kind: EffectKind::Enqueue, .. })
                        if effect.seq == seq && effect.kind == EffectKind::Enqueue && !effect.cancelling
                )
        }
        Phase::AwaitingDelivery => d.generation.checked_add(1) == Some(n.generation),
        _ => false,
    }
}
fn number(d: &Delivery) -> Result<u64> {
    if !matches!(
        d.output,
        Output::Effect {
            kind: EffectKind::Enqueue,
            ..
        }
    ) {
        return Err(Error::Invalid("unsupported direct delivery effect"));
    }
    d.pull_request
        .ok_or(Error::Invalid("direct delivery needs a PR number"))
}
// Parse the upstream annotation wording at this boundary; C1 owns aggregation/classification.
fn annotation_class(annotation: &tl3_github::CheckAnnotation) -> required::Annotation {
    if annotation.annotation_level != "failure" {
        return required::Annotation::Ignore;
    }
    if [
        "The runner has lost communication with the server",
        "The job was not acquired by Runner of type",
        "The job was abandoned as the runner did not pick it up",
        "The job was not started because recent account payments have failed",
    ]
    .iter()
    .any(|prefix| annotation.message.starts_with(prefix))
    {
        required::Annotation::RunnerFailure
    } else {
        required::Annotation::OtherFailure
    }
}
fn full_name(repo: &Repository) -> String {
    format!("{}/{}", repo.owner, repo.name)
}
fn head(sha: &str) -> Result<Head> {
    if sha.len() != 40 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::Invalid("invalid direct delivery head"));
    }
    Ok(Head {
        upper: u128::from_str_radix(&sha[..32], 16)
            .map_err(|_| Error::Invalid("invalid direct delivery head"))?,
        lower: u32::from_str_radix(&sha[32..], 16)
            .map_err(|_| Error::Invalid("invalid direct delivery head"))?,
    })
}
fn read_file<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub struct Fake {
    records: Records,
    hold_merges: bool,
}
impl Fake {
    pub fn new(root: &Path) -> Self {
        Self {
            records: Records::new(root),
            hold_merges: root.join("hold-merges").exists(),
        }
    }
}
impl Executor for Fake {
    fn contract_evidence(
        &mut self,
        delivery: &Delivery,
    ) -> Result<crate::dispatch_liveness::decision::ContractEvidence> {
        self.records.contract_evidence(delivery)
    }

    fn execute(&mut self, delivery: &Delivery) -> Result<Completion> {
        let node = delivery.node();
        if delivery.pull_request.is_none() {
            return Err(crate::Error::Invalid("enqueue needs a PR number"));
        }
        let observations = match delivery.output {
            Output::Effect {
                kind: EffectKind::Enqueue,
                ..
            } => vec![Observation::Merged {
                node,
                sha: delivery
                    .head_sha
                    .clone()
                    .ok_or(crate::Error::Invalid("delivery needs a head"))?,
                checks_passed: true,
            }],
            _ => return Err(crate::Error::Invalid("unsupported delivery effect")),
        };
        self.records.deliver(
            delivery,
            Completion::delivery_accepted(if self.hold_merges {
                vec![]
            } else {
                observations
            }),
        )
    }
    fn reconcile(&mut self, delivery: &Delivery) -> Result<Reconciliation> {
        self.records.reconcile(delivery)
    }
}

struct CheckRead {
    checks: Checks,
    approval: bool,
    failure: Option<(DeliveryOutcome, String)>,
    count: usize,
}
fn read_checks(
    runtime: &tokio::runtime::Runtime,
    github: &GitHub,
    repo: &str,
    pull: &PullRequest,
) -> Result<CheckRead> {
    let mut failure = None;
    let required = runtime.block_on(github.required_status_checks(repo, &pull.base.branch))?;
    let count = required.checks.len();
    if !required.unavailable.is_empty() {
        return Err(Error::Permanent {
            class: "direct_protection_read",
            source: required.unavailable.join("; ").into(),
        });
    }
    let runs = runtime.block_on(github.check_runs(repo, &pull.head.sha))?;
    // #830 retains the own-app approval check; durable exact-head approval is checked below.
    let own: Vec<_> = runs
        .iter()
        .filter(|check| {
            check.head_sha == pull.head.sha
                && check.name == super::github::APPROVAL_CHECK
                && check
                    .app
                    .as_ref()
                    .is_some_and(|app| app.id == github.app_id())
        })
        .collect();
    let approval = !own.is_empty()
        && own.iter().all(|check| {
            check.status == "completed"
                && check.conclusion.as_deref() == Some("success")
                && check
                    .external_id
                    .as_deref()
                    .is_some_and(|id| id.ends_with(&format!(":{}", pull.head.sha)))
        });
    let mut latest = BTreeMap::new();
    for check in &runs {
        if check.head_sha == pull.head.sha {
            let entry = latest
                .entry((check.name.as_str(), check.app.as_ref().map(|app| app.id)))
                .or_insert(check);
            if check.id > entry.id {
                *entry = check;
            }
        }
    }
    let statuses = if required
        .checks
        .iter()
        .any(|rule| rule.integration_id.is_none())
    {
        runtime.block_on(github.commit_statuses(repo, &pull.head.sha))?
    } else {
        vec![]
    };
    let mut summary = required::Summary {
        ready: true,
        failure: required::Failure::None,
    };
    for rule in required.checks {
        let matching: Vec<_> = latest
            .values()
            .filter(|check| {
                required::rule_matches(
                    check.name == rule.context,
                    rule.integration_id,
                    check.app.as_ref().map(|app| app.id),
                )
            })
            .collect();
        let status = rule
            .integration_id
            .is_none()
            .then(|| {
                statuses
                    .iter()
                    .filter(|status| status.context == rule.context)
                    .max_by_key(|status| status.id)
            })
            .flatten();
        if matching.is_empty() && status.is_none() {
            summary = required::record(summary, required::Contribution::Pending);
        }
        // Q47, docs/state-machine.md:681-689 on b8ed2968; #830 preserves required protection.
        // C1's record folds EVERY report conjunctively, including a same-name legacy status.
        for check in matching {
            let view = checks::views(std::slice::from_ref(*check), &pull.head.sha, |_| true)[0];
            let runner = if required::needs_annotations(
                view.status == checks::Status::Completed,
                view.app == checks::App::Actions,
                view.conclusion == Some(checks::Conclusion::Failure),
            ) {
                let annotations = runtime.block_on(github.check_annotations(repo, check.id))?;
                required::runner_failure(
                    &annotations.iter().map(annotation_class).collect::<Vec<_>>(),
                )
            } else {
                false
            };
            let item = match required::required_check(view, runner) {
                required::RequiredState::Passed => required::Contribution::Passed,
                required::RequiredState::Infrastructure => required::Contribution::Infrastructure,
                required::RequiredState::Tests => required::Contribution::Tests,
                required::RequiredState::Pending | required::RequiredState::Unknown => {
                    required::Contribution::Pending
                }
            };
            summary = required::record(summary, item);
            if matches!(
                item,
                required::Contribution::Tests | required::Contribution::Infrastructure
            ) && (failure.is_none() || item == required::Contribution::Tests)
            {
                failure = Some(head_failure(
                    &pull.head.sha,
                    item == required::Contribution::Infrastructure,
                    check.id,
                    check.completed_at.map(|time| time.to_rfc3339()),
                ));
            }
        }
        if let Some(status) = status {
            let item = required::classic_status(match status.state.as_str() {
                "success" => required::ClassicStatus::Success,
                "failure" => required::ClassicStatus::Failure,
                "error" => required::ClassicStatus::Error,
                _ => required::ClassicStatus::Other,
            });
            summary = required::record(summary, item);
            if item == required::Contribution::Tests {
                failure = Some(head_failure(
                    &pull.head.sha,
                    false,
                    status.id,
                    status.updated_at.map(|time| time.to_rfc3339()),
                ));
            }
        }
    }
    let checks = if summary.failure != required::Failure::None {
        Checks::Failed
    } else if summary.ready {
        Checks::Passed
    } else {
        Checks::Pending
    };
    Ok(CheckRead {
        checks,
        approval,
        failure,
        count,
    })
}
fn live_merge_base(
    runtime: &tokio::runtime::Runtime,
    github: &GitHub,
    repo: &str,
    pull: &PullRequest,
) -> Result<String> {
    let branch = runtime
        .block_on(github.branch(repo, &pull.base.branch))?
        .ok_or(tl3_github::Error::Http { status: 404 })?;
    head(&branch.commit.sha)?;
    Ok(branch.commit.sha)
}
fn passing_check_base<'a>(
    snapshot: &'a Snapshot,
    node: tl3_kernel::NodeId,
    approved: &str,
) -> Option<&'a str> {
    snapshot
        .rows
        .iter()
        .find(|row| {
            row.table == "check_reports"
                && row.id == format!("{}:{approved}", node.0)
                && row.value["head_sha"].as_str() == Some(approved)
                && row.value["pass"] == true
        })
        .and_then(|row| row.value["base_sha"].as_str())
        .filter(|base| head(base).is_ok())
}
struct SubmittedMerge {
    commit: Option<String>,
    base: Option<String>,
    checks: crate::outcome::RequiredChecks,
    method: Option<MergeMethod>,
}
struct MergeRead<'a> {
    pull: &'a PullRequest,
    commit: Option<&'a str>,
    base: Option<&'a str>,
    checks: Option<&'a crate::outcome::RequiredChecks>,
    method: Option<MergeMethod>,
    observed: bool,
    outside: bool,
}
impl<'a> MergeRead<'a> {
    fn new(
        pull: &'a PullRequest,
        submitted: Option<&'a SubmittedMerge>,
        intent: Option<&'a Intent>,
        intent_known: bool,
    ) -> Self {
        Self {
            pull,
            commit: submitted.map_or(pull.merge_commit_sha.as_deref(), |value| {
                value.commit.as_deref()
            }),
            base: submitted
                .and_then(|value| value.base.as_deref())
                .or_else(|| intent.map(|value| value.base_before_merge.as_str())),
            checks: submitted.map(|value| &value.checks),
            method: submitted
                .and_then(|value| value.method)
                .or_else(|| intent.map(|value| value.method)),
            observed: submitted.is_none(),
            outside: submitted.is_none() && intent.is_none() && intent_known,
        }
    }
}
impl Merged {
    fn pending(head: String, checks_passed: bool) -> Self {
        let mut evidence = crate::outcome::MergeEvidence::empty(head.clone());
        evidence.error("collection", "interrupted");
        Self {
            head,
            checks_passed,
            evidence: Box::new(evidence),
        }
    }
}
fn evidence_error_class(error: &Error) -> String {
    match error {
        Error::Permanent {
            class: "direct_protection_read",
            ..
        } => "direct_protection_read".into(),
        Error::Permanent { source, class } => source
            .downcast_ref::<tl3_github::Error>()
            .map(github_evidence_class)
            .unwrap_or_else(|| (*class).into()),
        Error::Adapter(source) => source
            .downcast_ref::<tl3_github::Error>()
            .map(github_evidence_class)
            .unwrap_or_else(|| "adapter".into()),
        Error::Quota { .. } => "quota".into(),
        Error::Io(_) => "io".into(),
        Error::Json(_) | Error::StoredJson(_) => "json".into(),
        Error::Store(_) => "store".into(),
        Error::Invalid(_) => "invalid".into(),
        _ => "controller".into(),
    }
}
fn github_evidence_class(error: &tl3_github::Error) -> String {
    match error {
        tl3_github::Error::Http { status } => format!("http_{status}"),
        tl3_github::Error::CatchUp { source, .. }
        | tl3_github::Error::InstallationToken { source } => github_evidence_class(source),
        _ => error.failure_class().as_str().into(),
    }
}
fn evidence_read<T>(
    evidence: &mut crate::outcome::MergeEvidence,
    read: &str,
    result: Result<T>,
) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(error) => {
            evidence.error(read, evidence_error_class(&error));
            None
        }
    }
}
fn collect_merge_evidence(
    runtime: &tokio::runtime::Runtime,
    github: &GitHub,
    snapshot: &Snapshot,
    node: tl3_kernel::NodeId,
    request: MergeRead<'_>,
    evidence: &mut crate::outcome::MergeEvidence,
) {
    let pull = request.pull;
    let repo = match crate::request(snapshot, node) {
        Ok(request) => full_name(&request.repo),
        Err(error) => {
            evidence.error("request", evidence_error_class(&error));
            return;
        }
    };
    evidence.observed = request.outside;
    evidence.landed.pr_base_sha = Some(pull.base.sha.clone());
    evidence.landed.base_before_merge = request.base.map(str::to_owned);
    evidence.landed.method = request.method;
    if let Some(world) = evidence_read(evidence, "snapshot", crate::snapshot::decode(snapshot)) {
        if let Some(n) = world.nodes.get(node.0 as usize) {
            evidence.tl3_checks_passed =
                Some(crate::commit_sha(snapshot, n.checked) == Some(pull.head.sha.as_str()));
            evidence.review_approved =
                Some(crate::commit_sha(snapshot, n.approved) == Some(pull.head.sha.as_str()));
            evidence.landed.checked_base = crate::commit_sha(snapshot, n.approved)
                .and_then(|approved| passing_check_base(snapshot, node, approved))
                .map(str::to_owned);
        } else {
            evidence.error("snapshot", "missing_node");
        }
    }
    evidence.required_checks = if let Some(checks) = request.checks {
        Some(checks.clone())
    } else {
        evidence_read(
            evidence,
            "required_checks",
            read_checks(runtime, github, &repo, pull),
        )
        .map(|checks| crate::outcome::RequiredChecks {
            passed: checks.checks == Checks::Passed,
            count: checks.count,
        })
    };
    evidence.landed.merge_base = if request.observed {
        evidence_read(
            evidence,
            "live_merge_base",
            live_merge_base(runtime, github, &repo, pull),
        )
    } else {
        request.base.map(str::to_owned)
    };
    let mut landed = request.commit.map(str::to_owned);
    if landed.is_none() {
        landed = evidence_read(
            evidence,
            "pull_request",
            runtime
                .block_on(github.pull_request(&repo, pull.number))
                .map_err(Error::from),
        )
        .and_then(|current| current.merge_commit_sha);
        if landed.is_none() {
            evidence.error("merge_commit_sha", "missing");
        }
    }
    evidence.landed.commit = landed.clone();
    if let Some(landed) = landed
        && let Some(commit) = evidence_read(
            evidence,
            "landed_commit",
            runtime
                .block_on(github.git_commit(&repo, &landed))
                .map_err(Error::from),
        )
    {
        evidence.landed.tree = Some(commit.tree.sha);
        evidence.landed.parents = Some(commit.parents.into_iter().map(|parent| parent.sha).collect());
    }
    if let Some(commit) = evidence_read(
        evidence,
        "head_commit",
        runtime
            .block_on(github.git_commit(&repo, &pull.head.sha))
            .map_err(Error::from),
    ) {
        evidence.landed.head_tree = Some(commit.tree.sha);
    }
    evidence.compare();
}
