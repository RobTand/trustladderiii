mod reproducing;
use super::record::Records;
use crate::executor::Output as Out;
use crate::{
    Error, Repository, Result,
    executor::{Completion, Delivery, Executor, Observation, Reconciliation},
};

use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
};
use tl3_kernel::EffectKind;
use tl3_worktrees::{BranchName, CommitId, InstallationToken, RebaseResult, Remote, Worktrees};

pub struct Fake(Records);
impl Fake {
    pub fn new(root: &Path) -> Self {
        Self(Records::new(root))
    }
}
impl Executor for Fake {
    fn execute(&mut self, delivery: &Delivery) -> Result<Completion> {
        let node = delivery.node();
        let observations = match delivery.output {
            Out::Effect {
                kind: EffectKind::StartRebase,
                ..
            } => vec![Observation::Rebased {
                node,
                generation: delivery.generation,
                sha: format!("{:040x}", 2000 + node.0 * 100 + delivery.generation),
            }],
            Out::Effect {
                kind: EffectKind::PrepareWorktree | EffectKind::Push | EffectKind::RemoveWorktree,
                ..
            } => vec![],
            _ => return Err(crate::Error::Invalid("unsupported worktree effect")),
        };
        self.0
            .deliver(delivery, Completion::confirmed(observations))
    }
    fn reconcile(&mut self, delivery: &Delivery) -> Result<Reconciliation> {
        self.0.reconcile(delivery)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Boundary {
    BeforeApply,
    AfterApply,
}

/// One controller process owns every served repository and its Git subprocesses.
/// Keep the common Git directory when restarting, including tl3-controller records.
pub struct Real {
    root: PathBuf,
    epoch: i64,
    repositories: Vec<Served>,
    branches: crate::branches::Branches,
}

struct Served {
    identity: Repository,
    git: Worktrees,
    remote: Remote,
    github: Option<(tl3_github::GitHub, tokio::runtime::Runtime)>,
    journal: PathBuf,
    mutex: Arc<Mutex<()>>,
}

#[derive(Serialize, Deserialize)]
enum Intent {
    Prepare {
        base: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        start: Option<String>,
    },
    Push {
        head: String,
        expected: Option<String>,
    },
    Rebase {
        target: String,
        #[serde(default)]
        pin_failed: bool,
    },
    Remove {
        expected: Option<String>,
        #[serde(default)]
        clean_verified: bool,
        #[serde(default)]
        scratch_ref: Option<String>,
    },
}

#[derive(Serialize, Deserialize)]
struct Record {
    delivery: Delivery,
    intent: Intent,
    #[serde(default)]
    scratch_prune: Vec<(String, String)>,
    completion: Option<Completion>,
}

fn adapter(error: impl Into<Error>) -> Error {
    error.into()
}

impl Real {
    pub fn with_repositories(mut self, repos: &[tl3_config::Repository]) -> Result<Self> {
        self.branches = crate::branches::Branches::new(repos)?;
        for repo in &mut self.repositories {
            repo.git
                .set_base_branch(self.branches.base(&repo.identity)?)?;
        }
        Ok(self)
    }

    pub fn new(root: impl AsRef<Path>, epoch: i64) -> Result<Self> {
        fs::create_dir_all(root.as_ref())?;
        Ok(Self {
            root: fs::canonicalize(root)?,
            epoch,
            repositories: vec![],
            branches: crate::branches::Branches::new(&[])?,
        })
    }

    /// Local fixtures use file transport and never obtain or pass credentials.
    pub fn serve_local(
        &mut self,
        identity: Repository,
        repository: &Path,
        remote: &Path,
    ) -> Result<()> {
        self.serve(
            identity,
            repository,
            Remote::local(remote).map_err(adapter)?,
            None,
        )
    }

    pub fn serve_github(
        &mut self,
        identity: Repository,
        repository: &Path,
        github: tl3_github::GitHub,
    ) -> Result<()> {
        if github.installation_id() != identity.installation {
            return Err(Error::Invalid("worktree GitHub installation mismatch"));
        }
        let remote = Remote::https(&format!(
            "https://github.com/{}/{}.git",
            identity.owner, identity.name
        ))
        .map_err(adapter)?;
        self.serve(identity, repository, remote, Some(github))
    }

    fn serve(
        &mut self,
        identity: Repository,
        repository: &Path,
        remote: Remote,
        github: Option<tl3_github::GitHub>,
    ) -> Result<()> {
        if self
            .repositories
            .iter()
            .any(|r| same_repo(&r.identity, &identity))
        {
            return Err(Error::Invalid("duplicate worktree repository"));
        }
        let base = self.branches.base(&identity)?;
        let github = github
            .map(|client| {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .max_blocking_threads(2)
                    .build()
                    .map(|runtime| (client, runtime))
            })
            .transpose()?;
        let mut git = Worktrees::open(repository, &self.root).map_err(adapter)?;
        git.set_base_branch(base)?;
        let journal = git.common_directory().join("tl3-controller");
        fs::create_dir_all(&journal)?;
        File::open(git.common_directory())?.sync_all()?;
        type Mutexes = Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>;
        static MUTEXES: OnceLock<Mutexes> = OnceLock::new();
        let mut mutexes = MUTEXES
            .get_or_init(Mutex::default)
            .lock()
            .map_err(|_| Error::Invalid("worktree mutex poisoned"))?;
        mutexes.retain(|_, mutex| mutex.strong_count() > 0);
        let mutex = mutexes
            .get(&journal)
            .and_then(Weak::upgrade)
            .unwrap_or_default();
        mutexes.insert(journal.clone(), Arc::downgrade(&mutex));
        drop(mutexes);
        let guard = mutex
            .lock()
            .map_err(|_| Error::Invalid("worktree mutex poisoned"))?;
        let binding = journal.join("repository.json");
        if let Some(existing) = read::<(Repository, PathBuf)>(&binding)? {
            if !same_repo(&existing.0, &identity) || existing.1 != self.root {
                return Err(Error::Invalid(
                    "worktree repository or root binding changed",
                ));
            }
        } else {
            write(&binding, &(&identity, &self.root))?;
        }
        let fence = read::<i64>(&journal.join("epoch.json"))?.unwrap_or(0);
        write(&journal.join("epoch.json"), &fence.max(self.epoch))?;
        drop(guard);
        self.repositories.push(Served {
            identity,
            git,
            remote,
            github,
            journal,
            mutex,
        });
        Ok(())
    }

    /// Run before reporting a proposed head. This does not approve it or change kernel state.
    pub fn trim_scope(
        &self,
        delivery: &Delivery,
        protected: &[String],
    ) -> Result<tl3_worktrees::ScopeTrim> {
        let repo = self.repository(delivery)?;
        let _guard = repo
            .mutex
            .lock()
            .map_err(|_| Error::Invalid("worktree mutex poisoned"))?;
        if delivery.epoch < repo.fence()? {
            return Err(adapter(std::io::Error::other(
                "worktree upstream refused an older controller epoch",
            )));
        }
        write(&repo.journal.join("epoch.json"), &delivery.epoch)?;
        repo.git
            .trim_scope(
                &self.branch_for(delivery)?,
                &self.checkout(delivery),
                &delivery.paths.iter().map(PathBuf::from).collect::<Vec<_>>(),
                protected,
            )
            .map_err(adapter)
    }

    pub fn branch_for(&self, delivery: &Delivery) -> Result<BranchName> {
        self.branches.branch(delivery)
    }

    pub fn checkout(&self, delivery: &Delivery) -> PathBuf {
        checkout_path(&self.root, delivery)
    }

    fn repository(&self, delivery: &Delivery) -> Result<&Served> {
        self.repositories
            .iter()
            .find(|r| same_repo(&r.identity, &delivery.repository))
            .ok_or(Error::Invalid("unserved worktree repository"))
    }
}

pub(crate) fn checkout_path(root: &Path, delivery: &Delivery) -> PathBuf {
    let owner = hex(delivery.repository.owner.to_ascii_lowercase().as_bytes());
    let repo = hex(delivery.repository.name.to_ascii_lowercase().as_bytes());
    root.join(owner).join(repo).join(format!(
        "issue-{}-node-{}",
        delivery.issue,
        delivery.node().0
    ))
}

impl Served {
    fn token(&self) -> Result<Option<InstallationToken>> {
        self.github
            .as_ref()
            .map(|(github, runtime)| {
                let token = runtime
                    .block_on(github.installation_token())
                    .map_err(adapter)?;
                InstallationToken::new(token).map_err(adapter)
            })
            .transpose()
    }

    fn fence(&self) -> Result<i64> {
        Ok(read::<i64>(&self.journal.join("epoch.json"))?.unwrap_or(0))
    }

    fn published_path(&self, branch: &BranchName) -> PathBuf {
        self.journal.join(format!(
            "published-{}.json",
            hex(branch.as_str().as_bytes())
        ))
    }

    fn record_path(&self, delivery: &Delivery) -> PathBuf {
        self.journal
            .join(format!("effect-{}.json", hex(delivery.key.as_bytes())))
    }

    fn load(&self, delivery: &Delivery) -> Result<Option<Record>> {
        let record: Option<Record> = read(&self.record_path(delivery))?;
        if let Some(record) = &record
            && (record.delivery.key != delivery.key
                || record.delivery.generation != delivery.generation
                || !same_repo(&record.delivery.repository, &delivery.repository)
                || record.delivery.issue != delivery.issue
                || serde_json::to_value(record.delivery.output)?
                    != serde_json::to_value(delivery.output)?
                || matches!(&record.intent, Intent::Push { head, .. }
                    if delivery.head_sha.as_ref() != Some(head)))
        {
            return Err(Error::Invalid(
                "worktree effect key reused with different payload",
            ));
        }
        Ok(record)
    }

    fn intent(&self, delivery: &Delivery, branch: &BranchName, checkout: &Path) -> Result<Intent> {
        match delivery.output {
            Out::Effect {
                kind: EffectKind::PrepareWorktree,
                ..
            } => {
                let base = self
                    .git
                    .fetch_base(&self.remote, self.token()?.as_ref())
                    .map_err(adapter)?;
                let start = delivery
                    .proposed
                    .as_ref()
                    .map(|proposed| {
                        let pinned = CommitId::new(&proposed.head).map_err(adapter)?;
                        self.git
                            .fetch_pull_head(
                                &self.remote,
                                self.token()?.as_ref(),
                                proposed.number,
                                &pinned,
                            )
                            .map_err(adapter)
                    })
                    .transpose()?;
                Ok(Intent::Prepare {
                    base: base.as_str().into(),
                    start: start.map(|head| head.as_str().to_owned()),
                })
            }
            Out::Effect {
                kind: EffectKind::Push,
                ..
            } => {
                let head = CommitId::new(
                    delivery
                        .head_sha
                        .clone()
                        .ok_or(Error::Invalid("push needs an exact head"))?,
                )
                .map_err(adapter)?;
                let expected = read(&self.published_path(branch))?;
                Ok(Intent::Push {
                    head: head.as_str().into(),
                    expected,
                })
            }
            Out::Effect {
                kind: EffectKind::StartRebase,
                ..
            } => {
                let target = self
                    .git
                    .fetch_base(&self.remote, self.token()?.as_ref())
                    .map_err(adapter)?;
                let mut pin_failed = false;
                if let Some(run) = &delivery.checks
                    && let Some(reviewed) = &run.reviewed_sha
                {
                    // Pin before rewriting the branch, including across a controller crash.
                    if let Err(error) =
                        reproducing::pin(self.git.common_directory(), branch, reviewed)
                    {
                        eprintln!("reviewed tree pin failed for {}: {error}", branch.as_str());
                        pin_failed = true;
                    }
                }
                Ok(Intent::Rebase {
                    target: target.as_str().into(),
                    pin_failed,
                })
            }
            Out::Effect {
                kind: EffectKind::RemoveWorktree,
                ..
            } => {
                let scratch_ref = match self.git.verify_removal_clean(branch, checkout) {
                    Ok(()) => None,
                    Err(tl3_worktrees::Error::DirtyWorktree) => self
                        .git
                        .save_scratch(branch, checkout, delivery.generation)
                        .map_err(adapter)?,
                    Err(error) => return Err(adapter(error)),
                };
                Ok(Intent::Remove {
                    expected: read(&self.published_path(branch))?,
                    clean_verified: true,
                    scratch_ref,
                })
            }
            _ => Err(Error::Invalid("unsupported worktree effect")),
        }
    }

    fn finish(
        &self,
        record: &mut Record,
        branch: &BranchName,
        checkout: &Path,
        writable: bool,
        hook: &mut impl FnMut(Boundary),
    ) -> Result<Option<Completion>> {
        let mut observations = vec![];
        match &record.intent {
            Intent::Prepare { base, start } => {
                if writable {
                    let base = CommitId::new(base).map_err(adapter)?;
                    if let Some(start) = start {
                        self.git
                            .create_branch_from(
                                branch,
                                &base,
                                &CommitId::new(start).map_err(adapter)?,
                            )
                            .map_err(adapter)?;
                    } else {
                        self.git.create_branch_at(branch, &base).map_err(adapter)?;
                    }
                    self.git
                        .create_worktree(branch, checkout)
                        .map_err(adapter)?;
                }
                let Some((_, durable_base)) = self.git.branch_state(branch).map_err(adapter)?
                else {
                    return Ok(None);
                };
                if !self
                    .git
                    .finish_prepare(branch, checkout, writable)
                    .map_err(adapter)?
                {
                    return Ok(None);
                }
                if let Some(start) = start {
                    observations.push(Observation::WorktreePrepared {
                        node: record.delivery.node(),
                        repository: checkout.into(),
                        base_sha: durable_base.as_str().into(),
                        sha: start.clone(),
                    });
                }
            }
            Intent::Push { head, expected } => {
                self.git
                    .verify_removal_clean(branch, checkout)
                    .map_err(adapter)?;
                let head = CommitId::new(head).map_err(adapter)?;
                let main = self
                    .git
                    .fetch_base(&self.remote, self.token()?.as_ref())
                    .map_err(adapter)?;
                if !self.git.merges_cleanly(&main, &head).map_err(adapter)? {
                    return Err(tl3_worktrees::Error::UnrebasedHead.into());
                }
                let expected = expected
                    .as_ref()
                    .map(CommitId::new)
                    .transpose()
                    .map_err(adapter)?;
                let token = self.token()?;
                if writable {
                    self.git
                        .publish(
                            branch,
                            &self.remote,
                            token.as_ref(),
                            &head,
                            expected.as_ref(),
                        )
                        .map_err(adapter)?;
                }
                if self
                    .git
                    .remote_head(branch, &self.remote, token.as_ref())
                    .map_err(adapter)?
                    .as_ref()
                    != Some(&head)
                {
                    return Ok(None);
                }
                write(&self.published_path(branch), &head.as_str())?;
            }
            Intent::Rebase { target, pin_failed } => {
                let target = CommitId::new(target).map_err(adapter)?;
                let Some((head, base)) = self.git.branch_state(branch).map_err(adapter)? else {
                    return Ok(None);
                };
                let resolution = self
                    .git
                    .merge_resolution(branch, checkout, &target, writable)
                    .map_err(adapter)?;
                let result = if let Some(files) = resolution {
                    RebaseResult::Conflicted { head, files }
                } else if base == target {
                    // Finish any checkout restoration interrupted after the atomic ref update.
                    if writable {
                        self.git.recover_rebase(branch, checkout).map_err(adapter)?;
                    } else if !self
                        .git
                        .rebase_complete(branch, checkout)
                        .map_err(adapter)?
                    {
                        return Ok(None);
                    }
                    if !self
                        .git
                        .checkout_exists(branch, checkout)
                        .map_err(adapter)?
                    {
                        return Ok(None);
                    }
                    RebaseResult::Clean { base, head }
                } else if writable {
                    self.git
                        .rebase_onto(branch, checkout, &target)
                        .map_err(adapter)?
                } else {
                    return Ok(None);
                };
                match result {
                    RebaseResult::Clean { base, head } => {
                        if let Some(mut run) = record.delivery.checks.clone() {
                            if *pin_failed {
                                reproducing::unavailable(
                                    &mut run,
                                    "the reviewed tree pin failed before rebasing",
                                );
                            }
                            if run.reviewed_sha.is_some() {
                                if !writable {
                                    return Ok(None);
                                }
                                reproducing::rebase(
                                    self.git.common_directory(),
                                    branch,
                                    &mut run,
                                    base.as_str(),
                                );
                            }
                            run.base_sha = base.clone();
                            observations.push(Observation::CheckPlan {
                                node: record.delivery.node(),
                                sha: head.as_str().into(),
                                run,
                            });
                        }
                        observations.push(Observation::Rebased {
                            node: record.delivery.node(),
                            generation: record.delivery.generation,
                            sha: head.as_str().into(),
                        });
                    }
                    RebaseResult::Conflicted { head, files } => {
                        if !writable {
                            return Ok(None);
                        }
                        self.git
                            .prepare_merge_resolution(branch, checkout, &target)
                            .map_err(adapter)?;
                        if let Some(mut run) = record.delivery.checks.clone() {
                            reproducing::rebase(
                                self.git.common_directory(),
                                branch,
                                &mut run,
                                target.as_str(),
                            );
                            run.base_sha = target.clone();
                            observations.push(Observation::CheckPlan {
                                node: record.delivery.node(),
                                sha: head.as_str().into(),
                                run,
                            });
                        }
                        observations.push(Observation::RebaseConflict {
                            node: record.delivery.node(),
                            generation: record.delivery.generation,
                            head: head.as_str().into(),
                            target: target.as_str().into(),
                            files: files
                                .iter()
                                .map(|path| path.to_string_lossy().into_owned())
                                .collect(),
                        });
                    }
                }
            }
            Intent::Remove {
                expected,
                clean_verified,
                scratch_ref,
            } => {
                let token = self.token()?;
                let remote = self
                    .git
                    .remote_head(branch, &self.remote, token.as_ref())
                    .map_err(adapter)?;
                if writable {
                    if let Some(reference) = scratch_ref {
                        self.git
                            .clean_saved_scratch(branch, checkout, reference)
                            .map_err(adapter)?;
                    }
                    if *clean_verified {
                        self.git.resume_removal(branch, checkout).map_err(adapter)?;
                    } else {
                        self.git
                            .remove_worktree(branch, checkout)
                            .map_err(adapter)?;
                    }
                    if let Some(remote) = remote {
                        if expected.as_deref() != Some(remote.as_str()) {
                            return Err(Error::Invalid("teardown remote branch changed"));
                        }
                        self.git
                            .delete_remote_branch_at(branch, &self.remote, token.as_ref(), &remote)
                            .map_err(adapter)?;
                    }
                    self.git.delete_local_branch(branch).map_err(adapter)?;
                    if let Some(proposed) = &record.delivery.proposed {
                        self.git
                            .delete_pull_head(proposed.number)
                            .map_err(adapter)?;
                    }
                    reproducing::remove(self.git.common_directory(), branch)?;
                }
                if self.git.branch_state(branch).map_err(adapter)?.is_some()
                    || self
                        .git
                        .checkout_exists(branch, checkout)
                        .map_err(adapter)?
                    || self
                        .git
                        .remote_head(branch, &self.remote, token.as_ref())
                        .map_err(adapter)?
                        .is_some()
                {
                    return Ok(None);
                }
                if let Some(proposed) = &record.delivery.proposed
                    && self.git.has_pull_head(proposed.number).map_err(adapter)?
                {
                    return Ok(None);
                }
                if let Some(reference) = scratch_ref {
                    observations.push(Observation::ScratchSaved {
                        node: record.delivery.node(),
                        reference: reference.clone(),
                    });
                }
            }
        }
        if writable {
            for (reference, expected) in &record.scratch_prune {
                let result = CommitId::new(expected)
                    .and_then(|expected| self.git.prune_scratch(reference, &expected));
                match result {
                    Ok(()) => observations.push(Observation::ScratchPruned {
                        reference: reference.clone(),
                    }),
                    Err(error) => eprintln!("scratch housekeeping for {reference}: {error}"),
                }
            }
        }
        hook(Boundary::AfterApply);
        let completion = Completion::confirmed(observations);
        record.completion = Some(completion.clone());
        write(&self.record_path(&record.delivery), record)?;
        Ok(Some(completion))
    }
}

impl Real {
    /// Exposes durable boundaries for process-level crash verification.
    pub fn execute_with(
        &mut self,
        delivery: &Delivery,
        mut hook: impl FnMut(Boundary),
    ) -> Result<Completion> {
        let repo = self.repository(delivery)?;
        let _guard = repo
            .mutex
            .lock()
            .map_err(|_| Error::Invalid("worktree mutex poisoned"))?;
        if delivery.epoch < repo.fence()? {
            return Err(adapter(std::io::Error::other(
                "worktree upstream refused an older controller epoch",
            )));
        }
        write(&repo.journal.join("epoch.json"), &delivery.epoch)?;
        let branch = self.branch_for(delivery)?;
        let mut record = match repo.load(delivery)? {
            Some(record) => record,
            None => {
                let scratch_prune = repo
                    .git
                    .expired_scratch(
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs(),
                    )
                    .unwrap_or_else(|error| {
                        eprintln!(
                            "scratch housekeeping for {}/{}: {error}",
                            repo.identity.owner, repo.identity.name
                        );
                        vec![]
                    })
                    .into_iter()
                    .map(|(reference, head)| (reference, head.as_str().to_owned()))
                    .collect();
                let record = Record {
                    delivery: delivery.clone(),
                    intent: repo.intent(delivery, &branch, &self.checkout(delivery))?,
                    scratch_prune,
                    completion: None,
                };
                write(&repo.record_path(delivery), &record)?;
                record
            }
        };
        if let Some(completion) = record.completion {
            return Ok(completion);
        }
        hook(Boundary::BeforeApply);
        repo.finish(
            &mut record,
            &branch,
            &self.checkout(delivery),
            true,
            &mut hook,
        )?
        .ok_or(Error::Invalid(
            "worktree operation has no terminal evidence",
        ))
    }
}

impl Executor for Real {
    fn execute(&mut self, delivery: &Delivery) -> Result<Completion> {
        self.execute_with(delivery, |_| {})
    }

    fn reconcile(&mut self, delivery: &Delivery) -> Result<Reconciliation> {
        let repo = self.repository(delivery)?;
        let _guard = repo
            .mutex
            .lock()
            .map_err(|_| Error::Invalid("worktree mutex poisoned"))?;
        let Some(mut record) = repo.load(delivery)? else {
            // No Git mutation occurs until the effect intent is durable.
            return Ok(Reconciliation::Absent);
        };
        if let Some(completion) = record.completion {
            return Ok(Reconciliation::Found(completion));
        }
        let writable = delivery.epoch >= repo.fence()?;
        Ok(
            match repo.finish(
                &mut record,
                &self.branch_for(delivery)?,
                &self.checkout(delivery),
                writable,
                &mut |_| {},
            )? {
                Some(completion) => Reconciliation::Found(completion),
                None if !writable => Reconciliation::Absent,
                None => Reconciliation::Pending,
            },
        )
    }
}

fn same_repo(a: &Repository, b: &Repository) -> bool {
    a.owner.eq_ignore_ascii_case(&b.owner)
        && a.name.eq_ignore_ascii_case(&b.name)
        && a.installation == b.installation
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
fn write(path: &Path, value: &impl Serialize) -> Result<()> {
    let temporary = path.with_extension("tmp");
    let mut file = File::create(&temporary)?;
    file.write_all(&serde_json::to_vec(value)?)?;
    file.sync_all()?;
    fs::rename(&temporary, path)?;
    File::open(
        path.parent()
            .ok_or(Error::Invalid("record parent missing"))?,
    )?
    .sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests;
