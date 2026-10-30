use crate::{ActionKey, Error, PrismaBuild};
use serde::Deserialize;
use serde_json::Value;
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
    pub state: ActionState,
    pub published_unix: f64,
    pub attempts: Vec<Attempt>,
    pub lease: Option<Lease>,
    pub detail: Value,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Lease {
    pub heartbeat_unix: f64,
    pub owner: String,
    pub host: String,
    pub progress_observation: Option<Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Attempt {
    pub number: u32,
    pub status: String,
    pub disposition: String,
    pub detail: Value,
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
struct Record {
    schema: String,
    action_key: ActionKey,
    published_unix: f64,
    status: Option<String>,
    #[serde(default)]
    detail: Value,
    #[serde(default)]
    attempt_history: Vec<Link>,
    #[serde(default)]
    attempt_history_before_withdrawal: Vec<Link>,
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
    detail: Value,
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
        let root = &self.options.queue_root;
        let mut chosen: Option<(usize, PathBuf, Record)> = None;
        for (rank, state) in STATES.iter().enumerate() {
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
            let Some(record) = document::<Record>(&path).await? else {
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
        let Some((rank, path, record)) = chosen else {
            return Ok(None);
        };
        let status = record.status.as_deref();
        let returncode = match record.detail.get("returncode") {
            None | Some(Value::Null) => None,
            Some(value) => Some(
                value
                    .as_i64()
                    .and_then(|n| i32::try_from(n).ok())
                    .ok_or_else(|| invalid(&path, "invalid returncode"))?,
            ),
        };
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
            record.attempt_history_before_withdrawal
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
                detail: attempt.detail,
            });
        }
        let lease = if state == ActionState::Claimed {
            let lease_path = root.join("claimed").join(format!("{key}.lease"));
            let raw: Option<Value> = document(&lease_path).await?;
            match raw {
                None => None,
                Some(value) => {
                    if value.get("action_key").and_then(Value::as_str) != Some(key.as_str())
                        || value.get("schema").and_then(Value::as_str)
                            != Some("prismaquant.prismabuild.pool_lease.v1")
                    {
                        return Err(invalid(&lease_path, "lease identity or schema mismatch"));
                    }
                    let lease: Lease = serde_json::from_value(value)
                        .map_err(|e| invalid(&lease_path, e.to_string()))?;
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
            state,
            published_unix: record.published_unix,
            attempts,
            lease,
            detail: record.detail,
        }))
    }

    pub async fn reconcile(
        &self,
        awaited: &BTreeSet<ActionKey>,
    ) -> Result<BTreeMap<ActionKey, Option<ActionRecord>>, Error> {
        let mut records = BTreeMap::new();
        for key in awaited {
            records.insert(key.clone(), self.read(key).await?);
        }
        Ok(records)
    }
}
