use crate::{ActionKey, Error, PrismaBuild};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path, PathBuf},
};

const STATES: [&str; 5] = ["ready", "claimed", "failed", "done", "withdrawn"];
const ITEM_SCHEMA: &str = "prismaquant.prismabuild.pool_item.v1";
const OUTCOME_SCHEMA: &str = "prismaquant.prismabuild.pool_outcome.v1";

#[derive(Clone, Debug, PartialEq)]
pub enum ActionState {
    Queued,
    Claimed,
    Finished {
        returncode: i32,
    },
    Failed {
        status: String,
        returncode: Option<i32>,
    },
    LeaseLost,
    Withdrawn,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ActionRecord {
    pub key: ActionKey,
    pub snapshot_parent: Option<String>,
    pub state: ActionState,
    pub published_unix: f64,
    pub attempts: Vec<Attempt>,
    pub lease: Option<Lease>,
    pub returncode: Option<i32>,
    pub termination_reason: Option<String>,
    pub stderr_tail: String,
    pub progress_observation: Option<ProgressObservation>,
}

/// Maximum retained stderr suffix, in UTF-8 bytes.
pub const MAX_STDERR_TAIL_BYTES: usize = 8192;

/// Only the progress counters and timestamps used by controller looks are retained.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct ProgressObservation {
    pub sampled_unix: Option<f64>,
    pub accepted_count: Option<u64>,
    pub rejected_count: Option<u64>,
    pub last_rejection: Option<String>,
    pub last_accepted: Option<AcceptedProgress>,
    pub quiet_s: Option<f64>,
    pub grace_s: Option<f64>,
    pub phase: Option<String>,
    pub phases_entered: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct AcceptedProgress {
    pub phase: String,
    pub units_completed: f64,
    pub reported_unix: f64,
    pub unit: Option<String>,
}

/// Results are independent: one unreadable key does not hide other observations.
pub type Observations = BTreeMap<ActionKey, Result<Option<ActionRecord>, Error>>;

#[derive(Default, Deserialize)]
struct Detail {
    returncode: Option<i32>,
    action_returncode: Option<i32>,
    termination_reason: Option<String>,
    #[serde(default, rename = "stderr", deserialize_with = "stderr_tail")]
    stderr_tail: String,
    progress_observation: Option<ProgressObservation>,
}

impl Detail {
    fn returncode(&self) -> Option<i32> {
        self.action_returncode.or(self.returncode)
    }
}

fn stderr_tail<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let text = Option::<String>::deserialize(deserializer)?.unwrap_or_default();
    let mut start = text.len().saturating_sub(MAX_STDERR_TAIL_BYTES);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    Ok(text[start..].to_owned())
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Lease {
    pub heartbeat_unix: f64,
    pub owner: String,
    pub host: String,
    pub progress_observation: Option<ProgressObservation>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Attempt {
    pub number: u32,
    pub status: String,
    pub disposition: String,
    pub returncode: Option<i32>,
    pub termination_reason: Option<String>,
    pub stderr_tail: String,
    pub progress_observation: Option<ProgressObservation>,
}

impl Attempt {
    pub fn lease_lost(&self) -> bool {
        lease_lost(&self.status)
    }
}

fn lease_lost(status: &str) -> bool {
    matches!(status, "lease_lost" | "lease_lost_max_attempts")
}

#[derive(Deserialize)]
struct LeaseWire {
    schema: String,
    action_key: ActionKey,
    #[serde(flatten)]
    lease: Lease,
}

#[derive(Deserialize)]
struct CheckoutSnapshot {
    parent: String,
}

#[derive(Deserialize)]
struct Record {
    schema: String,
    action_key: ActionKey,
    published_unix: f64,
    checkout_snapshot: Option<CheckoutSnapshot>,
    status: Option<String>,
    #[serde(default)]
    detail: Detail,
    #[serde(default)]
    attempt_history: Vec<Link>,
    withdrawn_attempt: Option<Box<Record>>,
}

#[derive(Deserialize)]
struct Link {
    attempt: u32,
    outcome: PathBuf,
}

#[derive(Deserialize)]
struct AttemptWire {
    schema: String,
    action_key: ActionKey,
    published_unix: f64,
    attempt: u32,
    status: String,
    disposition: String,
    detail: Detail,
}

fn invalid(path: &Path, reason: impl Into<String>) -> Error {
    Error::Record {
        path: path.into(),
        reason: reason.into(),
    }
}

async fn document<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>, Error> {
    let bytes = match tokio::fs::read(path).await {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(Error::Io {
                path: path.into(),
                source,
            });
        }
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| invalid(path, e.to_string()))
}

impl PrismaBuild {
    /// Missing records return None. Unreadable records remain errors, never evidence of loss.
    pub async fn read(&self, key: &ActionKey) -> Result<Option<ActionRecord>, Error> {
        self.read_with(key, |_| {}).await
    }

    async fn read_with(
        &self,
        key: &ActionKey,
        mut after_read: impl FnMut(&str),
    ) -> Result<Option<ActionRecord>, Error> {
        let root = &self.options.queue_root;
        let mut chosen: Option<(usize, PathBuf, Record)> = None;
        // Claims can return to ready. Revisit live slots after the terminal scan
        // so a link-ready/unlink-claimed handoff cannot look like an ending.
        for state in STATES.iter().chain(["ready", "claimed"].iter()) {
            let rank = STATES.iter().position(|candidate| candidate == state).unwrap();
            let directory = root.join(state);
            let metadata = tokio::fs::metadata(&directory)
                .await
                .map_err(|source| Error::Io {
                    path: directory.clone(),
                    source,
                })?;
            if !metadata.is_dir() {
                return Err(invalid(&directory, "expected a queue state directory"));
            }
            let path = directory.join(format!("{key}.json"));
            let record = document::<Record>(&path).await?;
            after_read(state);
            let Some(record) = record else {
                continue;
            };
            if &record.action_key != key
                || !record.published_unix.is_finite()
                || record.published_unix < 0.0
                || !matches!(record.schema.as_str(), ITEM_SCHEMA | OUTCOME_SCHEMA)
            {
                return Err(invalid(
                    &path,
                    "invalid queue identity, generation or schema",
                ));
            }
            // During handoff both slots can exist. A newer publication wins over an old ending.
            if chosen.as_ref().is_none_or(|(old_rank, _, old)| {
                record.published_unix > old.published_unix
                    || (record.published_unix == old.published_unix && rank > *old_rank)
            }) {
                chosen = Some((rank, path, record));
            }
        }
        // PrismaBuild entombs claims before publishing retries or endings.
        // Its own RecordStore.standing treats these sidecars as moving.
        let claimed = root.join("claimed");
        let mut entries = tokio::fs::read_dir(&claimed).await.map_err(|source| Error::Io {
            path: claimed.clone(), source,
        })?;
        while let Some(entry) = entries.next_entry().await.map_err(|source| Error::Io {
            path: claimed.clone(), source,
        })? {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(&format!("{key}."))
                && (name.ends_with(".tombstone") || name.ends_with(".late-finish"))
            {
                return Err(invalid(&entry.path(), "queue record is moving; reconcile again"));
            }
        }
        let Some((rank, path, record)) = chosen else {
            return Ok(None);
        };
        let status = record.status.as_deref();
        let returncode = record.detail.returncode();
        let state = match STATES[rank] {
            "ready" => ActionState::Queued,
            "claimed" => ActionState::Claimed,
            "withdrawn" => ActionState::Withdrawn,
            "failed" if status.is_some_and(lease_lost) => ActionState::LeaseLost,
            "failed" => ActionState::Failed {
                status: status
                    .ok_or_else(|| invalid(&path, "failed record has no status"))?
                    .into(),
                returncode,
            },
            "done" if matches!(status, Some("executed" | "cache_hit")) => ActionState::Finished {
                returncode: returncode
                    .or_else(|| (status == Some("cache_hit")).then_some(0))
                    .ok_or_else(|| invalid(&path, "finished record has no returncode"))?,
            },
            _ => return Err(invalid(&path, "unexpected terminal status")),
        };
        let mut attempts = Vec::new();
        let mut previous = 0;
        let history = if state == ActionState::Withdrawn {
            if let Some(withdrawn) = record.withdrawn_attempt {
                if withdrawn.action_key != record.action_key
                    || withdrawn.published_unix != record.published_unix
                    || withdrawn.schema != ITEM_SCHEMA
                {
                    return Err(invalid(
                        &path,
                        "withdrawn attempt identity or schema mismatch",
                    ));
                }
                withdrawn.attempt_history
            } else {
                record.attempt_history
            }
        } else {
            record.attempt_history
        };
        for link in history {
            let parts: Vec<_> = link.outcome.components().collect();
            let safe = parts.len() == 4
                && parts.iter().all(|p| matches!(p, Component::Normal(_)))
                && parts[0].as_os_str() == "attempts"
                && parts[1].as_os_str() == key.as_str()
                && parts[2]
                    .as_os_str()
                    .to_str()
                    .is_some_and(|v| v.parse::<ActionKey>().is_ok())
                && parts[3].as_os_str() == format!("{:08}.json", link.attempt).as_str();
            if !safe || link.attempt == 0 || link.attempt <= previous {
                return Err(invalid(&path, "invalid attempt history link"));
            }
            let attempt_path = root.join(&link.outcome);
            let attempt: AttemptWire = document(&attempt_path)
                .await?
                .ok_or_else(|| invalid(&attempt_path, "linked attempt is missing"))?;
            if attempt.schema != "prismaquant.prismabuild.pool_attempt.v1"
                || &attempt.action_key != key
                || attempt.published_unix != record.published_unix
                || attempt.attempt != link.attempt
            {
                return Err(invalid(
                    &attempt_path,
                    "attempt identity differs from queue record",
                ));
            }
            previous = attempt.attempt;
            attempts.push(Attempt {
                number: attempt.attempt,
                status: attempt.status,
                disposition: attempt.disposition,
                returncode: attempt.detail.returncode(),
                termination_reason: attempt.detail.termination_reason,
                stderr_tail: attempt.detail.stderr_tail,
                progress_observation: attempt.detail.progress_observation,
            });
        }
        let lease = if state == ActionState::Claimed {
            let lease_path = root.join("claimed").join(format!("{key}.lease"));
            let raw: Option<LeaseWire> = document(&lease_path).await?;
            match raw {
                None => None,
                Some(value) => {
                    if value.action_key != *key
                        || value.schema != "prismaquant.prismabuild.pool_lease.v1"
                    {
                        return Err(invalid(&lease_path, "lease identity or schema mismatch"));
                    }
                    let lease = value.lease;
                    if !lease.heartbeat_unix.is_finite() || lease.heartbeat_unix < 0.0 {
                        return Err(invalid(&lease_path, "invalid heartbeat"));
                    }
                    Some(lease)
                }
            }
        } else {
            None
        };
        Ok(Some(ActionRecord {
            key: key.clone(),
            snapshot_parent: record.checkout_snapshot.map(|s| s.parent),
            state,
            published_unix: record.published_unix,
            attempts,
            lease,
            returncode,
            termination_reason: record.detail.termination_reason,
            stderr_tail: record.detail.stderr_tail,
            progress_observation: record.detail.progress_observation,
        }))
    }

    pub async fn reconcile(&self, awaited: &BTreeSet<ActionKey>) -> Observations {
        let mut records = BTreeMap::new();
        for key in awaited {
            records.insert(key.clone(), self.read(key).await);
        }
        records
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Options, Tool};

    fn fixture() -> (tempfile::TempDir, PrismaBuild, ActionKey) {
        let root = tempfile::tempdir().unwrap();
        for state in STATES {
            std::fs::create_dir(root.path().join(state)).unwrap();
        }
        let key: ActionKey = "a".repeat(64).parse().unwrap();
        let client = PrismaBuild::new(Options {
            queue_root: root.path().into(),
            tool: Tool { program: "/never-submit".into(), args: vec![] },
            bell_url: "http://127.0.0.1:9/".into(),
        }).unwrap();
        std::fs::write(root.path().join(format!("claimed/{key}.json")),
            serde_json::to_vec(&serde_json::json!({
                "schema": ITEM_SCHEMA, "action_key": key, "published_unix": 100
            })).unwrap()).unwrap();
        (root, client, key)
    }

    #[tokio::test]
    async fn lookup_keeps_unstarted_claim_live_during_requeue() {
        let (root, client, key) = fixture();
        let mut moved = false;
        let record = client.read_with(&key, |state| {
            if state == "ready" && !moved {
                // PrismaBuild _ClaimReservation.abandon links ready before unlinking claimed.
                let claimed = root.path().join(format!("claimed/{key}.json"));
                std::fs::hard_link(&claimed, root.path().join(format!("ready/{key}.json"))).unwrap();
                std::fs::remove_file(claimed).unwrap();
                moved = true;
            }
        }).await.unwrap();
        assert!(record.is_some_and(|record| matches!(record.state, ActionState::Queued | ActionState::Claimed)),
            "live requeue read as absent");
    }

    #[tokio::test]
    async fn lookup_does_not_declare_absence_during_finish_handoff() {
        let (root, client, key) = fixture();
        std::fs::rename(root.path().join(format!("claimed/{key}.json")),
            root.path().join(format!("claimed/{key}.100.host.1.abc.tombstone"))).unwrap();
        assert!(client.read(&key).await.is_err(), "in-flight handoff read as absent");
    }
}
