use super::*;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct TrimRecord {
    path: Vec<u8>,
    footprint: Vec<Vec<u8>>,
    protected: Vec<String>,
    base: String,
    source: String,
    target: String,
    removed: Vec<Vec<u8>>,
    complete: bool,
}

fn save(path: &Path, record: &TrimRecord) -> Result<(), Error> {
    let parent = path.parent().ok_or(Error::Io)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|_| Error::Io)?;
    serde_json::to_writer(&mut temporary, record).map_err(|_| Error::Io)?;
    temporary.as_file().sync_all().map_err(|_| Error::Io)?;
    temporary.persist(path).map_err(|_| Error::Io)?;
    fs::File::open(parent)
        .and_then(|f| f.sync_all())
        .map_err(|_| Error::Io)
}

impl Worktrees {
    /// Trim once per leaf branch. Replays return the original head without touching later work.
    /// Read `branch_state` for the current head after a replay.
    pub fn trim_scope(
        &self,
        name: &BranchName,
        path: &Path,
        footprint: &[PathBuf],
        protected: &[String],
    ) -> Result<ScopeTrim, Error> {
        self.trim_scope_with(name, path, footprint, protected, |_| Ok(()))
    }

    /// The caller must retain repository ownership until this operation completes.
    pub fn trim_scope_with(
        &self,
        name: &BranchName,
        path: &Path,
        footprint: &[PathBuf],
        protected: &[String],
        mut hook: impl FnMut(TrimBoundary) -> Result<(), Error>,
    ) -> Result<ScopeTrim, Error> {
        let _guard = self.mutex.lock().map_err(|_| Error::Poisoned)?;
        self.leaf(name)?;
        let path = self.destination(path)?;
        let mut declared: Vec<_> = footprint
            .iter()
            .map(|p| p.as_os_str().as_encoded_bytes().to_vec())
            .collect();
        declared.sort();
        declared.dedup();
        let mut protected = protected.to_vec();
        protected.sort();
        protected.dedup();
        let journal = self.common.join("tl3-scope");
        fs::create_dir_all(&journal).map_err(|_| Error::Io)?;
        // Repository ownership and the mutex exclude live writers of these private files.
        for entry in fs::read_dir(&journal).map_err(|_| Error::Io)? {
            let entry = entry.map_err(|_| Error::Io)?;
            if entry.file_name().as_encoded_bytes().starts_with(b".tmp") {
                if entry.file_type().map_err(|_| Error::Io)?.is_dir() {
                    fs::remove_dir_all(entry.path()).map_err(|_| Error::Io)?;
                } else {
                    fs::remove_file(entry.path()).map_err(|_| Error::Io)?;
                }
            }
        }
        fs::File::open(&journal)
            .and_then(|f| f.sync_all())
            .map_err(|_| Error::Io)?;
        fs::File::open(&self.common)
            .and_then(|f| f.sync_all())
            .map_err(|_| Error::Io)?;
        let key: String = name.as_str().bytes().map(|b| format!("{b:02x}")).collect();
        let receipt = journal.join(format!("{key}.json"));
        let mut record = match fs::read(&receipt) {
            Ok(bytes) => {
                let record: TrimRecord =
                    serde_json::from_slice(&bytes).map_err(|_| Error::Recovery)?;
                if record.path != path.as_os_str().as_encoded_bytes()
                    || record.footprint != declared
                    || record.protected != protected
                {
                    return Err(Error::Recovery);
                }
                record
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.verify_checkout(&path, name)?;
                self.require_clean(&path)?;
                let source = self.resolve(&name.reference())?;
                let base = self.resolve(&name.base_ref())?;
                let ancestor = commit_line(
                    &self
                        .run(
                            &self.repo,
                            "scope merge base",
                            ["merge-base", base.as_str(), source.as_str()],
                        )?
                        .stdout,
                )?;
                let mut args = vec![
                    "diff".to_owned(),
                    "--no-ext-diff".into(),
                    "--no-textconv".into(),
                    "--no-renames".into(),
                    "--name-only".into(),
                    "-z".into(),
                    ancestor.as_str().into(),
                    source.as_str().into(),
                    "--".into(),
                ];
                args.extend(protected.iter().map(|p| format!(":(top,glob){p}")));
                let protected_files = if protected.is_empty() {
                    vec![]
                } else {
                    paths(&self.run(&self.repo, "protected scope paths", args)?.stdout)
                };
                let removed: Vec<_> = self
                    .changed_files_locked(&ancestor, &source)?
                    .into_iter()
                    .filter(|p| !footprint.contains(p) || protected_files.contains(p))
                    .collect();
                let target = if removed.is_empty() {
                    source.clone()
                } else {
                    self.trim_commit(&path, &journal, &base, &source, &removed)?
                };
                let record = TrimRecord {
                    path: path.as_os_str().as_encoded_bytes().to_vec(),
                    footprint: declared,
                    protected,
                    base: base.as_str().into(),
                    source: source.as_str().into(),
                    target: target.as_str().into(),
                    removed: removed
                        .iter()
                        .map(|p| p.as_os_str().as_encoded_bytes().to_vec())
                        .collect(),
                    complete: false,
                };
                // Pin the immutable target before the intent can authorize checkout changes.
                self.clear_trim_lock(
                    &self.repo,
                    &format!("refs/tl3/scope/{}.lock", name.as_str()),
                )?;
                self.run(
                    &self.repo,
                    "pin scope trim",
                    [
                        "update-ref",
                        &format!("refs/tl3/scope/{}", name.as_str()),
                        target.as_str(),
                    ],
                )?;
                save(&receipt, &record)?;
                record
            }
            Err(_) => return Err(Error::Io),
        };
        let target = CommitId::new(&record.target)?;
        let result = ScopeTrim {
            head: target.clone(),
            removed_paths: record
                .removed
                .iter()
                .map(|p| PathBuf::from(OsString::from_vec(p.clone())))
                .collect(),
        };
        if record.complete {
            return Ok(result);
        }
        self.verify_checkout(&path, name)?;
        let head = self.resolve(&name.reference())?;
        if self.resolve(&name.base_ref())?.as_str() != record.base
            || (head.as_str() != record.source && head != target)
        {
            return Err(Error::Recovery);
        }
        hook(TrimBoundary::Recorded)?;
        // The durable clean preflight owns these files until completion. A killed Git
        // child may have left index.lock or only part of the target tree installed.
        self.clear_trim_lock(&path, "index.lock")?;
        self.clear_trim_lock(&self.repo, &format!("{}.lock", name.reference()))?;
        self.run(
            &path,
            "restore scoped checkout",
            ["read-tree", "--reset", "-u", target.as_str()],
        )?;
        hook(TrimBoundary::CheckoutRestored)?;
        self.run(
            &self.repo,
            "advance scoped head",
            [
                "update-ref",
                &name.reference(),
                target.as_str(),
                head.as_str(),
            ],
        )?;
        hook(TrimBoundary::HeadUpdated)?;
        self.require_clean(&path)?;
        record.complete = true;
        save(&receipt, &record)?;
        Ok(result)
    }

    fn clear_trim_lock(&self, path: &Path, name: &str) -> Result<(), Error> {
        let lock = path_line(
            &self
                .run(
                    path,
                    "trim recovery lock",
                    ["rev-parse", "--path-format=absolute", "--git-path", name],
                )?
                .stdout,
        )?;
        match fs::remove_file(lock) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(Error::Io),
        }
    }

    fn trim_commit(
        &self,
        checkout: &Path,
        journal: &Path,
        base: &CommitId,
        source: &CommitId,
        removed: &[PathBuf],
    ) -> Result<CommitId, Error> {
        let temporary = tempfile::tempdir_in(journal).map_err(|_| Error::Io)?;
        let index = temporary.path().join("index");
        let run = |args: &[&str], input: Option<&[u8]>| {
            success(
                "build scoped tree",
                self.execute_index(checkout, args, None, input, Some(&index))?,
            )
        };
        run(&["read-tree", source.as_str()], None)?;
        let mut names = vec![];
        for path in removed {
            names.extend_from_slice(path.as_os_str().as_encoded_bytes());
            names.push(0);
        }
        // Remove both sides before installing base entries, including file/directory swaps.
        run(
            &["update-index", "--force-remove", "-z", "--stdin"],
            Some(&names),
        )?;
        let base_entries = run(&["ls-tree", "-r", "-z", base.as_str()], None)?.stdout;
        let mut restored = vec![];
        for entry in base_entries.split(|b| *b == 0).filter(|e| !e.is_empty()) {
            let path = entry
                .splitn(2, |b| *b == b'\t')
                .nth(1)
                .ok_or(Error::Recovery)?;
            if removed
                .iter()
                .any(|p| p.as_os_str().as_encoded_bytes() == path)
            {
                restored.extend_from_slice(entry);
                restored.push(0);
            }
        }
        run(&["update-index", "-z", "--index-info"], Some(&restored))?;
        let tree = commit_line(&run(&["write-tree"], None)?.stdout)?;
        let commit = self.execute(
            &self.repo,
            ["commit-tree", tree.as_str(), "-p", source.as_str()],
            None,
            Some(b"Remove changes outside the request scope\n"),
        )?;
        commit_line(&success("commit scope trim", commit)?.stdout)
    }
}
