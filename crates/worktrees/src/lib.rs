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
        Ok(Self(path.into_os_string()))
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
        self.remove_worktree_locked(name, path)
    }

    pub fn delete_local_branch(&self, name: &BranchName) -> Result<(), Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        self.delete_local_branch_locked(name)
    }

    pub fn push(
        &self,
        name: &BranchName,
        remote: &Remote,
        token: &InstallationToken,
    ) -> Result<CommitId, Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        self.push_locked(name, remote, token)
    }

    /// Publishes only if the remote still has the controller's recorded head.
    pub fn push_rebased(
        &self,
        name: &BranchName,
        remote: &Remote,
        token: &InstallationToken,
        expected: &CommitId,
    ) -> Result<CommitId, Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        self.push_rebased_locked(name, remote, token, expected)
    }

    pub fn remote_head(
        &self,
        name: &BranchName,
        remote: &Remote,
        token: &InstallationToken,
    ) -> Result<Option<CommitId>, Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        self.remote_head_locked(name, remote, token)
    }

    pub fn delete_remote_branch(
        &self,
        name: &BranchName,
        remote: &Remote,
        token: &InstallationToken,
    ) -> Result<(), Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        self.delete_remote_branch_locked(name, remote, token)
    }

    pub fn rebase_onto_main(
        &self,
        name: &BranchName,
        path: impl AsRef<Path>,
    ) -> Result<RebaseResult, Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        self.rebase_onto_main_locked(name, path)
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
    ) -> Result<(), Error> {
        leaf(name)?;
        let path = self.destination(path.as_ref())?;
        if let Some(checkout) = self.checkouts()?.iter().find(|c| c.path == path) {
            if checkout.branch.as_deref() != Some(&name.reference()) {
                return Err(Error::WorktreeOccupied);
            }
            if path.exists() {
                self.verify_checkout(&path, name)?;
                self.require_clean(&path)?;
            }
            self.run(
                &self.repo,
                "remove worktree",
                [
                    OsStr::new("worktree"),
                    OsStr::new("remove"),
                    path.as_os_str(),
                ],
            )?;
        } else if path.exists() {
            // Recover a lost registration before removing the checkout through Git.
            if fs::read_dir(&path).map_err(|_| Error::Io)?.next().is_none() {
                fs::remove_dir(&path).map_err(|_| Error::Io)?;
            } else {
                self.create_worktree_locked(name, &path)?;
                self.remove_worktree_locked(name, &path)?;
            }
        }
        Ok(())
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

    fn push_locked(
        &self,
        name: &BranchName,
        remote: &Remote,
        token: &InstallationToken,
    ) -> Result<CommitId, Error> {
        leaf(name)?;
        let head = self.head_locked(name)?;
        let reference = name.reference();
        // Read and lease the remote ref so retries cannot overwrite somebody else's push.
        let previous = self.remote_head_locked(name, remote, token)?;
        let lease = format!(
            "--force-with-lease={reference}:{}",
            previous.as_ref().map_or("", CommitId::as_str)
        );
        if previous.as_ref() != Some(&head) {
            if let Some(previous) = &previous {
                let known = self.resolve_optional(previous.as_str())?;
                if known.is_none() || !self.is_ancestor(previous, &head)? {
                    return Err(Error::Git {
                        operation: "push would overwrite remote work",
                        code: None,
                    });
                }
            }
            self.network(
                "push",
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
        }
        Ok(head)
    }

    fn push_rebased_locked(
        &self,
        name: &BranchName,
        remote: &Remote,
        token: &InstallationToken,
        expected: &CommitId,
    ) -> Result<CommitId, Error> {
        leaf(name)?;
        let head = self.head_locked(name)?;
        if self.remote_head_locked(name, remote, token)?.as_ref() == Some(&head) {
            return Ok(head);
        }
        let reference = name.reference();
        let lease = format!("--force-with-lease={reference}:{}", expected.as_str());
        self.network(
            "push rebased branch",
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
        Ok(head)
    }

    fn remote_head_locked(
        &self,
        name: &BranchName,
        remote: &Remote,
        token: &InstallationToken,
    ) -> Result<Option<CommitId>, Error> {
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

    fn delete_remote_branch_locked(
        &self,
        name: &BranchName,
        remote: &Remote,
        token: &InstallationToken,
    ) -> Result<(), Error> {
        leaf(name)?;
        if let Some(head) = self.remote_head_locked(name, remote, token)? {
            let reference = name.reference();
            let lease = format!("--force-with-lease={reference}:{}", head.as_str());
            self.network(
                "delete remote branch",
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
        }
        Ok(())
    }

    fn rebase_onto_main_locked(
        &self,
        name: &BranchName,
        path: impl AsRef<Path>,
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
        let main = self.resolve("refs/heads/main")?;
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

    fn is_ancestor(&self, base: &CommitId, head: &CommitId) -> Result<bool, Error> {
        let output = self.execute(
            &self.repo,
            ["merge-base", "--is-ancestor", base.as_str(), head.as_str()],
            None,
            None,
        )?;
        match output.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(git_error("check ancestry", &output)),
        }
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
        let output = self.run(
            path,
            "worktree status",
            ["status", "--porcelain=v1", "-z", "--untracked-files=all"],
        )?;
        if output.stdout.is_empty() {
            Ok(())
        } else {
            Err(Error::DirtyWorktree)
        }
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
        token: &InstallationToken,
    ) -> Result<Output, Error>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        success(
            operation,
            self.execute(&self.repo, args, Some(token), None)?,
        )
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
                ("http.extraHeader", header.as_str()),
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
