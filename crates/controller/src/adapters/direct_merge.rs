//! #830: SHA-pinned direct delivery behind the kernel's existing Enqueue effect.
pub mod decide;
mod events;
mod repository_config;
pub use events::Translator;

use super::{
    mergify::{Evidence, QueueFact},
    record::Records,
};
use crate::{
    Error, Repository, Result,
    executor::{Completion, Delivery, Executor, Look, Observation, Output, Reconciliation},
};
use decide::{Checks, Facts, Head, Outcome, Plan, Step};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tl3_github::{CheckRun, GitHub, MergeMethod, PullRequest};
use tl3_kernel::{EffectKind, Phase};
use tl3_store::{Snapshot, Store};

pub const VISIBILITY: &str = "delivered directly (CI bypassed by repo config)";

#[derive(Serialize, Deserialize)]
struct Intent {
    head: String,
    attempt: u64,
    action: String,
    method: MergeMethod,
}
#[derive(Serialize, Deserialize)]
struct Merged {
    head: String,
    checks_passed: bool,
}
struct Read {
    pull: PullRequest,
    facts: Facts,
    failure: Option<Evidence>,
    method: Option<MergeMethod>,
}

/// Sleeping effects are invalidated only by relevant webhook/catch-up events.
/// A new adapter after restart starts awake and reconciles from GitHub.
#[derive(Clone, Default)]
pub struct Wakes(Arc<Mutex<BTreeSet<String>>>);
impl Wakes {
    fn sleeping(&self, key: &str) -> bool {
        self.0.lock().unwrap().contains(key)
    }
    fn sleep(&self, key: &str) {
        self.0.lock().unwrap().insert(key.to_owned());
    }
    fn wake(&self, key: &str) {
        self.0.lock().unwrap().remove(key);
    }
}

/// Durable and upstream write boundaries exposed for process-crash verification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Write {
    Fence,
    Intent,
    Annotation,
    Merge,
    Receipt,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WriteBoundary {
    pub write: Write,
    pub after: bool,
}

pub struct Adapter {
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
        Ok(Self {
            boundary: None,
            wakes: Wakes::default(),
            clients,
            epoch,
            records: Records::new(records),
            store,
            runtime: crate::executor::runtime()?,
        })
    }
    pub fn with_wakes(mut self, wakes: Wakes) -> Self {
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
        Ok(self
            .records
            .path(&format!(
                "direct:{}:{}/{}#{}",
                d.repository.installation,
                d.repository.owner.to_ascii_lowercase(),
                d.repository.name.to_ascii_lowercase(),
                number(d)?
            ))
            .with_extension("merged"))
    }
    fn recorded(&self, d: &Delivery) -> Result<Option<Merged>> {
        read_file(&self.receipt_path(d)?)
    }
    fn snapshot(&self) -> Result<Snapshot> {
        Ok(Store::open(&self.store)?.load()?)
    }
    fn fence(&mut self, d: &Delivery) -> Result<()> {
        let snapshot = self.snapshot()?;
        if d.epoch < self.epoch || snapshot.epoch.get() != d.epoch {
            return Err(Error::Invalid("direct delivery epoch is fenced"));
        }
        let world = crate::snapshot::decode(&snapshot)?;
        let n = world
            .nodes
            .get(d.node().0 as usize)
            .ok_or(Error::Invalid("missing direct delivery node"))?;
        if n.generation != d.generation
            || n.paused
            || n.waiting_incident
            || !matches!(n.phase, Phase::Publishing | Phase::Queued)
        {
            return Err(Error::ChecksPending);
        }
        self.boundary(Write::Fence, false);
        self.records.fence(d.epoch)?;
        self.boundary(Write::Fence, true);
        self.epoch = d.epoch;
        Ok(())
    }
    fn read(&self, d: &Delivery) -> Result<Read> {
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
            let required = self
                .runtime
                .block_on(github.required_status_checks(&repo, &pull.base.branch))?;
            if !required.unavailable.is_empty() {
                return Err(Error::Permanent {
                    class: "direct_protection_read",
                    source: required.unavailable.join("; ").into(),
                });
            }
            let runs = self
                .runtime
                .block_on(github.check_runs(&repo, &pull.head.sha))?;
            // The durable kernel approval is checked separately below. This is Q44's own-app check.
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
            approval = !own.is_empty()
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
                self.runtime
                    .block_on(github.commit_statuses(&repo, &pull.head.sha))?
            } else {
                vec![]
            };
            checks = Checks::Passed;
            for rule in required.checks {
                let matching: Vec<_> = latest
                    .values()
                    .filter(|check| {
                        check.name == rule.context
                            && rule
                                .integration_id
                                .is_none_or(|id| check.app.as_ref().is_some_and(|app| app.id == id))
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
                let all_passed = if !matching.is_empty() {
                    matching.iter().all(|check| passed(check))
                } else {
                    status.is_some_and(|status| status.state == "success")
                };
                if !all_passed && checks != Checks::Failed {
                    checks = Checks::Pending;
                }
                let bad = matching.iter().find(|check| {
                    check.status == "completed"
                        && matches!(
                            check.conclusion.as_deref(),
                            Some(
                                "failure"
                                    | "timed_out"
                                    | "cancelled"
                                    | "action_required"
                                    | "startup_failure"
                                    | "stale"
                            )
                        )
                });
                if let Some(check) = bad {
                    checks = Checks::Failed;
                    failure = Some(Evidence::CiFailed {
                        check: check.id,
                        completed_at: check.completed_at.map(|time| time.to_rfc3339()),
                        head: pull.head.sha.clone(),
                        infra: false,
                        head_check: true,
                        details: Default::default(),
                    });
                } else if let Some(status) =
                    status.filter(|status| matches!(status.state.as_str(), "error" | "failure"))
                {
                    checks = Checks::Failed;
                    failure = Some(Evidence::CiFailed {
                        check: status.id,
                        completed_at: status.updated_at.map(|time| time.to_rfc3339()),
                        head: pull.head.sha.clone(),
                        infra: false,
                        head_check: true,
                        details: Default::default(),
                    });
                }
            }
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
            || n.generation != d.generation
            || !matches!(n.phase, Phase::Publishing | Phase::Queued)
            || snapshot.rows.iter().any(|row| {
                row.table == "incidents"
                    && row.value["open"] == true
                    && row.value["node"].as_u64() == Some(d.node().0)
            });
        Ok(Read {
            facts: Facts {
                approved,
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
        })
    }
    fn fact(&self, d: &Delivery, evidence: Evidence) -> Result<Completion> {
        let fact = QueueFact {
            repository: d.repository.clone(),
            number: number(d)?,
            evidence,
        };
        let observations = if fact.recorded(&self.snapshot()?)? {
            vec![]
        } else {
            vec![Observation::Queue(fact)]
        };
        Ok(Completion::confirmed(observations))
    }
    fn merged(&self, d: &Delivery, merged: Merged) -> Result<Completion> {
        self.write(
            Write::Receipt,
            &self.receipt_path(d)?,
            &serde_json::to_vec(&merged)?,
        )?;
        self.fact(
            d,
            Evidence::Merged {
                head: merged.head,
                checks_passed: merged.checks_passed,
            },
        )
    }
    fn result(&self, d: &Delivery, read: &Read, plan: &Plan) -> Result<Option<Completion>> {
        Ok(Some(match plan.outcome {
            Outcome::Merged => {
                let matched = read.facts.observed == read.facts.approved
                    && read.facts.checking
                    && read.facts.checked == read.facts.approved;
                self.merged(
                    d,
                    Merged {
                        head: read.pull.head.sha.clone(),
                        checks_passed: matched,
                    },
                )?
            }
            Outcome::Moved => self.fact(
                d,
                Evidence::PushByOther {
                    head: read.pull.head.sha.clone(),
                },
            )?,
            Outcome::Conflict => {
                Completion::confirmed(vec![Observation::Kernel(tl3_kernel::Event::Fact {
                    node: d.node(),
                    fact: tl3_kernel::Fact::Conflict,
                })])
            }
            Outcome::Failed => self.fact(
                d,
                read.failure
                    .clone()
                    .ok_or(Error::Invalid("direct check failure lacks evidence"))?,
            )?,
            Outcome::Closed => Completion::permanent("direct_pr_closed"),
            Outcome::Wait => {
                self.wakes.sleep(&d.key);
                return Err(Error::ChecksPending);
            }
            Outcome::Held => return Err(Error::ChecksPending),
            Outcome::Ready => return Ok(None),
        }))
    }
    fn attempt(&mut self, d: &Delivery) -> Result<Completion> {
        number(d)?;
        if let Some(merged) = self.recorded(d)? {
            return self.fact(
                d,
                Evidence::Merged {
                    head: merged.head,
                    checks_passed: merged.checks_passed,
                },
            );
        }
        if self.wakes.sleeping(&d.key) {
            return Err(Error::ChecksPending);
        }
        // At most one annotation and one merge per call. A pending check never loops.
        for _ in 0..2 {
            let read = self.read(d)?;
            let plan = decide::decide(read.facts);
            if let Some(completion) = self.result(d, &read, &plan)? {
                return Ok(completion);
            }
            let github = self.client(d)?.clone();
            let repo = full_name(&d.repository);
            let sha = format!("{:032x}{:08x}", plan.sha.upper, plan.sha.lower);
            for step in [plan.first, plan.second, plan.third] {
                match step {
                    Step::None => {}
                    Step::Fence => self.fence(d)?,
                    Step::Intent => {
                        let path = self.records.path(&d.key).with_extension("intent");
                        let previous: Option<Intent> = read_file(&path)?;
                        let attempt = previous.map_or(Ok(1), |intent| {
                            intent
                                .attempt
                                .checked_add(1)
                                .ok_or(Error::Invalid("direct intent counter exhausted"))
                        })?;
                        self.write(
                            Write::Intent,
                            &path,
                            &serde_json::to_vec(&Intent {
                                head: sha.clone(),
                                attempt,
                                action: format!("{:?}", plan.third),
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
                                return self.merged(
                                    d,
                                    Merged {
                                        head: sha,
                                        checks_passed: true,
                                    },
                                );
                            }
                            Ok(_) | Err(tl3_github::Error::Http { status: 405 }) => {
                                // Already-merged refusals converge; classify a genuine refusal from fresh evidence.
                                let current = self.read(d)?;
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
                                    return self.merged(
                                        d,
                                        Merged {
                                            checks_passed: current.head.sha == sha,
                                            head: current.head.sha,
                                        },
                                    );
                                }
                                return self.fact(
                                    d,
                                    Evidence::PushByOther {
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
                                    return self.merged(
                                        d,
                                        Merged {
                                            checks_passed: current.head.sha == sha,
                                            head: current.head.sha,
                                        },
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
        self.attempt(d)
    }
    fn reconcile(&mut self, d: &Delivery) -> Result<Reconciliation> {
        // Restart/webhook reconciliation is a fresh attempt, including all eligibility reads.
        match self.attempt(d) {
            Ok(completion) => Ok(Reconciliation::Found(Box::new(completion))),
            Err(Error::ChecksPending) => Ok(Reconciliation::Pending),
            Err(error) => Err(error),
        }
    }
    fn observations(&mut self, d: &Delivery) -> Result<Vec<Observation>> {
        if let Some(merged) = self.recorded(d)? {
            return Ok(self
                .fact(
                    d,
                    Evidence::Merged {
                        head: merged.head,
                        checks_passed: merged.checks_passed,
                    },
                )?
                .observations);
        }
        if self.wakes.sleeping(&d.key) {
            return Ok(vec![]);
        }
        let read = self.read(d)?;
        let plan = decide::decide(read.facts);
        match self.result(d, &read, &plan) {
            Ok(Some(completion)) => Ok(completion.observations),
            Ok(None) | Err(Error::ChecksPending) => Ok(vec![]),
            Err(error) => Err(error),
        }
    }
    fn look(&mut self, d: &Delivery) -> Result<Look> {
        Ok(match self.reconcile(d)? {
            Reconciliation::Found(completion) => Look::Found(completion),
            Reconciliation::Absent => Look::Absent,
            Reconciliation::Pending => Look::Pending,
        })
    }
    fn reconcile_cancellation(&mut self, d: &Delivery) -> Result<Reconciliation> {
        let github = self.client(d)?;
        let pull = self
            .runtime
            .block_on(github.pull_request(&full_name(&d.repository), number(d)?))?;
        let completion = if pull.merged_at.is_some() {
            self.merged(
                d,
                Merged {
                    checks_passed: Some(pull.head.sha.as_str()) == d.head_sha.as_deref(),
                    head: pull.head.sha,
                },
            )?
        } else {
            Completion::confirmed(vec![])
        };
        Ok(Reconciliation::Found(Box::new(completion)))
    }
    fn probe(&mut self, d: &Delivery) -> Result<bool> {
        self.runtime.block_on(
            self.client(d)?
                .pull_request(&full_name(&d.repository), number(d)?),
        )?;
        Ok(true)
    }
    fn app_login(&self, installation: u64) -> Option<String> {
        self.clients
            .get(&installation)
            .map(|client| format!("{}[bot]", client.app_slug()))
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
fn passed(check: &CheckRun) -> bool {
    check.status == "completed"
        && matches!(
            check.conclusion.as_deref(),
            Some("success" | "neutral" | "skipped")
        )
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
