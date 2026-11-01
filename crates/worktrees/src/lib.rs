//! Git operations for a repository owned by one controller process.
//!
//! All handles for the same common Git directory share an in-process mutex.
//! Other processes must not mutate the repository while the controller uses it.
//! No OS file locks are used because of the deployment kernel's file-lock bug.
#![forbid(unsafe_code)]

pub mod branch_template;
mod queries;
mod snapshots;

use base64::{Engine, engine::general_purpose::STANDARD};
use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    fs,
    io::Write,
    os::unix::ffi::OsStringExt,
    path::{Component, Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::{Arc, Mutex, OnceLock, Weak},
};
use url::Url;

enum GitEnvironment<'a> {
    Default,
    Snapshot,
    Index(&'a Path),
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid branch name")]
    BranchName,
    #[error("invalid commit id")]
    CommitId,
    #[error("invalid remote; use credential-free GitHub HTTPS or an absolute local repository")]
    Remote,
    #[error("worktree destination must be beneath the configured root")]
    WorktreePath,
    #[error("repository operation was interrupted by a panic")]
    Poisoned,
    #[error("invalid installation token")]
    Token,
    #[error("filesystem operation failed")]
    Io,
    #[error("could not execute git")]
    Spawn,
    #[error("git {operation} failed with exit code {code:?}")]
    Git {
        operation: &'static str,
        code: Option<i32>,
    },
    #[error("git returned an unexpected response")]
    Response,
    #[error("branch or base is missing")]
    MissingBranch,
    #[error("main is reserved")]
    MainBranch,
    #[error("worktree path or branch belongs to another checkout")]
    WorktreeOccupied,
    #[error("worktree has uncommitted or untracked files")]
    DirtyWorktree,
    #[error("refusing to publish a checked head that conflicts with current main")]
    UnrebasedHead,
    #[error("could not abort interrupted rebase; recovery is required")]
    Recovery,
}

impl Error {
    pub fn permanent_class(&self) -> Option<&'static str> {
        match self {
            Self::BranchName => Some("invalid_branch"),
            Self::CommitId => Some("invalid_commit"),
            Self::Remote => Some("invalid_remote"),
            Self::WorktreePath => Some("invalid_worktree_path"),
            Self::Token => Some("invalid_token"),
            Self::Response => Some("invalid_response"),
            Self::MissingBranch => Some("missing_branch"),
            Self::MainBranch => Some("reserved_branch"),
            Self::WorktreeOccupied => Some("worktree_occupied"),
            Self::DirtyWorktree => Some("dirty_worktree"),
            Self::Recovery => Some("worktree_recovery"),
            Self::UnrebasedHead => Some("unrebased_head"),
            // A Git exit alone cannot distinguish a lock or network failure.
            Self::Io | Self::Spawn | Self::Poisoned | Self::Git { .. } => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchName(String);

impl BranchName {
    pub fn new(value: impl Into<String>) -> Result<Self, Error> {
        let value = value.into();
        if value.is_empty()
            || value == "HEAD"
            || value == "@"
            || value.starts_with('-')
            || value.contains("..")
            || value.contains("@{")
            || value.ends_with('.')
            || value
                .bytes()
                .any(|b| b <= b' ' || b == 127 || b"~^:?*[\\".contains(&b))
            || value
                .split('/')
                .any(|part| part.is_empty() || part.starts_with('.') || part.ends_with(".lock"))
        {
            return Err(Error::BranchName);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn reference(&self) -> String {
        format!("refs/heads/{}", self.0)
    }

    fn base_ref(&self) -> String {
        format!("refs/tl3/bases/{}", self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(transparent)]
pub struct CommitId(String);

impl std::fmt::Display for CommitId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for CommitId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = <String as serde::Deserialize>::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

impl CommitId {
    pub fn new(value: impl Into<String>) -> Result<Self, Error> {
        let value = value.into();
        if !matches!(value.len(), 40 | 64)
            || !value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::CommitId);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlobId(CommitId);

impl BlobId {
    pub fn new(value: impl Into<String>) -> Result<Self, Error> {
        CommitId::new(value).map(Self)
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branch {
    pub name: BranchName,
    pub base: CommitId,
}

// Credentials deliberately implement neither Debug nor Serialize.
pub struct InstallationToken(String);

impl InstallationToken {
    pub fn new(value: impl Into<String>) -> Result<Self, Error> {
        let value = value.into();
        if value.is_empty() || !value.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(Error::Token);
        }
        Ok(Self(value))
    }
}

pub struct Remote(OsString);

impl Remote {
    fn credentials<'a>(
        &self,
        token: Option<&'a InstallationToken>,
    ) -> Option<&'a InstallationToken> {
        if self.0.to_string_lossy().starts_with("https://github.com/") {
            token
        } else {
            None
        }
    }

    pub fn https(value: &str) -> Result<Self, Error> {
        let url = Url::parse(value).map_err(|_| Error::Remote)?;
        if url.scheme() != "https"
            || url.host_str() != Some("github.com")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(Error::Remote);
        }
        Ok(Self(url.as_str().into()))
    }

    pub fn local(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = fs::canonicalize(path).map_err(|_| Error::Remote)?;
        Ok(Self(
            Url::from_file_path(path)
                .map_err(|_| Error::Remote)?
                .as_str()
                .into(),
        ))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum RebaseResult {
    Clean { base: CommitId, head: CommitId },
    Conflicted { head: CommitId, files: Vec<PathBuf> },
}

pub struct Worktrees {
    repo: PathBuf,
    common: PathBuf,
    root: PathBuf,
    mutex: Arc<Mutex<()>>,
    #[cfg(test)]
    invocations: Mutex<Vec<Invocation>>,
}

struct Checkout {
    path: PathBuf,
    branch: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopeTrim {
    pub head: CommitId,
    pub removed_paths: Vec<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrimBoundary {
    Recorded,
    CheckoutRestored,
    HeadUpdated,
}

impl Worktrees {
    pub fn create_worktree(
        &self,
        name: &BranchName,
        path: impl AsRef<Path>,
    ) -> Result<PathBuf, Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        self.create_worktree_locked(name, path)
    }

    /// Refuses dirty checkouts. It never discards a maker's uncommitted files.
    pub fn remove_worktree(&self, name: &BranchName, path: impl AsRef<Path>) -> Result<(), Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        self.remove_worktree_locked(name, path, false)
    }

    /// Called before recording a Remove intent. No deletion has begun yet.
    pub fn verify_removal_clean(&self, name: &BranchName, path: &Path) -> Result<(), Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        let path = self.destination(path)?;
        if path.exists() && fs::read_dir(&path).map_err(|_| Error::Io)?.next().is_some() {
            self.verify_checkout(&path, name)?;
            if self.initializing_at_start(name, &path)? {
                return Ok(());
            }
            self.require_clean(&path)?;
        }
        Ok(())
    }

    /// Save teardown scratch without changing the live index or working files.
    /// The second parent retains staged content even when the working copy differs.
    pub fn save_scratch(
        &self,
        name: &BranchName,
        path: &Path,
        generation: u64,
    ) -> Result<Option<String>, Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        leaf(name)?;
        let path = self.destination(path)?;
        let reference = format!("refs/tl3/scratch/{}/{generation}", name.as_str());
        if self.resolve_optional(&reference)?.is_some() {
            return Ok(Some(reference));
        }
        self.verify_checkout(&path, name)?;
        match self.require_clean(&path) {
            Ok(()) => return Ok(None),
            Err(Error::DirtyWorktree) => {}
            Err(error) => return Err(error),
        }
        let head = self.resolve(&name.reference())?;
        let temporary = tempfile::tempdir_in(&self.common).map_err(|_| Error::Io)?;
        let index = temporary.path().join("index");
        let live_index = path_line(
            &self
                .run(
                    &path,
                    "scratch index",
                    ["rev-parse", "--path-format=absolute", "--git-path", "index"],
                )?
                .stdout,
        )?;
        if live_index.exists() {
            fs::copy(live_index, &index).map_err(|_| Error::Io)?;
        } else {
            success(
                "scratch read tree",
                self.execute_index(&path, ["read-tree", "HEAD"], None, None, Some(&index))?,
            )?;
        }
        // A scratch tree cannot contain index stages. Preserve stage zero and the
        // leaf side of each conflict in the private index; never change the live index.
        let unmerged = success(
            "scratch conflicts",
            self.execute_index(
                &path,
                ["ls-files", "--unmerged", "-z"],
                None,
                None,
                Some(&index),
            )?,
        )?;
        let mut entries = HashMap::<Vec<u8>, Option<Vec<u8>>>::new();
        for entry in unmerged
            .stdout
            .split(|byte| *byte == 0)
            .filter(|entry| !entry.is_empty())
        {
            let tab = entry
                .iter()
                .position(|byte| *byte == b'\t')
                .ok_or(Error::Response)?;
            let metadata = &entry[..tab];
            let path = entry[tab + 1..].to_vec();
            let side = entries.entry(path).or_default();
            if metadata.ends_with(b" 2") {
                *side = Some(metadata[..metadata.len() - 2].to_vec());
            }
        }
        if !entries.is_empty() {
            let mut instructions = Vec::new();
            for (path, side) in entries {
                instructions.extend_from_slice(
                    format!("0 {}\t", "0".repeat(head.as_str().len())).as_bytes(),
                );
                instructions.extend_from_slice(&path);
                instructions.push(0);
                if let Some(side) = side {
                    instructions.extend_from_slice(&side);
                    instructions.push(b'\t');
                    instructions.extend_from_slice(&path);
                    instructions.push(0);
                }
            }
            success(
                "save conflicted index",
                self.execute_index(
                    &path,
                    ["update-index", "-z", "--index-info"],
                    None,
                    Some(&instructions),
                    Some(&index),
                )?,
            )?;
        }
        let merge = self.execute(&path, ["rev-parse", "--verify", "MERGE_HEAD"], None, None)?;
        let merge = if merge.status.success() {
            Some(commit_line(&merge.stdout)?)
        } else {
            None
        };
        let staged_tree = self.scratch_git(&path, &index, ["write-tree"])?;
        let staged = self.scratch_git(
            &path,
            &index,
            [
                "commit-tree",
                staged_tree.as_str(),
                "-p",
                merge.as_ref().unwrap_or(&head).as_str(),
                "-m",
                "Saved maker index",
            ],
        )?;
        self.scratch_git(&path, &index, ["add", "-A"])?;
        let tree = self.scratch_git(&path, &index, ["write-tree"])?;
        let commit = self.scratch_git(
            &path,
            &index,
            [
                "commit-tree",
                tree.as_str(),
                "-p",
                head.as_str(),
                "-p",
                staged.as_str(),
                "-m",
                "Saved maker scratch",
            ],
        )?;
        // Objects and the owning ref must reach disk before cleanup is authorized.
        self.run(
            &self.repo,
            "save scratch ref",
            [
                "-c",
                "core.fsync=committed",
                "-c",
                "core.fsyncMethod=fsync",
                "update-ref",
                &reference,
                &commit,
                &"0".repeat(head.as_str().len()),
            ],
        )?;
        Ok(Some(reference))
    }

    fn scratch_git<const N: usize>(
        &self,
        path: &Path,
        index: &Path,
        args: [&str; N],
    ) -> Result<String, Error> {
        let args = ["-c", "core.fsync=committed", "-c", "core.fsyncMethod=fsync"]
            .into_iter()
            .chain(args);
        let output = success(
            "save scratch",
            self.execute_index(path, args, None, None, Some(index))?,
        )?;
        Ok(String::from_utf8(output.stdout)
            .map_err(|_| Error::Response)?
            .trim()
            .into())
    }

    /// Only a durable scratch ref authorizes resetting a maker's unfinished edit.
    pub fn clean_saved_scratch(
        &self,
        name: &BranchName,
        path: &Path,
        reference: &str,
    ) -> Result<(), Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        leaf(name)?;
        let path = self.destination(path)?;
        if !reference.starts_with(&format!("refs/tl3/scratch/{}/", name.as_str()))
            || self.resolve_optional(reference)?.is_none()
        {
            return Err(Error::Recovery);
        }
        if path.join(".git").exists() {
            self.verify_checkout(&path, name)?;
            let lock = path_line(
                &self
                    .run(
                        &path,
                        "removal lock",
                        [
                            "rev-parse",
                            "--path-format=absolute",
                            "--git-path",
                            "locked",
                        ],
                    )?
                    .stdout,
            )?;
            if lock.exists() && !self.initializing_at_start(name, &path)? {
                return Err(Error::WorktreeOccupied);
            }
            self.run(&path, "clean saved scratch", ["reset", "--hard", "HEAD"])?;
            self.run(&path, "clean saved untracked files", ["clean", "-fd"])?;
            self.require_clean(&path)?;
        }
        Ok(())
    }

    /// Discover expiry candidates before the caller journals their removal.
    pub fn expired_scratch(&self, now_secs: u64) -> Result<Vec<(String, CommitId)>, Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        let output = self.run(
            &self.repo,
            "list saved scratch",
            [
                "for-each-ref",
                "--format=%(refname) %(objectname) %(committerdate:unix)",
                "refs/tl3/scratch/",
            ],
        )?;
        let text = std::str::from_utf8(&output.stdout).map_err(|_| Error::Response)?;
        let mut expired = Vec::new();
        for line in text.lines() {
            let mut fields = line.split_whitespace();
            let reference = fields.next().ok_or(Error::Response)?;
            let head = fields.next().ok_or(Error::Response)?;
            let saved: u64 = fields
                .next()
                .ok_or(Error::Response)?
                .parse()
                .map_err(|_| Error::Response)?;
            if now_secs.saturating_sub(saved) >= 30 * 24 * 60 * 60 {
                expired.push((reference.to_owned(), CommitId::new(head)?));
            }
        }
        Ok(expired)
    }

    /// A missing ref confirms an earlier deletion when replaying a journalled prune.
    pub fn prune_scratch(&self, reference: &str, expected: &CommitId) -> Result<(), Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        if !reference.starts_with("refs/tl3/scratch/") {
            return Err(Error::Recovery);
        }
        if self.resolve_optional(reference)?.is_some() {
            self.run(
                &self.repo,
                "expire saved scratch",
                ["update-ref", "-d", reference, expected.as_str()],
            )?;
        }
        Ok(())
    }

    fn initializing_at_start(&self, name: &BranchName, path: &Path) -> Result<bool, Error> {
        if !self.initializing(path)? {
            return Ok(false);
        }
        let Some(base) = self
            .resolve_optional(&format!("refs/tl3/starts/{}", name.as_str()))?
            .or(self.resolve_optional(&name.base_ref())?)
        else {
            return Ok(false);
        };
        Ok(self.resolve_optional(&name.reference())?.as_ref() == Some(&base))
    }

    /// Requires a durable Remove intent proving cleanliness before deletion began.
    pub fn resume_removal(&self, name: &BranchName, path: &Path) -> Result<(), Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        self.remove_worktree_locked(name, path, true)
    }

    /// Git's initializing lock and unchanged start authorize interrupted checkout repair.
    pub fn finish_prepare(
        &self,
        name: &BranchName,
        path: &Path,
        writable: bool,
    ) -> Result<bool, Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        if !path.exists() {
            return Ok(false);
        }
        self.verify_checkout(path, name)?;
        if !self.initializing(path)? {
            return Ok(true);
        }
        if !writable {
            return Ok(false);
        }
        if !self.initializing_at_start(name, path)? {
            return Err(Error::Recovery);
        }
        // Repository ownership guarantees the killed controller's Git children have ended.
        let lock = path_line(
            &self
                .run(
                    path,
                    "prepare index lock",
                    [
                        "rev-parse",
                        "--path-format=absolute",
                        "--git-path",
                        "index.lock",
                    ],
                )?
                .stdout,
        )?;
        match fs::remove_file(lock) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(Error::Io),
        }
        self.run(
            path,
            "populate prepared checkout",
            ["reset", "--hard", &name.reference()],
        )?;
        self.require_clean(path)?;
        self.run(
            &self.repo,
            "unlock prepared checkout",
            [
                OsStr::new("worktree"),
                OsStr::new("unlock"),
                path.as_os_str(),
            ],
        )?;
        Ok(true)
    }

    fn initializing(&self, path: &Path) -> Result<bool, Error> {
        let locked = path_line(
            &self
                .run(
                    path,
                    "prepare registration lock",
                    [
                        "rev-parse",
                        "--path-format=absolute",
                        "--git-path",
                        "locked",
                    ],
                )?
                .stdout,
        )?;
        match fs::read(locked) {
            // Git translates this reason. execute sets LC_ALL=C for every Git child.
            Ok(reason) => Ok(reason == b"initializing\n"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(_) => Err(Error::Io),
        }
    }

    pub fn delete_local_branch(&self, name: &BranchName) -> Result<(), Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        self.delete_local_branch_locked(name)
    }

    pub fn remote_head(
        &self,
        name: &BranchName,
        remote: &Remote,
        token: Option<&InstallationToken>,
    ) -> Result<Option<CommitId>, Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        self.remote_head_locked(name, remote, token)
    }

    /// Creates both refs at the operation's durably recorded base.
    pub fn create_branch_at(&self, name: &BranchName, base: &CommitId) -> Result<Branch, Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        leaf(name)?;
        if self.resolve_optional(&name.reference())?.is_none()
            && self.resolve_optional(&name.base_ref())?.is_none()
        {
            self.update_refs(&format!(
                "create {} {}\ncreate {} {}\n",
                name.reference(),
                base.as_str(),
                name.base_ref(),
                base.as_str()
            ))?;
        }
        self.create_branch_locked(name)
    }

    pub fn recover_rebase(&self, name: &BranchName, path: &Path) -> Result<(), Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        self.verify_repository(path)?;
        let marker = self.rebase_marker(path)?;
        if marker.exists() {
            self.restore_rebase_checkout(path, name, &marker)?;
        }
        self.verify_checkout(path, name)?;
        self.require_clean(path)
    }

    pub fn rebase_complete(&self, name: &BranchName, path: &Path) -> Result<bool, Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        if !path.exists() {
            return Ok(false);
        }
        self.verify_repository(path)?;
        if self.rebase_marker(path)?.exists() || self.rebase_active(path)? {
            return Ok(false);
        }
        self.verify_checkout(path, name)?;
        Ok(self.status(path)?.is_empty())
    }

    /// Deletes only the head recorded by the teardown effect, never a newer push.
    pub fn delete_remote_branch_at(
        &self,
        name: &BranchName,
        remote: &Remote,
        token: Option<&InstallationToken>,
        expected: &CommitId,
    ) -> Result<(), Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        leaf(name)?;
        let token = remote.credentials(token);
        if self.remote_head_locked(name, remote, token)?.is_none() {
            return Ok(());
        }
        let reference = name.reference();
        let lease = format!("--force-with-lease={reference}:{}", expected.as_str());
        self.network(
            "delete recorded remote branch",
            [
                OsStr::new("push"),
                OsStr::new("--porcelain"),
                OsStr::new(&lease),
                OsStr::new("--"),
                &remote.0,
                OsStr::new(&format!(":{reference}")),
            ],
            token,
        )?;
        Ok(())
    }

    pub fn common_directory(&self) -> &Path {
        &self.common
    }

    pub fn branch_state(&self, name: &BranchName) -> Result<Option<(CommitId, CommitId)>, Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        match (
            self.resolve_optional(&name.reference())?,
            self.resolve_optional(&name.base_ref())?,
        ) {
            (Some(head), Some(base)) => Ok(Some((head, base))),
            (None, None) => Ok(None),
            _ => Err(Error::Recovery),
        }
    }

    pub fn checkout_exists(&self, name: &BranchName, path: &Path) -> Result<bool, Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        if !path.exists() {
            return Ok(false);
        }
        let path = self.destination(path)?;
        if !self.checkouts()?.iter().any(|checkout| {
            checkout.path == path && checkout.branch.as_deref() == Some(&name.reference())
        }) {
            return Err(Error::WorktreeOccupied);
        }
        self.verify_checkout(&path, name)?;
        Ok(true)
    }

    pub fn has_pull_head(&self, number: u64) -> Result<bool, Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        Ok(self
            .resolve_optional(&format!("refs/tl3/pulls/{number}"))?
            .is_some())
    }

    pub fn delete_pull_head(&self, number: u64) -> Result<(), Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        self.run(
            &self.repo,
            "release pull head",
            ["update-ref", "-d", &format!("refs/tl3/pulls/{number}")],
        )?;
        Ok(())
    }

    /// Fetch the base repository's pull ref and retain the label-time commit.
    pub fn fetch_pull_head(
        &self,
        remote: &Remote,
        token: Option<&InstallationToken>,
        number: u64,
        pinned: &CommitId,
    ) -> Result<CommitId, Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        if number == 0 {
            return Err(Error::Response);
        }
        let reference = format!("refs/tl3/pulls/{number}");
        if self.resolve_optional(&reference)?.is_some()
            && self.resolve_optional(pinned.as_str())?.is_some()
        {
            return Ok(pinned.clone());
        }
        self.network(
            "fetch pull head",
            [
                OsStr::new("fetch"),
                OsStr::new("--no-tags"),
                OsStr::new("--no-auto-maintenance"),
                OsStr::new("--no-write-fetch-head"),
                OsStr::new("--"),
                &remote.0,
                OsStr::new(&format!("+refs/pull/{number}/head:{reference}")),
            ],
            remote.credentials(token),
        )?;
        if self.resolve_optional(pinned.as_str())?.is_none() {
            // A force-push can remove the label-time head from the current pull history.
            self.network(
                "fetch pinned pull commit",
                [
                    OsStr::new("fetch"),
                    OsStr::new("--no-tags"),
                    OsStr::new("--no-auto-maintenance"),
                    OsStr::new("--no-write-fetch-head"),
                    OsStr::new("--"),
                    &remote.0,
                    OsStr::new(pinned.as_str()),
                ],
                remote.credentials(token),
            )?;
        }
        self.resolve(pinned.as_str())?;
        self.update_refs(&format!("update {reference} {}\n", pinned.as_str()))?;
        Ok(pinned.clone())
    }

    /// Atomically record a new branch, its comparison base, and its author start.
    pub fn create_branch_from(
        &self,
        name: &BranchName,
        base: &CommitId,
        start: &CommitId,
    ) -> Result<Branch, Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        leaf(name)?;
        let start_ref = format!("refs/tl3/starts/{}", name.as_str());
        match (
            self.resolve_optional(&name.reference())?,
            self.resolve_optional(&name.base_ref())?,
            self.resolve_optional(&start_ref)?,
        ) {
            (None, None, None) => self.update_refs(&format!(
                "create {} {}\ncreate {} {}\ncreate {start_ref} {}\n",
                name.reference(),
                start.as_str(),
                name.base_ref(),
                base.as_str(),
                start.as_str()
            ))?,
            (Some(_), Some(_), Some(existing)) if existing == *start => {}
            _ => return Err(Error::Recovery),
        }
        self.create_branch_locked(name)
    }

    /// Fetches only main, with the same credential and transport policy as pushes.
    pub fn fetch_main(
        &self,
        remote: &Remote,
        token: Option<&InstallationToken>,
    ) -> Result<CommitId, Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        self.network(
            "fetch main",
            [
                OsStr::new("fetch"),
                OsStr::new("--no-tags"),
                OsStr::new("--no-auto-maintenance"),
                OsStr::new("--no-write-fetch-head"),
                OsStr::new("--"),
                &remote.0,
                OsStr::new("+refs/heads/main:refs/tl3/upstream-main"),
            ],
            remote.credentials(token),
        )?;
        let main = self.resolve("refs/tl3/upstream-main")?;
        Ok(main)
    }

    /// Uses the effect's immutable head and the last head this controller published.
    pub fn publish(
        &self,
        name: &BranchName,
        remote: &Remote,
        token: Option<&InstallationToken>,
        head: &CommitId,
        expected: Option<&CommitId>,
    ) -> Result<(), Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        leaf(name)?;
        let token = remote.credentials(token);
        let actual = self.remote_head_locked(name, remote, token)?;
        if actual.as_ref() == Some(head) {
            return Ok(());
        }
        if actual.as_ref() != expected {
            return Err(Error::Git {
                operation: "remote branch changed",
                code: None,
            });
        }
        let reference = name.reference();
        let lease = format!(
            "--force-with-lease={reference}:{}",
            expected.map_or("", CommitId::as_str)
        );
        self.network(
            "publish head",
            [
                OsStr::new("push"),
                OsStr::new("--porcelain"),
                OsStr::new(&lease),
                OsStr::new("--"),
                &remote.0,
                OsStr::new(&format!("{}:{reference}", head.as_str())),
            ],
            token,
        )?;
        Ok(())
    }

    /// Replace an unfinished merge when a newer rebase effect records another main.
    pub fn merge_resolution(
        &self,
        name: &BranchName,
        path: &Path,
        target: &CommitId,
        writable: bool,
    ) -> Result<Option<Vec<PathBuf>>, Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        self.verify_repository(path)?;
        self.merge_resolution_locked(name, path, target, writable)
    }

    fn merge_resolution_locked(
        &self,
        name: &BranchName,
        path: &Path,
        target: &CommitId,
        writable: bool,
    ) -> Result<Option<Vec<PathBuf>>, Error> {
        let output = self.run(
            path,
            "merge state path",
            [
                "rev-parse",
                "--path-format=absolute",
                "--git-path",
                "MERGE_HEAD",
            ],
        )?;
        let marker = path_line(&output.stdout)?;
        let head = match fs::read_to_string(marker) {
            Ok(head) => head,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(Error::Io),
        };
        self.verify_checkout(path, name)?;
        if head.trim() != target.as_str() {
            if writable {
                self.run(path, "abort superseded merge", ["merge", "--abort"])?;
            }
            return Ok(None);
        }
        let output = self.run(
            path,
            "merge conflicts",
            ["diff", "--name-only", "--diff-filter=U", "-z", "--"],
        )?;
        Ok(Some(paths(&output.stdout)))
    }

    /// Leave both sides in the index and conflict markers in the maker's checkout.
    /// The maker's merge commit, rather than a commit on the old base, completes integration.
    pub fn prepare_merge_resolution(
        &self,
        name: &BranchName,
        path: &Path,
        target: &CommitId,
    ) -> Result<(), Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        self.verify_checkout(path, name)?;
        if self
            .merge_resolution_locked(name, path, target, true)?
            .is_none()
        {
            self.require_clean(path)?;
            let output = self.execute(
                path,
                [
                    "merge",
                    "--no-commit",
                    "--no-ff",
                    "--no-autostash",
                    target.as_str(),
                ],
                None,
                None,
            )?;
            if self
                .merge_resolution_locked(name, path, target, true)?
                .is_none()
            {
                return Err(git_error("prepare conflict resolution", &output));
            }
        }
        let base = self.create_branch_locked(name)?.base;
        if base != *target {
            self.update_refs(&format!(
                "update {} {} {}\n",
                name.base_ref(),
                target.as_str(),
                base.as_str()
            ))?;
        }
        Ok(())
    }

    pub fn rebase_onto(
        &self,
        name: &BranchName,
        path: &Path,
        target: &CommitId,
    ) -> Result<RebaseResult, Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        self.rebase_onto_locked(name, path, target)
    }

    pub fn changed_files(&self, base: &CommitId, head: &CommitId) -> Result<Vec<PathBuf>, Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        self.changed_files_locked(base, head)
    }

    /// The configured worktrees root must already exist.
    pub fn open(repo: impl AsRef<Path>, root: impl AsRef<Path>) -> Result<Self, Error> {
        let root = fs::canonicalize(root).map_err(|_| Error::Io)?;
        let repo = fs::canonicalize(repo).map_err(|_| Error::Io)?;
        let mut this = Self {
            common: repo.clone(),
            root,
            mutex: Arc::new(Mutex::new(())),
            repo,
            #[cfg(test)]
            invocations: Default::default(),
        };
        let output = this.run(
            &this.repo,
            "open",
            ["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )?;
        this.common = fs::canonicalize(path_line(&output.stdout)?).map_err(|_| Error::Io)?;
        type RepoMutexes = Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>;
        static MUTEXES: OnceLock<RepoMutexes> = OnceLock::new();
        let mut mutexes = MUTEXES
            .get_or_init(Mutex::default)
            .lock()
            .map_err(|_| Error::Poisoned)?;
        mutexes.retain(|_, mutex| mutex.strong_count() > 0);
        this.mutex = mutexes
            .entry(this.common.clone())
            .or_default()
            .upgrade()
            .unwrap_or_else(|| {
                let mutex = Arc::new(Mutex::new(()));
                mutexes.insert(this.common.clone(), Arc::downgrade(&mutex));
                mutex
            });
        Ok(this)
    }

    fn create_branch_locked(&self, name: &BranchName) -> Result<Branch, Error> {
        leaf(name)?;
        let reference = name.reference();
        let base_ref = name.base_ref();
        if let Some(base) = self.resolve_optional(&base_ref)? {
            if self.resolve_optional(&reference)?.is_none() {
                self.update_refs(&format!("create {reference} {}\n", base.as_str()))?;
            }
            return Ok(Branch {
                name: name.clone(),
                base,
            });
        }
        let main = self.resolve("refs/heads/main")?;
        let (base, update) = if self.resolve_optional(&reference)?.is_some() {
            let output = self.run(
                &self.repo,
                "merge-base",
                ["merge-base", "refs/heads/main", &reference],
            )?;
            let base = commit_line(&output.stdout)?;
            (
                base.clone(),
                format!("create {base_ref} {}\n", base.as_str()),
            )
        } else {
            (
                main.clone(),
                format!(
                    "create {reference} {}\ncreate {base_ref} {}\n",
                    main.as_str(),
                    main.as_str()
                ),
            )
        };
        // A single ref transaction prevents a crash between branch and base creation.
        self.update_refs(&update)?;
        Ok(Branch {
            name: name.clone(),
            base,
        })
    }

    fn head_locked(&self, name: &BranchName) -> Result<CommitId, Error> {
        self.resolve(&name.reference())
    }

    fn create_worktree_locked(
        &self,
        name: &BranchName,
        path: impl AsRef<Path>,
    ) -> Result<PathBuf, Error> {
        let path = self.destination(path.as_ref())?;
        self.create_branch_locked(name)?;
        let stage = self.common.join("tl3-worktree-staging").join(name.as_str());
        let checkouts = self.checkouts()?;
        if let Some(checkout) = checkouts.iter().find(|c| c.path == path) {
            if checkout.branch.as_deref() != Some(&name.reference()) {
                return Err(Error::WorktreeOccupied);
            }
            if path.exists() {
                self.verify_checkout(&path, name)?;
                self.clean_stage(&path, &stage)?;
                return Ok(path);
            }
            self.run(
                &self.repo,
                "remove missing worktree",
                [
                    OsStr::new("worktree"),
                    OsStr::new("remove"),
                    path.as_os_str(),
                ],
            )?;
        }

        if checkouts.iter().any(|c| {
            c.branch.as_deref() == Some(&name.reference()) && c.path != path && c.path != stage
        }) {
            return Err(Error::WorktreeOccupied);
        }
        if path.exists() && fs::read_dir(&path).map_err(|_| Error::Io)?.next().is_some() {
            // Only adopt a checkout whose stale gitlink points into this repository.
            let link = fs::read(path.join(".git")).map_err(|error| match error.kind() {
                std::io::ErrorKind::NotFound | std::io::ErrorKind::IsADirectory => {
                    Error::WorktreeOccupied
                }
                _ => Error::Io,
            })?;
            let expected = self.common.join("worktrees");
            let old_admin = gitlink(&link)?;
            if old_admin.parent() != Some(expected.as_path()) {
                return Err(Error::WorktreeOccupied);
            }
            self.attach_orphan(name, &path, &stage)?;
        } else {
            self.run(
                &self.repo,
                "create worktree",
                [
                    OsStr::new("worktree"),
                    OsStr::new("add"),
                    OsStr::new("--"),
                    path.as_os_str(),
                    OsStr::new(name.as_str()),
                ],
            )?;
        }
        self.verify_checkout(&path, name)?;
        Ok(path)
    }

    fn destination(&self, path: &Path) -> Result<PathBuf, Error> {
        let path = absolute_destination(path)?;
        if path == self.root || !path.starts_with(&self.root) {
            return Err(Error::WorktreePath);
        }
        Ok(path)
    }

    fn attach_orphan(&self, name: &BranchName, path: &Path, stage: &Path) -> Result<(), Error> {
        let registered = self.checkouts()?.iter().any(|c| c.path == stage);
        if !registered {
            fs::create_dir_all(stage.parent().ok_or(Error::Io)?).map_err(|_| Error::Io)?;
            self.run(
                &self.repo,
                "stage worktree registration",
                [
                    OsStr::new("worktree"),
                    OsStr::new("add"),
                    OsStr::new("--no-checkout"),
                    OsStr::new("--"),
                    stage.as_os_str(),
                    OsStr::new(name.as_str()),
                ],
            )?;
        }
        // Initialize only the index. Orphaned working files, including edits, stay intact.
        self.run(
            stage,
            "initialize worktree index",
            ["read-tree", &name.reference()],
        )?;
        let link = fs::read(stage.join(".git")).map_err(|_| Error::Io)?;
        let mut temporary = tempfile::NamedTempFile::new_in(path).map_err(|_| Error::Io)?;
        temporary.write_all(&link).map_err(|_| Error::Io)?;
        temporary
            .persist(path.join(".git"))
            .map_err(|_| Error::Io)?;
        self.run(
            &self.repo,
            "repair worktree",
            [
                OsStr::new("worktree"),
                OsStr::new("repair"),
                OsStr::new("--"),
                path.as_os_str(),
            ],
        )?;
        self.clean_stage(path, stage)
    }

    fn clean_stage(&self, path: &Path, stage: &Path) -> Result<(), Error> {
        if stage.join(".git").is_file()
            && fs::read(stage.join(".git")).map_err(|_| Error::Io)?
                == fs::read(path.join(".git")).map_err(|_| Error::Io)?
        {
            fs::remove_file(stage.join(".git")).map_err(|_| Error::Io)?;
        }
        if stage.is_dir() && fs::read_dir(stage).map_err(|_| Error::Io)?.next().is_none() {
            fs::remove_dir(stage).map_err(|_| Error::Io)?;
        }
        Ok(())
    }

    fn remove_worktree_locked(
        &self,
        name: &BranchName,
        path: impl AsRef<Path>,
        allow_deletions: bool,
    ) -> Result<(), Error> {
        leaf(name)?;
        let path = self.destination(path.as_ref())?;
        if let Some(checkout) = self.checkouts()?.iter().find(|c| c.path == path) {
            if checkout.branch.as_deref() != Some(&name.reference()) {
                return Err(Error::WorktreeOccupied);
            }
            if path.exists() {
                if allow_deletions && !path.join(".git").exists() {
                    self.restore_removal_gitlink(name, &path)?;
                }
                self.verify_checkout(&path, name)?;
                let initializing = allow_deletions && self.initializing_at_start(name, &path)?;
                if allow_deletions && !initializing {
                    let status = self.status(&path)?;
                    if status
                        .split(|b| *b == 0)
                        .filter(|entry| !entry.is_empty())
                        .any(|entry| !matches!(entry.get(..3), Some(b" D " | b"D  ")))
                    {
                        return Err(Error::DirtyWorktree);
                    }
                } else if !initializing {
                    self.require_clean(&path)?;
                }
            }
            let initializing =
                allow_deletions && path.exists() && self.initializing_at_start(name, &path)?;
            let mut args = vec![OsStr::new("worktree"), OsStr::new("remove")];
            if allow_deletions {
                args.push(OsStr::new("--force"));
            }
            if initializing {
                // Keep Git's evidence until removal; two forces permit a locked checkout.
                args.push(OsStr::new("--force"));
            }
            args.extend([OsStr::new("--"), path.as_os_str()]);
            self.run(&self.repo, "remove worktree", args)?;
        } else if path.exists() {
            // Recover a lost registration before removing the checkout through Git.
            if fs::read_dir(&path).map_err(|_| Error::Io)?.next().is_none() {
                fs::remove_dir(&path).map_err(|_| Error::Io)?;
            } else {
                self.create_worktree_locked(name, &path)?;
                self.remove_worktree_locked(name, &path, allow_deletions)?;
            }
        }
        Ok(())
    }

    fn restore_removal_gitlink(&self, name: &BranchName, path: &Path) -> Result<(), Error> {
        let link = path.join(".git");
        // A removal may delete the gitlink before any tracked files. Reattach only
        // the surviving registration whose branch and recorded gitdir both match.
        match fs::symlink_metadata(&link) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            _ => return Err(Error::WorktreeOccupied),
        }
        for entry in fs::read_dir(self.common.join("worktrees")).map_err(|_| Error::Io)? {
            let admin = entry.map_err(|_| Error::Io)?.path();
            let gitdir = fs::read(admin.join("gitdir")).map_err(|_| Error::Io)?;
            if path_line(&gitdir)? != link {
                continue;
            }
            if fs::read(admin.join("HEAD")).map_err(|_| Error::Io)?
                != format!("ref: {}\n", name.reference()).as_bytes()
            {
                return Err(Error::WorktreeOccupied);
            }
            let mut temporary = tempfile::NamedTempFile::new_in(path).map_err(|_| Error::Io)?;
            temporary.write_all(b"gitdir: ").map_err(|_| Error::Io)?;
            temporary
                .write_all(admin.as_os_str().as_encoded_bytes())
                .map_err(|_| Error::Io)?;
            temporary.write_all(b"\n").map_err(|_| Error::Io)?;
            temporary.persist_noclobber(&link).map_err(|_| Error::Io)?;
            return Ok(());
        }
        Err(Error::WorktreeOccupied)
    }

    fn delete_local_branch_locked(&self, name: &BranchName) -> Result<(), Error> {
        leaf(name)?;
        if self
            .checkouts()?
            .iter()
            .any(|c| c.branch.as_deref() == Some(&name.reference()))
        {
            return Err(Error::WorktreeOccupied);
        }
        self.update_refs(&format!(
            "delete {}\ndelete {}\ndelete refs/tl3/starts/{}\n",
            name.reference(),
            name.base_ref(),
            name.as_str()
        ))
    }

    fn remote_head_locked(
        &self,
        name: &BranchName,
        remote: &Remote,
        token: Option<&InstallationToken>,
    ) -> Result<Option<CommitId>, Error> {
        let token = remote.credentials(token);
        let output = self.network(
            "read remote branch",
            [
                OsStr::new("ls-remote"),
                OsStr::new("--refs"),
                OsStr::new("--"),
                &remote.0,
                OsStr::new(&name.reference()),
            ],
            token,
        )?;
        if output.stdout.is_empty() {
            return Ok(None);
        }
        let hash = output
            .stdout
            .split(|b| *b == b'\t')
            .next()
            .ok_or(Error::Response)?;
        Ok(Some(commit_line(hash)?))
    }

    fn rebase_onto_locked(
        &self,
        name: &BranchName,
        path: impl AsRef<Path>,
        target: &CommitId,
    ) -> Result<RebaseResult, Error> {
        leaf(name)?;
        let path = fs::canonicalize(path).map_err(|_| Error::Io)?;
        self.verify_repository(&path)?;
        let marker = self.rebase_marker(&path)?;
        if marker.exists() {
            self.restore_rebase_checkout(&path, name, &marker)?;
        }
        if self.rebase_active(&path)? {
            self.verify_repository(&path)?;
            let mut belongs = false;
            for state in ["rebase-merge/head-name", "rebase-apply/head-name"] {
                let output = self.run(
                    &path,
                    "interrupted rebase branch",
                    ["rev-parse", "--path-format=absolute", "--git-path", state],
                )?;
                if let Ok(branch) = fs::read(path_line(&output.stdout)?) {
                    belongs = branch.strip_suffix(b"\n") == Some(name.reference().as_bytes());
                    if belongs {
                        break;
                    }
                }
            }
            if !belongs {
                return Err(Error::WorktreeOccupied);
            }
            self.abort_rebase(&path)?;
        }
        self.verify_checkout(&path, name)?;
        self.require_clean(&path)?;
        let old_head = self.head_locked(name)?;
        let old_base = self.create_branch_locked(name)?.base;
        let main = target.clone();
        let mut temporary = tempfile::NamedTempFile::new_in(marker.parent().ok_or(Error::Io)?)
            .map_err(|_| Error::Io)?;
        temporary
            .write_all(name.reference().as_bytes())
            .map_err(|_| Error::Io)?;
        temporary.persist(&marker).map_err(|_| Error::Io)?;
        self.run(
            &path,
            "detach for rebase",
            ["checkout", "--detach", old_head.as_str()],
        )?;
        let output = self.execute(
            &path,
            ["rebase", "--no-autostash", main.as_str()],
            None,
            None,
        )?;
        if output.status.success() {
            self.require_clean(&path)?;
            let head = commit_line(
                &self
                    .run(&path, "rebased head", ["rev-parse", "HEAD"])?
                    .stdout,
            )?;
            self.update_refs(&format!(
                "update {} {} {}\nupdate {} {} {}\n",
                name.reference(),
                head.as_str(),
                old_head.as_str(),
                name.base_ref(),
                main.as_str(),
                old_base.as_str()
            ))?;
            self.restore_rebase_checkout(&path, name, &marker)?;
            return Ok(RebaseResult::Clean { base: main, head });
        }
        let conflicts = self.run(
            &path,
            "list conflicts",
            ["diff", "--name-only", "--diff-filter=U", "-z", "--"],
        );
        if self.rebase_active(&path)? {
            self.abort_rebase(&path)?;
        }
        self.restore_rebase_checkout(&path, name, &marker)?;
        if self.head_locked(name)? != old_head {
            return Err(Error::Recovery);
        }
        let files = paths(&conflicts?.stdout);
        if files.is_empty() {
            return Err(git_error("rebase", &output));
        }
        Ok(RebaseResult::Conflicted {
            head: old_head,
            files,
        })
    }

    fn changed_files_locked(
        &self,
        base: &CommitId,
        head: &CommitId,
    ) -> Result<Vec<PathBuf>, Error> {
        let output = self.run(
            &self.repo,
            "changed files",
            [
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--no-renames",
                "--name-only",
                "-z",
                base.as_str(),
                head.as_str(),
                "--",
            ],
        )?;
        Ok(paths(&output.stdout))
    }

    fn rebase_marker(&self, path: &Path) -> Result<PathBuf, Error> {
        path_line(
            &self
                .run(
                    path,
                    "rebase recovery path",
                    [
                        "rev-parse",
                        "--path-format=absolute",
                        "--git-path",
                        "tl3-rebase-branch",
                    ],
                )?
                .stdout,
        )
    }

    fn restore_rebase_checkout(
        &self,
        path: &Path,
        name: &BranchName,
        marker: &Path,
    ) -> Result<(), Error> {
        if fs::read(marker).map_err(|_| Error::Io)? != name.reference().as_bytes() {
            return Err(Error::WorktreeOccupied);
        }
        let attached = self.execute(path, ["symbolic-ref", "--quiet", "HEAD"], None, None)?;
        match attached.status.code() {
            Some(0) if attached.stdout.strip_suffix(b"\n") == Some(name.reference().as_bytes()) => {
            }
            Some(0) => return Err(Error::WorktreeOccupied),
            Some(1) => {}
            _ => return Err(git_error("rebase recovery branch", &attached)),
        }
        if self.rebase_active(path)? {
            self.abort_rebase(path)?;
        }
        self.require_clean(path)?;
        // The branch still has the old head, or both refs committed before a crash.
        self.run(
            path,
            "restore rebase head",
            ["reset", "--hard", &name.reference()],
        )?;
        self.run(
            path,
            "reattach rebase branch",
            ["symbolic-ref", "HEAD", &name.reference()],
        )?;
        fs::remove_file(marker).map_err(|_| Error::Io)
    }

    fn verify_checkout(&self, path: &Path, name: &BranchName) -> Result<(), Error> {
        self.verify_repository(path)?;
        let branch = self.run(path, "worktree branch", ["symbolic-ref", "HEAD"])?;
        if branch.stdout.strip_suffix(b"\n") != Some(name.reference().as_bytes()) {
            return Err(Error::WorktreeOccupied);
        }
        Ok(())
    }

    fn verify_repository(&self, path: &Path) -> Result<(), Error> {
        let common = self.run(
            path,
            "worktree repository",
            ["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )?;
        if fs::canonicalize(path_line(&common.stdout)?).map_err(|_| Error::Io)? != self.common {
            return Err(Error::WorktreeOccupied);
        }
        Ok(())
    }

    fn require_clean(&self, path: &Path) -> Result<(), Error> {
        if self.status(path)?.is_empty() {
            Ok(())
        } else {
            Err(Error::DirtyWorktree)
        }
    }

    fn status(&self, path: &Path) -> Result<Vec<u8>, Error> {
        let output = self.run(
            path,
            "worktree status",
            [
                "--no-optional-locks",
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=all",
            ],
        )?;
        Ok(output.stdout)
    }

    fn rebase_active(&self, path: &Path) -> Result<bool, Error> {
        for state in ["rebase-merge", "rebase-apply"] {
            let output = self.run(
                path,
                "rebase state",
                ["rev-parse", "--path-format=absolute", "--git-path", state],
            )?;
            if path_line(&output.stdout)?.exists() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn abort_rebase(&self, path: &Path) -> Result<(), Error> {
        self.run(path, "abort rebase", ["rebase", "--abort"])
            .map_err(recovery_error)?;
        self.require_clean(path).map_err(recovery_error)
    }

    fn checkouts(&self) -> Result<Vec<Checkout>, Error> {
        let output = self.run(
            &self.repo,
            "list worktrees",
            ["worktree", "list", "--porcelain", "-z"],
        )?;
        let mut checkouts = Vec::<Checkout>::new();
        for field in output.stdout.split(|b| *b == 0) {
            if let Some(path) = field.strip_prefix(b"worktree ") {
                checkouts.push(Checkout {
                    path: PathBuf::from(OsString::from_vec(path.to_vec())),
                    branch: None,
                });
            } else if let Some(branch) = field.strip_prefix(b"branch ") {
                checkouts.last_mut().ok_or(Error::Response)?.branch =
                    Some(String::from_utf8(branch.to_vec()).map_err(|_| Error::Response)?);
            }
        }
        Ok(checkouts)
    }

    fn resolve(&self, reference: &str) -> Result<CommitId, Error> {
        self.resolve_optional(reference)?
            .ok_or(Error::MissingBranch)
    }

    fn resolve_optional(&self, reference: &str) -> Result<Option<CommitId>, Error> {
        let output = self.execute(
            &self.repo,
            [
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{reference}^{{commit}}"),
            ],
            None,
            None,
        )?;
        match output.status.code() {
            Some(0) => Ok(Some(commit_line(&output.stdout)?)),
            Some(1) => Ok(None),
            _ => Err(git_error("resolve commit", &output)),
        }
    }

    fn update_refs(&self, updates: &str) -> Result<(), Error> {
        let input = format!("start\n{updates}prepare\ncommit\n");
        let output = self.execute(
            &self.repo,
            ["update-ref", "--stdin"],
            None,
            Some(input.as_bytes()),
        )?;
        success("update refs", output).map(|_| ())
    }

    fn run<I, S>(&self, path: &Path, operation: &'static str, args: I) -> Result<Output, Error>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        success(operation, self.execute(path, args, None, None)?)
    }

    fn network<I, S>(
        &self,
        operation: &'static str,
        args: I,
        token: Option<&InstallationToken>,
    ) -> Result<Output, Error>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        success(operation, self.execute(&self.repo, args, token, None)?)
    }

    fn execute<I, S>(
        &self,
        path: &Path,
        args: I,
        token: Option<&InstallationToken>,
        input: Option<&[u8]>,
    ) -> Result<Output, Error>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.execute_index(path, args, token, input, None)
    }

    fn execute_index<I, S>(
        &self,
        path: &Path,
        args: I,
        token: Option<&InstallationToken>,
        input: Option<&[u8]>,
        index: Option<&Path>,
    ) -> Result<Output, Error>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.execute_with_environment(
            path,
            args,
            token,
            input,
            index.map_or(GitEnvironment::Default, GitEnvironment::Index),
        )
    }

    fn execute_with_environment<I, S>(
        &self,
        path: &Path,
        args: I,
        token: Option<&InstallationToken>,
        input: Option<&[u8]>,
        environment: GitEnvironment<'_>,
    ) -> Result<Output, Error>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = Command::new("git");
        // Clear inherited tracing, config injection, alternate indexes and credential helpers.
        command
            .env_clear()
            .env(
                "PATH",
                std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into()),
            )
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_NO_LAZY_FETCH", "1")
            .env("GIT_EDITOR", "true")
            .env("GIT_SEQUENCE_EDITOR", "true")
            .env("LC_ALL", "C")
            .args([
                "--no-replace-objects",
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "credential.helper=",
                "-c",
                "http.followRedirects=false",
                "-c",
                "protocol.allow=never",
                "-c",
                "protocol.https.allow=always",
                "-c",
                "protocol.file.allow=always",
                "-c",
                "core.fsmonitor=false",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "rebase.updateRefs=false",
                "-c",
                "rerere.enabled=false",
                "-c",
                "user.name=TL3",
                "-c",
                "user.email=tl3@localhost",
            ])
            .current_dir(path)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            });
        if matches!(environment, GitEnvironment::Snapshot) {
            command
                .env("GIT_AUTHOR_NAME", "TL3")
                .env("GIT_AUTHOR_EMAIL", "tl3@example.invalid")
                .env("GIT_COMMITTER_NAME", "TL3")
                .env("GIT_COMMITTER_EMAIL", "tl3@example.invalid")
                .env("GIT_AUTHOR_DATE", "2000-01-01T00:00:00Z")
                .env("GIT_COMMITTER_DATE", "2000-01-01T00:00:00Z");
        }
        if let GitEnvironment::Index(index) = environment {
            command.env("GIT_INDEX_FILE", index);
        }
        if let Some(token) = token {
            let header = format!(
                "Authorization: Basic {}",
                STANDARD.encode(format!("x-access-token:{}", token.0))
            );
            for (index, (key, value)) in [
                ("credential.helper", ""),
                ("http.extraHeader", ""),
                ("http.https://github.com/.extraHeader", header.as_str()),
                ("http.followRedirects", "false"),
                ("protocol.allow", "never"),
                ("protocol.https.allow", "always"),
                ("protocol.file.allow", "always"),
            ]
            .into_iter()
            .enumerate()
            {
                command
                    .env(format!("GIT_CONFIG_KEY_{index}"), key)
                    .env(format!("GIT_CONFIG_VALUE_{index}"), value);
            }
            command.env("GIT_CONFIG_COUNT", "7");
        }
        #[cfg(test)]
        let argv = command.get_args().map(OsStr::to_os_string).collect();
        #[cfg(test)]
        let environment = command
            .get_envs()
            .map(|(k, v)| (k.to_os_string(), v.map(OsStr::to_os_string)))
            .collect();
        let mut child = command.spawn().map_err(|_| Error::Spawn)?;
        if let Some(input) = input {
            let written = child.stdin.take().ok_or(Error::Io)?.write_all(input);
            if written.is_err() {
                let _ = child.wait();
                return Err(Error::Io);
            }
        }
        let output = child.wait_with_output().map_err(|_| Error::Io)?;
        #[cfg(test)]
        self.invocations
            .lock()
            .map_err(|_| Error::Poisoned)?
            .push(Invocation {
                argv,
                environment,
                stdout: output.stdout.clone(),
                stderr: output.stderr.clone(),
            });
        Ok(output)
    }
}

fn leaf(name: &BranchName) -> Result<(), Error> {
    if name.as_str() == "main" {
        Err(Error::MainBranch)
    } else {
        Ok(())
    }
}

fn success(operation: &'static str, output: Output) -> Result<Output, Error> {
    if output.status.success() {
        Ok(output)
    } else {
        Err(git_error(operation, &output))
    }
}

fn git_error(operation: &'static str, output: &Output) -> Error {
    Error::Git {
        operation,
        code: output.status.code(),
    }
}

fn commit_line(bytes: &[u8]) -> Result<CommitId, Error> {
    CommitId::new(
        std::str::from_utf8(bytes.strip_suffix(b"\n").unwrap_or(bytes))
            .map_err(|_| Error::Response)?,
    )
    .map_err(|_| Error::Response)
}

fn path_line(bytes: &[u8]) -> Result<PathBuf, Error> {
    let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    if bytes.is_empty() {
        return Err(Error::Response);
    }
    Ok(PathBuf::from(OsString::from_vec(bytes.to_vec())))
}

fn gitlink(bytes: &[u8]) -> Result<PathBuf, Error> {
    path_line(
        bytes
            .strip_prefix(b"gitdir: ")
            .ok_or(Error::WorktreeOccupied)?,
    )
}

fn paths(bytes: &[u8]) -> Vec<PathBuf> {
    bytes
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| PathBuf::from(OsString::from_vec(p.to_vec())))
        .collect()
}

fn absolute_destination(path: &Path) -> Result<PathBuf, Error> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir().map_err(|_| Error::Io)?.join(path)
    };
    let mut resolved = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::RootDir => resolved.push("/"),
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            Component::Normal(part) => {
                resolved.push(part);
                match fs::symlink_metadata(&resolved) {
                    Ok(_) => resolved = fs::canonicalize(&resolved).map_err(|_| Error::Io)?,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(_) => return Err(Error::Io),
                }
            }
            Component::Prefix(_) => return Err(Error::WorktreePath),
        }
    }
    Ok(resolved)
}

#[cfg(test)]
struct Invocation {
    argv: Vec<OsString>,
    environment: Vec<(OsString, Option<OsString>)>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

mod scope;
fn recovery_error(error: Error) -> Error {
    match error {
        Error::Git { .. } | Error::DirtyWorktree => Error::Recovery,
        error => error,
    }
}

#[cfg(test)]
mod tests;
