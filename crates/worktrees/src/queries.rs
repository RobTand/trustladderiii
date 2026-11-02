use super::*;

impl Worktrees {
    pub fn head(&self) -> Result<CommitId, Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        self.resolve("HEAD")
    }

    pub fn verify_commit(&self, commit: &CommitId) -> Result<CommitId, Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        self.resolve(commit.as_str())
    }

    pub fn merge_base(&self, base: &CommitId, head: &CommitId) -> Result<CommitId, Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        commit_line(
            &self
                .run(
                    &self.repo,
                    "merge base",
                    ["merge-base", base.as_str(), head.as_str()],
                )?
                .stdout,
        )
    }

    /// The recorded base of the branch this checkout has checked out, if any.
    pub fn checked_out_base(&self) -> Result<Option<CommitId>, Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        let attached = self.execute(&self.repo, ["symbolic-ref", "--quiet", "HEAD"], None, None)?;
        match attached.status.code() {
            Some(0) => {}
            Some(1) => return Ok(None),
            _ => return Err(git_error("checked-out branch", &attached)),
        }
        let reference = std::str::from_utf8(&attached.stdout).map_err(|_| Error::BranchName)?;
        let Some(name) = reference.trim_end().strip_prefix("refs/heads/") else {
            return Ok(None);
        };
        self.resolve_optional(&BranchName::new(name)?.base_ref())
    }

    pub fn is_ancestor(&self, base: &CommitId, head: &CommitId) -> Result<bool, Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        let output = self.execute(
            &self.repo,
            ["merge-base", "--is-ancestor", base.as_str(), head.as_str()],
            None,
            None,
        )?;
        match output.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(git_error("is ancestor", &output)),
        }
    }

    /// Check publication against the current base without rewriting the approved head.
    pub fn merges_cleanly(&self, base: &CommitId, head: &CommitId) -> Result<bool, Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        let output = self.execute(
            &self.repo,
            ["merge-tree", "--write-tree", base.as_str(), head.as_str()],
            None,
            None,
        )?;
        match output.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(git_error("publication merge", &output)),
        }
    }

    /// Compare the two trees, without external diff or text conversion programs.
    pub fn diff(&self, base: &CommitId, head: &CommitId) -> Result<Vec<u8>, Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        Ok(self
            .run(
                &self.repo,
                "diff",
                [
                    "diff",
                    "--no-ext-diff",
                    "--no-textconv",
                    base.as_str(),
                    head.as_str(),
                    "--",
                ],
            )?
            .stdout)
    }

    /// Paths changed on head since the merge base. Base-only changes are excluded.
    pub fn changed_paths_since_merge_base(
        &self,
        base: &CommitId,
        head: &CommitId,
    ) -> Result<Vec<PathBuf>, Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        let output = self.run(
            &self.repo,
            "changed paths since merge base",
            [
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--no-renames",
                "--name-only",
                "-z",
                &format!("{}...{}", base.as_str(), head.as_str()),
                "--",
            ],
        )?;
        Ok(paths(&output.stdout))
    }

    /// The id of the commit's root tree.
    pub fn tree(&self, commit: &CommitId) -> Result<String, Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        let tree = format!("{}^{{tree}}", commit.as_str());
        let output = self.run(
            &self.repo,
            "commit tree",
            ["rev-parse", "--verify", tree.as_str()],
        )?;
        Ok(commit_line(&output.stdout)?.as_str().to_owned())
    }

    pub fn list_tree(&self, commit: &CommitId) -> Result<Vec<u8>, Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        Ok(self
            .run(
                &self.repo,
                "list tree",
                ["ls-tree", "-r", "-l", "-z", commit.as_str()],
            )?
            .stdout)
    }

    pub fn blob(&self, object: &BlobId) -> Result<Vec<u8>, Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        Ok(self
            .run(
                &self.repo,
                "read blob",
                ["cat-file", "blob", object.as_str()],
            )?
            .stdout)
    }
}
