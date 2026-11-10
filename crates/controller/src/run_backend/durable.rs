use super::*;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

pub const MAX_BYTES: usize = 8 * 1024 * 1024;

pub type RecordScan = Vec<(String, Result<Option<Record>>)>;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub request: Request,
    pub intent: SubmissionIntent,
    pub dispatched: bool,
    /// Failed cleanup eligibility, never inferred from launch debt.
    #[serde(default)]
    pub cleanup_retry_ms: Option<u64>,
    /// Proven undispatched intent refused by the runtime debt gate.
    #[serde(default)]
    pub held_submission_ms: Option<u64>,
    pub binding: Option<RunBinding>,
    pub cancel_requested: bool,
    pub events: Vec<Event>,
    pub terminal: Option<Terminal>,
    #[serde(default)]
    pub runtime_ownerships: Vec<RuntimeOwnership>,
    #[serde(default)]
    pub container_launches: std::collections::BTreeMap<String, ContainerLaunchIntent>,
    #[serde(default)]
    pub cleanup_evidence: std::collections::BTreeMap<String, BlobRef>,
    #[serde(default)]
    pub jobspec: Option<BlobRef>,
    #[serde(default)]
    pub allocation_grant: Option<CpuGrant>,
    #[serde(default)]
    pub instance_observation: Option<InstanceObservation>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct InstanceObservation {
    pub broker_starttime: Option<String>,
    pub loaded_config: Option<BlobRef>,
    pub config_sha256: Option<String>,
}

#[derive(Clone)]
pub struct Journal {
    root: PathBuf,
}

impl Journal {
    pub fn new(root: PathBuf) -> Result<Self> {
        if !absolute(&root) {
            return Err(BackendError::invalid("journal root must be absolute"));
        }
        fs::create_dir_all(&root)?;
        Ok(Self {
            root: fs::canonicalize(root)?,
        })
    }

    pub fn transaction<T>(
        &self,
        owner: &Owner,
        f: impl FnOnce(&Path, Option<Record>) -> Result<T>,
    ) -> Result<T> {
        let (dir, lock) = self.owner_lock(owner)?;
        lock.lock()?;
        self.transact_locked(owner, &dir, f)
    }

    /// A busy owner is unobserved, not a failed transaction.
    pub fn try_transaction<T>(
        &self,
        owner: &Owner,
        f: impl FnOnce(&Path, Option<Record>) -> Result<T>,
    ) -> Result<Option<T>> {
        let (dir, lock) = self.owner_lock(owner)?;
        match lock.try_lock() {
            Ok(()) => self.transact_locked(owner, &dir, f).map(Some),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(error)) => Err(error.into()),
        }
    }

    fn owner_lock(&self, owner: &Owner) -> Result<(PathBuf, File)> {
        if !token(&owner.owner) || !token(&owner.correlation) {
            return Err(BackendError::invalid(
                "invalid durable owner or correlation",
            ));
        }
        let dir = self.root.join(&owner.correlation);
        fs::create_dir_all(&dir)?;
        if fs::symlink_metadata(&dir)?.file_type().is_symlink() {
            return Err(BackendError::invalid(
                "journal directory cannot be a symlink",
            ));
        }
        let lock_path = dir.join("lock");
        if lock_path.exists() && fs::symlink_metadata(&lock_path)?.file_type().is_symlink() {
            return Err(BackendError::invalid("journal lock cannot be a symlink"));
        }
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)?;
        Ok((dir, lock))
    }

    fn transact_locked<T>(
        &self,
        owner: &Owner,
        dir: &Path,
        f: impl FnOnce(&Path, Option<Record>) -> Result<T>,
    ) -> Result<T> {
        let path = dir.join("state.json");
        let record: Option<Record> = if path.exists() {
            Some(serde_json::from_slice(&read(&path)?)?)
        } else {
            None
        };
        if record.as_ref().is_some_and(|r| {
            r.intent.owner != owner.owner || r.intent.correlation != owner.correlation
        }) {
            return Err(BackendError {
                code: ErrorCode::Conflict,
                message: "correlation belongs to a different durable owner".into(),
            });
        }
        f(dir, record)
    }

    pub fn runtime_gate<T>(&self, f: impl FnOnce(&Path) -> Result<T>) -> Result<T> {
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.root.join("runtime-lock"))?;
        lock.lock()?;
        let debts = self.root.join("cleanup-debt");
        fs::create_dir_all(&debts)?;
        f(&debts)
    }

    /// Retain each entry's identity. Ok(None) means its lock is busy.
    pub fn records(&self) -> Result<RecordScan> {
        let mut records = Vec::new();
        for (index, entry) in fs::read_dir(&self.root)?.enumerate() {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    records.push((format!("unreadable-entry-{index}"), Err(error.into())));
                    continue;
                }
            };
            let key = entry.file_name().to_string_lossy().into_owned();
            if key == "cleanup-debt" || key == "maintenance-errors" {
                continue;
            }
            let record = (|| {
                let kind = entry.file_type()?;
                if !kind.is_dir() && !kind.is_symlink() {
                    return Ok(None);
                }
                let path = entry.path().join("state.json");
                if !kind.is_symlink() && !path.try_exists()? {
                    return Ok(None);
                }
                if kind.is_symlink() || !token(&key) {
                    return Err(BackendError::invalid("unsafe journal owner directory"));
                }
                let lock_path = entry.path().join("lock");
                if lock_path.is_symlink() {
                    return Err(BackendError::invalid("journal lock cannot be a symlink"));
                }
                let lock = OpenOptions::new()
                    .create(true)
                    .truncate(false)
                    .read(true)
                    .write(true)
                    .open(lock_path)?;
                match lock.try_lock() {
                    Ok(()) => {}
                    Err(std::fs::TryLockError::WouldBlock) => return Ok(Some(None)),
                    Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
                }
                let record: Record = serde_json::from_slice(&read(&path)?)?;
                if record.intent.correlation != key
                    || !token(&record.intent.owner)
                    || !token(&record.intent.correlation)
                {
                    return Err(BackendError::invalid(
                        "journal owner does not match directory",
                    ));
                }
                Ok(Some(Some(record)))
            })();
            match record {
                Ok(Some(record)) => records.push((key, Ok(record))),
                Ok(None) => {}
                Err(error) => records.push((key, Err(error))),
            }
        }
        records.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(records)
    }

    /// A native operation lock is separate from the journal transaction lock.
    pub fn operation_lock(&self, owner: &Owner, name: &str) -> Result<File> {
        if !token(name) {
            return Err(BackendError::invalid("invalid operation lock"));
        }
        self.transaction(owner, |dir, _| {
            let path = dir.join(name);
            if path.exists() && fs::symlink_metadata(&path)?.file_type().is_symlink() {
                return Err(BackendError::invalid("operation lock cannot be a symlink"));
            }
            Ok(OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(path)?)
        })
    }

    pub fn load(&self, owner: &Owner) -> Result<Record> {
        self.transaction(owner, |_, record| {
            record.ok_or_else(|| BackendError::invalid("submission intent not found"))
        })
    }
}

pub fn read(path: &Path) -> Result<Vec<u8>> {
    read_limit(path, MAX_BYTES)
}

/// Protocol-owned bound, used for worker artifacts larger than the control envelope.
pub fn read_limit(path: &Path, limit: usize) -> Result<Vec<u8>> {
    if fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(BackendError::invalid("evidence cannot be a symlink"));
    }
    let file = File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(BackendError::invalid("evidence must be a regular file"));
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(BackendError::invalid("evidence exceeds byte bound"));
    }
    Ok(bytes)
}

pub fn atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let parent = path
        .parent()
        .ok_or_else(|| BackendError::invalid("durable path requires parent"))?;
    let temporary = parent.join(format!(
        ".fx-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&temporary, path)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

pub fn save(dir: &Path, record: &Record) -> Result<()> {
    let bytes = serde_json::to_vec(record)?;
    if bytes.len() > MAX_BYTES {
        return Err(BackendError::unknown(
            "durable journal byte bound reached; retain last valid observation",
        ));
    }
    atomic(&dir.join("state.json"), &bytes)
}

pub fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

pub fn blob(path: &Path, bytes: &[u8]) -> Result<BlobRef> {
    atomic(path, bytes)?;
    Ok(BlobRef {
        sha256: digest(bytes),
        bytes: bytes.len() as u64,
        path: path.to_owned(),
    })
}

pub fn checked_blob(reference: &BlobRef, root: &Path) -> Result<Vec<u8>> {
    checked_blob_limit(reference, root, MAX_BYTES)
}

pub fn checked_blob_limit(reference: &BlobRef, root: &Path, limit: usize) -> Result<Vec<u8>> {
    let canonical = fs::canonicalize(&reference.path)?;
    if !canonical.starts_with(fs::canonicalize(root)?) || !hex(&reference.sha256, 64) {
        return Err(BackendError::invalid("output outside owned directory"));
    }
    let bytes = read_limit(&canonical, limit)?;
    if bytes.len() as u64 != reference.bytes || digest(&bytes) != reference.sha256 {
        return Err(BackendError::invalid("output digest mismatch"));
    }
    Ok(bytes)
}

pub fn safe(text: &str) -> Result<String> {
    safe_limit(text, 16_384, 16_384)
}

pub(crate) fn safe_report(text: &str) -> Result<String> {
    let chars = crate::contracts::report::PROOF_MESSAGE_CHARS;
    safe_limit(text, chars, chars * 4)
}

fn safe_limit(text: &str, chars: usize, bytes: usize) -> Result<String> {
    let redacted =
        crate::redaction::text(text).map_err(|_| BackendError::invalid("redaction unavailable"))?;
    let mut text: String = redacted
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .take(chars)
        .collect();
    while text.len() > bytes {
        text.pop();
    }
    Ok(text)
}

/// Kept outside owner state so malformed state cannot erase its failure or retry deadline.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MaintenanceFailure {
    pub key: String,
    pub message: String,
    pub next_attempt_ms: u64,
}

pub fn maintenance_failure(
    root: &Path,
    key: &str,
    message: &str,
    next_attempt_ms: u64,
) -> Result<()> {
    let dir = root.join("maintenance-errors");
    fs::create_dir_all(&dir)?;
    if dir.is_symlink() {
        return Err(BackendError::invalid(
            "maintenance error directory cannot be a symlink",
        ));
    }
    let failure = MaintenanceFailure {
        key: key.into(),
        message: safe(message)?,
        next_attempt_ms,
    };
    atomic(
        &dir.join(format!("{}.json", digest(key.as_bytes()))),
        &serde_json::to_vec(&failure)?,
    )
}

pub fn maintenance_failures(root: &Path) -> Result<Vec<MaintenanceFailure>> {
    let dir = root.join("maintenance-errors");
    if !dir.try_exists()? {
        return Ok(vec![]);
    }
    if dir.is_symlink() {
        return Err(BackendError::invalid(
            "maintenance error directory cannot be a symlink",
        ));
    }
    let mut failures = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            failures.push(serde_json::from_slice(&read(&path)?)?);
        }
    }
    Ok(failures)
}

pub fn clear_maintenance_failure(root: &Path, key: &str) -> Result<()> {
    let dir = root.join("maintenance-errors");
    let path = dir.join(format!("{}.json", digest(key.as_bytes())));
    match fs::remove_file(path) {
        Ok(()) => {
            File::open(dir)?.sync_all()?;
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}
