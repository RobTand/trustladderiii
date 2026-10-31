use super::*;

impl Worktrees {
    pub fn init_bare(path: &Path) -> Result<(), Error> {
        let repo = Self::unopened(path)?;
        repo.run(
            path,
            "init bare",
            ["init", "--bare", "--quiet", "--template="],
        )?;
        Ok(())
    }

    /// Clone committed objects into a private, clean checkout at the exact head.
    pub fn clone_checkout(&self, destination: &Path, head: &CommitId) -> Result<Self, Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        self.run(
            &self.repo,
            "clone checkout",
            [
                OsStr::new("clone"),
                OsStr::new("--quiet"),
                OsStr::new("--no-local"),
                OsStr::new("--no-checkout"),
                OsStr::new("--template="),
                OsStr::new("--"),
                self.repo.as_os_str(),
                destination.as_os_str(),
            ],
        )?;
        self.run(
            destination,
            "checkout snapshot",
            ["checkout", "--quiet", "--detach", head.as_str()],
        )?;
        self.require_clean(destination)?;
        Self::open(destination, destination)
    }

    /// Preserve every check commit across PrismaBuild's checkout materialization.
    pub fn bundle_check_input(
        &self,
        destination: &Path,
        base: &CommitId,
        reviewed: Option<&CommitId>,
    ) -> Result<(), Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        self.require_clean(&self.repo)?;
        self.update_refs(&format!("update refs/heads/tl3-base {}\n", base.as_str()))?;
        let mut args = vec![
            OsString::from("bundle"),
            OsString::from("create"),
            OsString::from("--"),
            destination.as_os_str().to_owned(),
            OsString::from("HEAD"),
            OsString::from("refs/heads/tl3-base"),
        ];
        if let Some(reviewed) = reviewed {
            self.update_refs(&format!(
                "update refs/heads/tl3-reviewed {}\n",
                reviewed.as_str()
            ))?;
            args.push(OsString::from("refs/heads/tl3-reviewed"));
        }
        self.run(&self.repo, "bundle check input", args)?;
        Ok(())
    }

    pub fn commit_check_input(path: &Path) -> Result<CommitId, Error> {
        let repo = Self::unopened(path)?;
        repo.run(path, "init check input", ["init", "--quiet", "--template="])?;
        repo.run(path, "add check input", ["add", "--force", "--all"])?;
        success(
            "commit check input",
            repo.execute_with_environment(
                path,
                ["commit", "--quiet", "-m", "Freeze controller check input"],
                None,
                None,
                GitEnvironment::Snapshot,
            )?,
        )?;
        repo.resolve("HEAD")
    }

    pub fn pin_reviewed(&self, branch: &BranchName, reviewed: &CommitId) -> Result<(), Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        self.pin_reviewed_locked(branch, reviewed)
    }

    /// Replay the reviewed tree without touching the maker's index or checkout.
    pub fn replay_reviewed(
        &self,
        branch: &BranchName,
        base: &CommitId,
        target: &CommitId,
        reviewed: &CommitId,
    ) -> Result<CommitId, Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        let tree = commit_line(
            &self
                .run(
                    &self.repo,
                    "merge reviewed tree",
                    [
                        "merge-tree",
                        "--write-tree",
                        &format!("--merge-base={}", base.as_str()),
                        target.as_str(),
                        reviewed.as_str(),
                    ],
                )?
                .stdout,
        )?;
        let output = success(
            "commit reviewed tree",
            self.execute_with_environment(
                &self.repo,
                [
                    "commit-tree",
                    tree.as_str(),
                    "-p",
                    target.as_str(),
                    "-m",
                    "Replay reviewed state for reproducing checks",
                ],
                None,
                None,
                GitEnvironment::Snapshot,
            )?,
        )?;
        let head = commit_line(&output.stdout)?;
        self.pin_reviewed_locked(branch, &head)?;
        Ok(head)
    }

    pub fn remove_reviewed(&self, branch: &BranchName) -> Result<(), Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        let refs = self.run(
            &self.repo,
            "list reviewed pins",
            [
                "for-each-ref",
                "--format=%(refname)",
                &reviewed_prefix(branch),
            ],
        )?;
        for reference in std::str::from_utf8(&refs.stdout)
            .map_err(|_| Error::Response)?
            .lines()
        {
            self.run(
                &self.repo,
                "remove reviewed pin",
                ["update-ref", "-d", reference],
            )?;
        }
        Ok(())
    }

    fn pin_reviewed_locked(&self, branch: &BranchName, reviewed: &CommitId) -> Result<(), Error> {
        self.update_refs(&format!(
            "update {}{} {}\n",
            reviewed_prefix(branch),
            reviewed.as_str(),
            reviewed.as_str()
        ))
    }

    fn unopened(path: &Path) -> Result<Self, Error> {
        let repo = fs::canonicalize(path).map_err(|_| Error::Io)?;
        Ok(Self {
            repo: repo.clone(),
            common: repo.clone(),
            root: repo,
            mutex: Arc::new(Mutex::new(())),
            #[cfg(test)]
            invocations: Default::default(),
        })
    }
}

fn reviewed_prefix(branch: &BranchName) -> String {
    // Private clones fetch heads, not arbitrary controller refs.
    format!("refs/heads/tl3-reviewed/{}/", branch.as_str())
}
