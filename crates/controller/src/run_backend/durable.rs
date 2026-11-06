use super::*;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

pub const MAX_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub request: Request,
    pub intent: SubmissionIntent,
    pub dispatched: bool,
    pub binding: Option<RunBinding>,
    pub cancel_requested: bool,
    pub events: Vec<Event>,
    pub terminal: Option<Terminal>,
    #[serde(default)]
    pub runtime_ownership: Option<RuntimeOwnership>,
    #[serde(default)]
    pub cleanup_evidence: Option<BlobRef>,
    #[serde(default)]
    pub jobspec: Option<BlobRef>,
    #[serde(default)]
    pub allocation_grant: Option<CpuGrant>,
}

#[derive(Clone)]
pub struct Journal {
    root: PathBuf,
}

impl Journal {
    pub fn new(root: PathBuf) -> Result<Self> {
        if !absolute(&root) { return Err(BackendError::invalid("journal root must be absolute")); }
        fs::create_dir_all(&root)?;
        Ok(Self { root: fs::canonicalize(root)? })
    }

    pub fn transaction<T>(&self, owner: &Owner, f: impl FnOnce(&Path, Option<Record>) -> Result<T>) -> Result<T> {
        if !token(&owner.owner) || !token(&owner.correlation) {
            return Err(BackendError::invalid("invalid durable owner or correlation"));
        }
        let dir = self.root.join(&owner.correlation);
        fs::create_dir_all(&dir)?;
        if fs::symlink_metadata(&dir)?.file_type().is_symlink() {
            return Err(BackendError::invalid("journal directory cannot be a symlink"));
        }
        let lock_path = dir.join("lock");
        if lock_path.exists() && fs::symlink_metadata(&lock_path)?.file_type().is_symlink() {
            return Err(BackendError::invalid("journal lock cannot be a symlink"));
        }
        let lock = OpenOptions::new().create(true).truncate(false).read(true).write(true).open(lock_path)?;
        lock.lock()?;
        let path = dir.join("state.json");
        let record: Option<Record> = if path.exists() {
            Some(serde_json::from_slice(&read(&path)?)?)
        } else { None };
        if record.as_ref().is_some_and(|r| r.intent.owner != owner.owner || r.intent.correlation != owner.correlation) {
            return Err(BackendError { code: ErrorCode::Conflict, message: "correlation belongs to a different durable owner".into() });
        }
        f(&dir, record)
    }

    pub fn runtime_gate<T>(&self, f: impl FnOnce(&Path) -> Result<T>) -> Result<T> {
        let lock = OpenOptions::new().create(true).truncate(false).read(true).write(true).open(self.root.join("runtime-lock"))?;
        lock.lock()?;
        let debts = self.root.join("cleanup-debt");
        fs::create_dir_all(&debts)?;
        f(&debts)
    }

    pub fn load(&self, owner: &Owner) -> Result<Record> {
        self.transaction(owner, |_, record| record.ok_or_else(|| BackendError::invalid("submission intent not found")))
    }
}

pub fn read(path: &Path) -> Result<Vec<u8>> {
    if fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(BackendError::invalid("evidence cannot be a symlink"));
    }
    let file = File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(BackendError::invalid("evidence must be a regular file"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_BYTES { return Err(BackendError::invalid("evidence exceeds byte bound")); }
    Ok(bytes)
}

pub fn atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let parent = path.parent().ok_or_else(|| BackendError::invalid("durable path requires parent"))?;
    let temporary = parent.join(format!(".fx-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let mut file = OpenOptions::new().write(true).create_new(true).open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&temporary, path)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

pub fn save(dir: &Path, record: &Record) -> Result<()> {
    atomic(&dir.join("state.json"), &serde_json::to_vec(record)?)
}

pub fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

pub fn blob(path: &Path, bytes: &[u8]) -> Result<BlobRef> {
    atomic(path, bytes)?;
    Ok(BlobRef { sha256: digest(bytes), bytes: bytes.len() as u64, path: path.to_owned() })
}

pub fn checked_blob(reference: &BlobRef, root: &Path) -> Result<Vec<u8>> {
    let canonical = fs::canonicalize(&reference.path)?;
    if !canonical.starts_with(fs::canonicalize(root)?) || !hex(&reference.sha256, 64) {
        return Err(BackendError::invalid("output outside owned directory"));
    }
    let bytes = read(&canonical)?;
    if bytes.len() as u64 != reference.bytes || digest(&bytes) != reference.sha256 {
        return Err(BackendError::invalid("output digest mismatch"));
    }
    Ok(bytes)
}

pub fn safe(text: &str) -> Result<String> {
    let redacted = crate::redaction::text(text)
        .map_err(|_| BackendError::invalid("redaction unavailable"))?;
    let mut text: String = redacted.chars().filter(|c| !c.is_control() || *c == '\n' || *c == '\t').take(16_384).collect();
    while text.len() > 16_384 { text.pop(); }
    Ok(text)
}
