//! Git operations for a repository owned by one controller process.
//!
//! All handles for the same common Git directory share an in-process mutex.
//! Other processes must not mutate the repository while the controller uses it.
//! No OS file locks are used because of the deployment kernel's file-lock bug.
#![forbid(unsafe_code)]

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
    #[error("could not abort interrupted rebase; recovery is required")]
    Recovery,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitId(String);

impl CommitId {
    pub fn new(value: impl Into<String>) -> Result<Self, Error> {
        let value = value.into();
        if !matches!(value.len(), 40 | 64) || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::CommitId);
        }
        Ok(Self(value.to_ascii_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
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

impl Worktrees {
    /// Reads main locally. The controller refreshes main before starting an operation.
    pub fn create_branch(&self, name: &BranchName) -> Result<Branch, Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        self.create_branch_locked(name)
    }

    pub fn head(&self, name: &BranchName) -> Result<CommitId, Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        self.head_locked(name)
    }

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
            self.require_clean(&path)?;
        }
        Ok(())
    }

    /// Requires a durable Remove intent proving cleanliness before deletion began.
    pub fn resume_removal(&self, name: &BranchName, path: &Path) -> Result<(), Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        self.remove_worktree_locked(name, path, true)
    }

    /// Only an unfinished Prepare may repair files, before any maker has run.
    pub fn finish_prepare(
        &self,
        name: &BranchName,
        path: &Path,
        base: &CommitId,
        writable: bool,
    ) -> Result<bool, Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        if !path.exists() {
            return Ok(false);
        }
        self.verify_checkout(path, name)?;
        if self.status(path)?.is_empty() {
            return Ok(true);
        }
        if !writable {
            return Ok(false);
        }
        if self.resolve_optional(&name.reference())?.as_ref() != Some(base)
            || self.resolve_optional(&name.base_ref())?.as_ref() != Some(base)
        {
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
        Ok(true)
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
        self.verify_checkout(path, name)?;
        Ok(true)
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
            let link = fs::read(path.join(".git")).map_err(|_| Error::WorktreeOccupied)?;
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
                if allow_deletions {
                    let status = self.status(&path)?;
                    if status
                        .split(|b| *b == 0)
                        .filter(|entry| !entry.is_empty())
                        .any(|entry| !matches!(entry.get(..3), Some(b" D " | b"D  ")))
                    {
                        return Err(Error::DirtyWorktree);
                    }
                } else {
                    self.require_clean(&path)?;
                }
            }
            self.run(
                &self.repo,
                "remove worktree",
                [
                    OsStr::new("worktree"),
                    OsStr::new("remove"),
                    // Only deletion-only retries need force; other dirt was refused above.
                    OsStr::new(if allow_deletions { "--force" } else { "--" }),
                    path.as_os_str(),
                ],
            )?;
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
            "delete {}\ndelete {}\n",
            name.reference(),
            name.base_ref()
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
            .map_err(|_| Error::Recovery)?;
        self.require_clean(path).map_err(|_| Error::Recovery)
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
            .env("GIT_EDITOR", "true")
            .env("GIT_SEQUENCE_EDITOR", "true")
            .env("LC_ALL", "C")
            .args([
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

#[cfg(test)]
mod tests;
