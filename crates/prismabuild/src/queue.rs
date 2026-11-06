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

/// A transition owns the record until its destination is published.
#[derive(Clone, Debug, PartialEq)]
pub enum RecordRead {
    Absent,
    Moving,
    Present(Box<ActionRecord>),
}

impl RecordRead {
    /// Moving records retain their waits and cannot establish termination.
    pub fn is_live(&self) -> bool {
        match self {
            Self::Moving => true,
            Self::Present(record) => {
                matches!(record.state, ActionState::Queued | ActionState::Claimed)
            }
            Self::Absent => false,
        }
    }
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
pub type Observations = BTreeMap<ActionKey, Result<RecordRead, Error>>;

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
    /// Contended-heartbeat stamps under `lease-heartbeats/<key>/`, read beside
    /// the lease. They are not part of the lease document.
    #[serde(skip)]
    pub stamps: HeartbeatStamps,
}

/// PrismaBuild's `_stamp_contended_heartbeat` renews only these stamps, never
/// the lease, while the claim's key lock is contended.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HeartbeatStamps {
    /// `heartbeat_unix` of every well-formed stamp naming this action.
    pub heartbeats: Vec<f64>,
    /// A stamp could not be read. PrismaBuild: "unreadable I/O retains custody".
    pub unreadable: bool,
}

/// PrismaBuild's lease timeout (`pool.LEASE_TIMEOUT_S`, 300 s): a claim whose
/// effective heartbeat is older is expired by PrismaBuild's own contract
/// (`PoolQueue._effective_heartbeat`; design.md, "Lease").
pub const LEASE_TIMEOUT_S: f64 = 300.0;
/// TL3 reads a claim as lost only past three lease timeouts, so a PrismaBuild
/// that raises its lease timeout or renewal interval up to 3x never makes a
/// live claim read lost; the live wedges (PB #2819, #2821) were hours stale.
pub const LOST_AFTER_S: f64 = 3.0 * LEASE_TIMEOUT_S;
/// PrismaBuild's cross-host skew bound for a stamp (`OFFER_FUTURE_TOLERANCE_S`).
pub const HEARTBEAT_FUTURE_TOLERANCE_S: f64 = 60.0;

impl Lease {
    /// Q43 (#817, found by: live probe, PB #2819 and #2821): a lease whose
    /// effective heartbeat is older than `LOST_AFTER_S` is the claim lost by its
    /// own record, even while the record stays CLAIMED.
    ///
    /// The effective heartbeat follows `_effective_heartbeat`: the lease beat,
    /// raised by any stamp within the skew bound (capped at now). Conservative
    /// reading: PrismaBuild credits only the stamp of the lease's exact claim
    /// identity; here every well-formed stamp naming the action counts, so a
    /// doubt keeps the claim alive. An unreadable stamp keeps it alive too.
    pub fn expired(&self, now_unix: f64) -> bool {
        if self.stamps.unreadable {
            return false;
        }
        let beat = self
            .stamps
            .heartbeats
            .iter()
            .filter(|stamp| **stamp - now_unix <= HEARTBEAT_FUTURE_TOLERANCE_S)
            .map(|stamp| stamp.min(now_unix))
            .fold(self.heartbeat_unix, f64::max);
        now_unix - beat > LOST_AFTER_S
    }
}

/// Bounds one stamp read; PrismaBuild's stamps are a few hundred bytes.
const MAX_STAMP_BYTES: u64 = 64 * 1024;

async fn heartbeat_stamps(directory: &Path, key: &ActionKey) -> HeartbeatStamps {
    let mut stamps = HeartbeatStamps::default();
    let mut entries = match tokio::fs::read_dir(directory).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return stamps,
        Err(_) => {
            stamps.unreadable = true;
            return stamps;
        }
    };
    loop {
        let entry = match entries.next_entry().await {
            Ok(Some(entry)) => entry,
            Ok(None) => return stamps,
            Err(_) => {
                stamps.unreadable = true;
                return stamps;
            }
        };
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let bytes = match tokio::fs::metadata(&path).await {
            Ok(metadata) if metadata.len() > MAX_STAMP_BYTES => continue,
            Ok(_) => tokio::fs::read(&path).await,
            Err(e) => Err(e),
        };
        let bytes = match bytes {
            Ok(bytes) => bytes,
            // An atomic replace can remove the name between listing and read.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                stamps.unreadable = true;
                return stamps;
            }
        };
        // Malformed or foreign stamps add no credit, as in PrismaBuild.
        let Ok(stamp) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        if stamp["action_key"].as_str() == Some(key.as_str())
            && let Some(beat) = stamp["heartbeat_unix"].as_f64()
            && beat.is_finite()
        {
            stamps.heartbeats.push(beat);
        }
    }
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

const LEASE_SCHEMA: &str = "prismaquant.prismabuild.pool_lease.v1";

/// The claim lease is a PrismaBuild-internal queue file, not its public record
/// (#817 review 2, Flux). A lease TL3 cannot read as a v1 claim lease (absent,
/// not JSON, another schema, missing fields, a non-finite or negative beat)
/// gives no liveness verdict, so a declared wait stays alive. A v1 lease that
/// names another action is corruption and stays an error, as do I/O errors.
async fn claim_lease(path: &Path, key: &ActionKey) -> Result<Option<Lease>, Error> {
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
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Ok(None);
    };
    if value["schema"].as_str() != Some(LEASE_SCHEMA) {
        return Ok(None);
    }
    if value["action_key"].as_str() != Some(key.as_str()) {
        return Err(invalid(path, "lease identity mismatch"));
    }
    Ok(serde_json::from_value::<Lease>(value)
        .ok()
        .filter(|lease| lease.heartbeat_unix.is_finite() && lease.heartbeat_unix >= 0.0))
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
    /// Moving records are present evidence. Unreadable records remain errors, never evidence of loss.
    pub async fn read(&self, key: &ActionKey) -> Result<RecordRead, Error> {
        self.read_with(key, |_| {}).await
    }

    async fn read_with(
        &self,
        key: &ActionKey,
        mut after_read: impl FnMut(&str),
    ) -> Result<RecordRead, Error> {
        let root = &self.options.queue_root;
        let mut chosen: Option<(usize, PathBuf, Record)> = None;
        let mut ready_capture_seen = false;
        // Claims can return to ready. Revisit live slots after the terminal scan
        // so a link-ready/unlink-claimed handoff cannot look like an ending.
        for (index, state) in STATES.iter().chain(["ready", "claimed"].iter()).enumerate() {
            // PrismaBuild captures claims and ready items before publishing their destinations.
            // Scan before terminals, so publishing an ending and retiring its marker cannot hide both.
            if index == 2 && chosen.is_none() {
                for state in ["claimed", "ready-transitions"] {
                    let directory = root.join(state);
                    let mut entries = match tokio::fs::read_dir(&directory).await {
                        Ok(entries) => entries,
                        Err(error)
                            if state == "ready-transitions"
                                && error.kind() == std::io::ErrorKind::NotFound =>
                        {
                            continue;
                        }
                        Err(source) => {
                            return Err(Error::Io {
                                path: directory,
                                source,
                            });
                        }
                    };
                    while let Some(entry) =
                        entries.next_entry().await.map_err(|source| Error::Io {
                            path: directory.clone(),
                            source,
                        })?
                    {
                        let name = entry.file_name();
                        let name = name.to_string_lossy();
                        if name.starts_with(&format!("{key}."))
                            && (if state == "claimed" {
                                name.ends_with(".tombstone") || name.ends_with(".late-finish")
                            } else {
                                name.ends_with(".json")
                            })
                        {
                            if state == "claimed" {
                                return Ok(RecordRead::Moving);
                            }
                            // Ready captures have no retirement bell. Read their published ending now.
                            ready_capture_seen = true;
                            break;
                        }
                    }
                }
            }
            let rank = STATES
                .iter()
                .position(|candidate| candidate == state)
                .unwrap();
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
        let Some((rank, path, record)) = chosen else {
            return Ok(if ready_capture_seen {
                RecordRead::Moving
            } else {
                RecordRead::Absent
            });
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
            match claim_lease(&lease_path, key).await? {
                None => None,
                Some(mut lease) => {
                    lease.stamps =
                        heartbeat_stamps(&root.join("lease-heartbeats").join(key.as_str()), key)
                            .await;
                    Some(lease)
                }
            }
        } else {
            None
        };
        Ok(RecordRead::Present(Box::new(ActionRecord {
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
        })))
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
            tool: Tool {
                program: "/never-submit".into(),
                args: vec![],
            },
            bell_url: "http://127.0.0.1:9/".into(),
        })
        .unwrap();
        std::fs::write(
            root.path().join(format!("claimed/{key}.json")),
            serde_json::to_vec(&serde_json::json!({
                "schema": ITEM_SCHEMA, "action_key": key, "published_unix": 100
            }))
            .unwrap(),
        )
        .unwrap();
        (root, client, key)
    }

    #[test]
    fn lease_reads_lost_only_past_three_lease_timeouts() {
        let lease = Lease {
            heartbeat_unix: 1_000.0,
            owner: "host:1:owner".into(),
            host: "host".into(),
            progress_observation: None,
            stamps: HeartbeatStamps::default(),
        };
        assert!(!lease.expired(1_000.0 + 2.0 * LEASE_TIMEOUT_S));
        assert!(!lease.expired(1_000.0 + LOST_AFTER_S));
        assert!(lease.expired(1_000.0 + LOST_AFTER_S + 0.001));
        let now = 1_000.0 + 10.0 * LEASE_TIMEOUT_S;
        let stamped = |heartbeats: Vec<f64>, unreadable| Lease {
            stamps: HeartbeatStamps {
                heartbeats,
                unreadable,
            },
            ..lease.clone()
        };
        // A fresh stamp renews a stale lease; an old one does not.
        assert!(!stamped(vec![now - 1.0], false).expired(now));
        assert!(stamped(vec![now - LOST_AFTER_S - 1.0], false).expired(now));
        // A stamp past the skew bound adds no credit; within it, it counts as now.
        assert!(stamped(vec![now + HEARTBEAT_FUTURE_TOLERANCE_S + 1.0], false).expired(now));
        assert!(!stamped(vec![now + HEARTBEAT_FUTURE_TOLERANCE_S], false).expired(now));
        // Unreadable custody evidence keeps the claim alive.
        assert!(!stamped(vec![], true).expired(now));
    }

    #[tokio::test]
    async fn claim_lease_gives_no_verdict_for_leases_it_cannot_read_as_v1() {
        let (root, _client, key) = fixture();
        let path = root.path().join("claimed").join(format!("{key}.lease"));
        let lease = |value: serde_json::Value| {
            std::fs::write(&path, value.to_string()).unwrap();
        };
        assert!(claim_lease(&path, &key).await.unwrap().is_none(), "absent");
        std::fs::write(&path, "flux-lease 1").unwrap();
        assert!(
            claim_lease(&path, &key).await.unwrap().is_none(),
            "not JSON"
        );
        lease(
            serde_json::json!({"schema": "prismabuild.pool_lease.v2", "action_key": key,
            "heartbeat_unix": 1.0, "owner": "o", "host": "h"}),
        );
        assert!(
            claim_lease(&path, &key).await.unwrap().is_none(),
            "unknown schema"
        );
        for heartbeat in [
            serde_json::json!(null),
            serde_json::json!(-1.0),
            serde_json::json!("1"),
        ] {
            lease(
                serde_json::json!({"schema": LEASE_SCHEMA, "action_key": key,
                "heartbeat_unix": heartbeat, "owner": "o", "host": "h"}),
            );
            assert!(
                claim_lease(&path, &key).await.unwrap().is_none(),
                "{heartbeat}"
            );
        }
        lease(
            serde_json::json!({"schema": LEASE_SCHEMA, "action_key": key, "owner": "o", "host": "h"}),
        );
        assert!(
            claim_lease(&path, &key).await.unwrap().is_none(),
            "missing heartbeat"
        );
        lease(
            serde_json::json!({"schema": LEASE_SCHEMA, "action_key": "b".repeat(64),
            "heartbeat_unix": 1.0, "owner": "o", "host": "h"}),
        );
        assert!(
            claim_lease(&path, &key).await.is_err(),
            "a v1 lease for another action is corrupt"
        );
        lease(
            serde_json::json!({"schema": LEASE_SCHEMA, "action_key": key,
            "heartbeat_unix": 1.0, "owner": "o", "host": "h"}),
        );
        assert_eq!(
            claim_lease(&path, &key)
                .await
                .unwrap()
                .unwrap()
                .heartbeat_unix,
            1.0
        );
    }

    #[tokio::test]
    async fn heartbeat_stamps_read_only_well_formed_stamps_for_the_action() {
        let (root, _client, key) = fixture();
        let directory = root.path().join("lease-heartbeats").join(key.as_str());
        assert_eq!(
            heartbeat_stamps(&directory, &key).await,
            HeartbeatStamps::default()
        );
        std::fs::create_dir_all(&directory).unwrap();
        let write = |name: &str, body: String| std::fs::write(directory.join(name), body).unwrap();
        write(
            "a.json",
            serde_json::json!({"action_key": key, "heartbeat_unix": 7.5}).to_string(),
        );
        write(
            "b.json",
            serde_json::json!({"action_key": "b".repeat(64), "heartbeat_unix": 9.0}).to_string(),
        );
        write("c.json", "{not json".into());
        write(
            "d.tmp",
            serde_json::json!({"action_key": key, "heartbeat_unix": 9.0}).to_string(),
        );
        let stamps = heartbeat_stamps(&directory, &key).await;
        assert_eq!(stamps.heartbeats, vec![7.5]);
        assert!(!stamps.unreadable);
    }

    #[tokio::test]
    async fn lookup_keeps_unstarted_claim_live_during_requeue() {
        let (root, client, key) = fixture();
        let mut moved = false;
        let record = client
            .read_with(&key, |state| {
                if state == "ready" && !moved {
                    // PrismaBuild _ClaimReservation.abandon links ready before unlinking claimed.
                    let claimed = root.path().join(format!("claimed/{key}.json"));
                    std::fs::hard_link(&claimed, root.path().join(format!("ready/{key}.json")))
                        .unwrap();
                    std::fs::remove_file(claimed).unwrap();
                    moved = true;
                }
            })
            .await
            .unwrap();
        assert!(
            matches!(record, RecordRead::Present(record)
            if matches!(record.state, ActionState::Queued | ActionState::Claimed)),
            "live requeue read as absent"
        );
    }

    #[tokio::test]
    async fn lookup_does_not_declare_absence_during_finish_handoff() {
        let (root, client, key) = fixture();
        std::fs::rename(
            root.path().join(format!("claimed/{key}.json")),
            root.path()
                .join(format!("claimed/{key}.100.host.1.abc.tombstone")),
        )
        .unwrap();
        assert!(
            matches!(client.read(&key).await, Ok(RecordRead::Moving)),
            "in-flight handoff must be present moving evidence, not an error"
        );
    }

    #[tokio::test]
    async fn lookup_keeps_retry_live_when_finishing_marker_retires() {
        let (root, client, key) = fixture();
        let claimed = root.path().join(format!("claimed/{key}.json"));
        let marker = root
            .path()
            .join(format!("claimed/{key}.100.host.1.abc.tombstone"));
        std::fs::rename(&claimed, &marker).unwrap();
        let mut claimed_reads = 0;
        let record = client
            .read_with(&key, |state| {
                if state == "claimed" {
                    claimed_reads += 1;
                    if claimed_reads == 2 {
                        std::fs::hard_link(&marker, root.path().join(format!("ready/{key}.json")))
                            .unwrap();
                        std::fs::remove_file(&marker).unwrap();
                    }
                }
            })
            .await;
        assert!(
            record.is_ok_and(|record| record.is_live()),
            "retiring retry marker read as absent"
        );
    }
    #[tokio::test]
    async fn lookup_keeps_finish_present_when_marker_retires_after_terminals() {
        for finish_after in ["failed", "done", "withdrawn"] {
            let (root, client, key) = fixture();
            let marker = root
                .path()
                .join(format!("claimed/{key}.100.host.1.abc.tombstone"));
            std::fs::rename(root.path().join(format!("claimed/{key}.json")), &marker).unwrap();
            let record = client
                .read_with(&key, |state| {
                    if state == finish_after {
                        std::fs::write(
                            root.path().join(format!("done/{key}.json")),
                            serde_json::to_vec(&serde_json::json!({
                                "schema": OUTCOME_SCHEMA, "action_key": key, "published_unix": 100,
                                "status": "executed", "detail": {"returncode": 0}
                            }))
                            .unwrap(),
                        )
                        .unwrap();
                        std::fs::remove_file(&marker).unwrap();
                    }
                })
                .await;
            let present = match &record {
                Ok(RecordRead::Moving) => true,
                Ok(RecordRead::Present(record)) => {
                    record.state == ActionState::Finished { returncode: 0 }
                }
                _ => false,
            };
            assert!(
                present,
                "finish after {finish_after}: expected moving or successful terminal, got {record:?}"
            );
        }
    }

    #[tokio::test]
    async fn lookup_returns_terminal_while_ready_capture_remains() {
        let (root, client, key) = fixture();
        std::fs::create_dir(root.path().join("ready-transitions")).unwrap();
        let capture = root.path().join(format!(
            "ready-transitions/{key}.100000000.abc.origin-released.json"
        ));
        std::fs::rename(root.path().join(format!("claimed/{key}.json")), &capture).unwrap();
        std::fs::write(
            root.path().join(format!("failed/{key}.json")),
            serde_json::to_vec(&serde_json::json!({
                "schema": OUTCOME_SCHEMA, "action_key": key, "published_unix": 100,
                "status": "origin_consumer_released"
            }))
            .unwrap(),
        )
        .unwrap();
        let record = client.read(&key).await;
        assert!(
            matches!(&record, Ok(RecordRead::Present(record))
                if matches!(&record.state, ActionState::Failed { status, .. }
                    if status == "origin_consumer_released")),
            "ready capture hid durable terminal: {record:?}"
        );
        assert!(capture.exists());
    }

    #[tokio::test]
    async fn lookup_keeps_ready_captures_present_during_transition() {
        for kind in ["withdraw-ready", "orphan", "dead-input", "origin-released"] {
            let (root, client, key) = fixture();
            std::fs::create_dir(root.path().join("ready-transitions")).unwrap();
            std::fs::rename(
                root.path().join(format!("claimed/{key}.json")),
                root.path()
                    .join(format!("ready-transitions/{key}.100000000.abc.{kind}.json")),
            )
            .unwrap();
            let record = client.read(&key).await;
            assert!(
                matches!(record, Ok(RecordRead::Moving)),
                "ready capture {kind} read as absent or error"
            );
        }
    }
}
