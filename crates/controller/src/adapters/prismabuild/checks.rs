use crate::{
    Error, Result,
    contracts::{CheckRequest, bounded::CommitSha},
    executor::Delivery,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs::{self, File},
    path::{Path, PathBuf},
};
use tl3_worktrees::{CommitId, Worktrees};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CheckRun {
    pub repository: PathBuf,
    pub base_sha: CommitId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewed_sha: Option<CommitId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reproducing_unavailable: Option<String>,
    pub plan: Vec<Check>,
    /// Controller-owned membership of selected units that failed in any earlier round.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selected_failures: Vec<String>,
}

pub use crate::contracts::{Check, CheckKind as Kind};

/// The trusted harness this controller stages.
///
/// The configured path runs through `current`, which deploy.py's install_release switches
/// without restarting tl3d, and collect_releases may later delete the release it pointed
/// to (#750 review). So the adapter reads the harness modules once, at startup, from the
/// release `current` resolves to then. Adapters start before `Controller::open` claims
/// ownership, so loading only reads; the copy that staging uses is published under
/// `copies` on first use, which only the owning controller reaches. Any failure leaves the
/// harness unavailable for the run; nothing falls back to the configured path.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Harness {
    /// Read at startup and not yet published.
    Loaded(Modules),
    /// The published copy, which only this controller writes.
    Copied(PathBuf),
    /// Loading or publishing failed. Staging refuses until tl3d restarts.
    Unavailable(String),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Modules {
    copies: PathBuf,
    digest: String,
    files: Vec<(std::ffi::OsString, Vec<u8>)>,
}

/// A point in publishing a copy at which a test can stop it, as a crash would.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Publication {
    /// The partial copy is complete and synced, not yet renamed.
    Written,
    /// The copy is renamed into place; its parent is not yet synced.
    Renamed,
}

/// The refusal while the harness is unavailable.
pub(crate) const HARNESS_UNAVAILABLE: &str =
    "the trusted harness was unavailable when tl3d started; install it, then restart tl3d";

const PARTIAL: &str = ".partial-";

impl Harness {
    pub(super) fn load(configured: &Path, copies: &Path) -> Self {
        Self::load_observed(configured, copies, &mut |_| {})
    }

    /// `after_read` runs after each module is read; tests switch `current` there.
    pub(super) fn load_observed(
        configured: &Path,
        copies: &Path,
        after_read: &mut dyn FnMut(&Path),
    ) -> Self {
        match Self::read(configured, after_read) {
            Ok(files) => Self::Loaded(Modules {
                copies: copies.to_owned(),
                digest: digest(&files),
                files,
            }),
            Err(error) => Self::unavailable(configured, error),
        }
    }

    // stage() takes only the harness directory's own *.py files, so they are what is read.
    fn read(
        configured: &Path,
        after_read: &mut dyn FnMut(&Path),
    ) -> std::io::Result<Vec<(std::ffi::OsString, Vec<u8>)>> {
        // Resolve `current` once; every module is then read from that immutable release.
        let release = configured.canonicalize()?;
        let mut names = vec![];
        for entry in fs::read_dir(&release)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|v| v == "py") && entry.file_type()?.is_file() {
                names.push(entry.file_name());
            }
        }
        if names.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no harness modules",
            ));
        }
        names.sort();
        let mut files = vec![];
        for name in names {
            let path = release.join(&name);
            files.push((name, fs::read(&path)?));
            after_read(&path);
        }
        Ok(files)
    }

    fn unavailable(configured: &Path, error: std::io::Error) -> Self {
        let reason = format!("{}: {error}", configured.display());
        eprintln!("PrismaBuild checks: {HARNESS_UNAVAILABLE} ({reason})");
        Self::Unavailable(reason)
    }

    /// The copy staging reads. Called only while staging, so only by the owning controller.
    pub(super) fn path(&mut self) -> Result<PathBuf> {
        self.path_with(&mut |_| Ok(()))
    }

    /// `interrupt` runs at each publication step; tests fail it there, as a crash would.
    pub(super) fn path_with(
        &mut self,
        interrupt: &mut dyn FnMut(Publication) -> std::io::Result<()>,
    ) -> Result<PathBuf> {
        if let Self::Loaded(modules) = self {
            *self = match modules.publish(interrupt) {
                Ok(copy) => Self::Copied(copy),
                Err(error) => Self::unavailable(&modules.copies, error),
            };
        }
        match self {
            Self::Copied(copy) => Ok(copy.clone()),
            _ => Err(Error::Invalid(HARNESS_UNAVAILABLE)),
        }
    }
}

fn digest(files: &[(std::ffi::OsString, Vec<u8>)]) -> String {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    for (name, bytes) in files {
        digest.update((name.len() as u64).to_be_bytes());
        digest.update(name.as_encoded_bytes());
        digest.update((bytes.len() as u64).to_be_bytes());
        digest.update(bytes);
    }
    format!("{:x}", digest.finalize())
}

fn sync_directory(path: &Path) -> std::io::Result<()> {
    File::open(path)?.sync_all()
}

impl Modules {
    // Publish once, under ownership: verify or write <copies>/<digest>, then sweep the rest.
    fn publish(
        &self,
        interrupt: &mut dyn FnMut(Publication) -> std::io::Result<()>,
    ) -> std::io::Result<PathBuf> {
        fs::create_dir_all(&self.copies)?;
        // A partial copy here was abandoned by an earlier process: only the owner writes here.
        for entry in fs::read_dir(&self.copies)? {
            let entry = entry?;
            if entry.file_name().to_string_lossy().starts_with(PARTIAL)
                && entry.file_type()?.is_dir()
            {
                fs::remove_dir_all(entry.path())?;
            }
        }
        let copy = self.copies.join(&self.digest);
        if !self.verified(&copy)? {
            if copy.is_dir() {
                fs::remove_dir_all(&copy)?;
            } else if copy.symlink_metadata().is_ok() {
                fs::remove_file(&copy)?;
            }
            let partial = self.copies.join(format!(
                "{PARTIAL}{}-{}",
                std::process::id(),
                &self.digest[..16]
            ));
            let written = self.write(&partial, &copy, interrupt);
            if written.is_err() {
                let _ = fs::remove_dir_all(&partial);
            }
            written?;
        }
        // Only the current copy is kept: no other controller owns this directory.
        for entry in fs::read_dir(&self.copies)? {
            let entry = entry?;
            if entry.path() == copy {
                continue;
            }
            if entry.file_type()?.is_dir() {
                fs::remove_dir_all(entry.path())?;
            } else {
                fs::remove_file(entry.path())?;
            }
        }
        sync_directory(&self.copies)?;
        Ok(copy)
    }

    fn write(
        &self,
        partial: &Path,
        copy: &Path,
        interrupt: &mut dyn FnMut(Publication) -> std::io::Result<()>,
    ) -> std::io::Result<()> {
        use std::io::Write;
        fs::create_dir(partial)?;
        for (name, bytes) in &self.files {
            let mut file = File::create(partial.join(name))?;
            file.write_all(bytes)?;
            file.sync_all()?;
        }
        sync_directory(partial)?;
        interrupt(Publication::Written)?;
        fs::rename(partial, copy)?;
        interrupt(Publication::Renamed)?;
        sync_directory(&self.copies)
    }

    // An existing copy is reused only if it holds exactly these modules.
    fn verified(&self, copy: &Path) -> std::io::Result<bool> {
        if !copy.is_dir() {
            return Ok(false);
        }
        let mut files = vec![];
        for entry in fs::read_dir(copy)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                return Ok(false);
            }
            files.push((entry.file_name(), fs::read(entry.path())?));
        }
        files.sort();
        Ok(digest(&files) == self.digest)
    }
}

const MAX_REPORT_LINE_BYTES: usize = 4 * 1024 * 1024;

pub(crate) fn validate_plan_size(plan: &[Check]) -> Result<()> {
    let tests: BTreeSet<_> = plan.iter().flat_map(|check| &check.tests).collect();
    let collection = serde_json::to_vec(&serde_json::json!({"event":"collection", "tests":tests}))?;
    // Python's event encoding adds three object spaces, list separators and a newline.
    let collection_bytes = collection.len() + 4 + tests.len().saturating_sub(1);
    let oversized_phase = tests.iter().try_fold(false, |oversized, test| {
        serde_json::to_vec(test)
            .map(|bytes| oversized || bytes.len() + 8192 > MAX_REPORT_LINE_BYTES)
    })?;
    if collection_bytes > MAX_REPORT_LINE_BYTES || oversized_phase {
        return Err(Error::Invalid(
            "Check plan exceeds the 4194304-byte report limit; reduce the selected tests or parameter cases.",
        ));
    }
    let mut result_bytes =
        65536 + crate::findings::MAX_FAILED_TESTS * crate::findings::MAX_MESSAGE_CHARS * 12 + 2;
    for check in plan {
        let mut header = serde_json::json!({
            "name":check.name, "kind":check.kind, "custody":"in-process",
            "verdict":"infrastructure-failure", "tests":[],
        });
        if let Some(forcing) = &check.forcing {
            // A forced check's result echoes its forcing, and each test gains head_forced (#393).
            header["forcing"] = serde_json::to_value(forcing)?;
        }
        if matches!(check.kind, Kind::Demonstrating | Kind::Reproducing) {
            result_bytes += 65536;
            if let Some(proof) = &check.proof {
                result_bytes += ascii_json_bytes(&serde_json::to_value(proof)?)?;
            }
        }
        if let Some(symptom) = &check.declared_symptom {
            header["declared_symptom"] = serde_json::to_value(symptom)?;
            // Up to thirty reports, each with a bounded pytest traceback and identity.
            result_bytes += 384 * 1024;
        }
        result_bytes += ascii_json_bytes(&header)? + 1;
        for test in &check.tests {
            let mut case = serde_json::json!({
                "test":test, "base":"uncollected", "head":"uncollected",
                "pre_existing":false, "passed":false, "message":"",
            });
            if check.forcing.is_some() {
                case["head_forced"] = "uncollected".into();
            }
            result_bytes += ascii_json_bytes(&case)? + 1;
            // Match the harness reservation: three bounded phase facts and the concrete node per side.
            let sides = if check.forcing.is_some() { 3 } else { 2 };
            result_bytes +=
                sides * (3 * (200 * 12 + 100) + 100 + ascii_json_bytes(&serde_json::json!(test))?);
        }
        if result_bytes > super::results::MAX_RESULT_BYTES {
            return Err(Error::Invalid(
                "Check plan exceeds the 8388608-byte result limit; reduce the selected tests or checks.",
            ));
        }
    }
    Ok(())
}

fn ascii_json_bytes(value: &serde_json::Value) -> Result<usize> {
    Ok(serde_json::to_string(value)?
        .chars()
        .map(|ch| if ch.is_ascii() { 1 } else { 6 * ch.len_utf16() })
        .sum())
}

/// The plan with its base moved forward to the checkout's recorded base when the
/// head descends from it. A plan can keep an older base, as a restarted request
/// once inherited its predecessor's; checks must compare the head with the base
/// it actually sits on (#326). Rebases move the recorded base with the head.
pub(super) fn descended_base(delivery: &Delivery) -> Result<CheckRun> {
    let mut run = delivery
        .checks
        .clone()
        .ok_or(Error::Invalid("missing check plan"))?;
    let Some(head) = delivery.head_sha.as_deref() else {
        return Ok(run);
    };
    let head = CommitId::new(head)?;
    let checkout = Worktrees::open(&run.repository, &run.repository)?;
    if let Some(base) = checkout.checked_out_base()?
        && base != run.base_sha
        && checkout.is_ancestor(&run.base_sha, &base)?
        && checkout.is_ancestor(&base, &head)?
    {
        run.base_sha = base;
    }
    Ok(run)
}

pub(super) fn stage(
    root: &Path,
    harness: &Path,
    delivery: &Delivery,
    image: &str,
    protected_paths: &[String],
    tests: &super::super::images::AdditionalTests,
) -> Result<Vec<String>> {
    let run = delivery
        .checks
        .as_ref()
        .ok_or(Error::Invalid("missing recorded check plan and base"))?;
    let head = delivery
        .head_sha
        .as_deref()
        .ok_or(Error::Invalid("checks require an exact head"))?;
    let head = CommitId::new(head)?;
    let base = &run.base_sha;
    let reviewed = run.reviewed_sha.as_ref();
    if (run.plan.iter().any(|c| c.kind == Kind::Reproducing) && reviewed.is_none())
        || !run.repository.is_absolute()
        || run.plan.is_empty()
        || run
            .plan
            .iter()
            .any(|c| c.name.is_empty() || c.tests.is_empty())
    {
        return Err(Error::Invalid("invalid check plan, repository or commit"));
    }
    if harness
        .canonicalize()?
        .starts_with(run.repository.canonicalize()?)
    {
        return Err(Error::Invalid(
            "install the trusted harness outside the served repository",
        ));
    }
    if root.exists() {
        fs::remove_dir_all(root)?;
    }
    fs::create_dir_all(root)?;
    // Never snapshot the maker's changing worktree. Clone committed objects into our private checkout.
    let repository = Worktrees::open(&run.repository, &run.repository)?;
    let served = root.join("served");
    let checkout = repository.clone_checkout(&served, &head)?;
    // The adopted head can diverge from its durable main base.
    checkout.import_commit(&run.repository, base)?;
    if checkout.verify_commit(base)? != *base {
        return Err(Error::Invalid("checks require an exact base"));
    }
    let changed = checkout
        .changed_paths_since_merge_base(base, &head)?
        .into_iter()
        .map(|path| {
            path.into_os_string()
                .into_string()
                .map_err(|_| Error::Invalid("changed path must be UTF-8"))
        })
        .collect::<Result<Vec<_>>>()?;
    checkout.bundle_check_input(&root.join("served.bundle"), base, reviewed)?;
    fs::remove_dir_all(served)?;
    let trusted = root.join("trusted/tl3_harness");
    fs::create_dir_all(&trusted)?;
    for entry in fs::read_dir(harness)? {
        let entry = entry?;
        if entry.path().extension().is_some_and(|v| v == "py") && entry.file_type()?.is_file() {
            fs::copy(entry.path(), trusted.join(entry.file_name()))?;
        }
    }
    fs::write(root.join("action.py"), include_str!("action.py"))?;
    let commit = |id: &CommitId| {
        CommitSha::new(id.as_str()).map_err(|_| Error::Invalid("checks require an exact commit"))
    };
    let request = CheckRequest {
        effect_key: delivery.key.clone(),
        base: commit(&run.base_sha)?,
        head: commit(&head)?,
        reviewed: run.reviewed_sha.as_ref().map(commit).transpose()?,
        plan: run.plan.clone(),
        image: image.to_owned(),
        protected_paths: protected_paths.to_vec(),
        selected_failures: run.selected_failures.clone(),
        always_tests: tests.always.clone(),
        selected_tests: tests.selected,
    };
    // Through a Value, so the keys stay sorted and the staged bytes stay as they were.
    let request = serde_json::to_value(request)?;
    fs::write(root.join("request.json"), serde_json::to_vec(&request)?)?;
    Worktrees::commit_check_input(root)?;
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn check_plan_base_rejects_uppercase_at_the_edge() {
        for length in [40, 64] {
            let mut value =
                serde_json::to_value(crate::git_tests::Repo::new().delivery().checks.unwrap())
                    .unwrap();
            value["base_sha"] = serde_json::json!("A".repeat(length));
            assert!(
                serde_json::from_value::<CheckRun>(value).is_err(),
                "reject uppercase check base before staging"
            );
        }
    }

    #[test]
    fn check_plan_reviewed_rejects_uppercase_at_the_edge() {
        for length in [40, 64] {
            let mut value =
                serde_json::to_value(crate::git_tests::Repo::new().delivery().checks.unwrap())
                    .unwrap();
            value["reviewed_sha"] = serde_json::json!("A".repeat(length));
            assert!(
                serde_json::from_value::<CheckRun>(value).is_err(),
                "reject uppercase reviewed commit before staging"
            );
        }
    }

    // Two releases laid out as deploy.py's install_release lays them out, each with two
    // harness modules, so a copy that mixes releases shows.
    fn releases(root: &Path) -> PathBuf {
        for release in ["a", "b"] {
            let harness = root
                .join("releases")
                .join(release)
                .join("harness/src/tl3_harness");
            fs::create_dir_all(&harness).unwrap();
            for module in ["first.py", "release.py"] {
                fs::write(harness.join(module), format!("RELEASE = {release:?}\n")).unwrap();
            }
        }
        std::os::unix::fs::symlink(root.join("releases/a"), root.join("current")).unwrap();
        root.join("current/harness/src/tl3_harness")
    }

    // install_release: link the new release beside `current`, then replace it.
    fn switch_current(root: &Path, release: &str) {
        let next = root.join("current.tmp");
        std::os::unix::fs::symlink(root.join("releases").join(release), &next).unwrap();
        fs::rename(&next, root.join("current")).unwrap();
    }

    // What one staging of `harness` ships: each trusted module's release.
    fn staged_release(
        f: &crate::git_tests::Repo,
        harness: &mut Harness,
        name: &str,
    ) -> Result<Vec<String>> {
        let staged = f.root.path().join(name);
        stage(
            &staged,
            &harness.path()?,
            &f.delivery(),
            "sha256:fixture",
            &[],
            &Default::default(),
        )?;
        ["first.py", "release.py"]
            .iter()
            .map(|module| {
                Ok(fs::read_to_string(
                    staged.join("trusted/tl3_harness").join(module),
                )?)
            })
            .collect()
    }

    fn release(name: &str) -> Vec<String> {
        vec![format!("RELEASE = {name:?}\n"); 2]
    }

    fn listing(directory: &Path) -> Vec<String> {
        let mut names: Vec<_> = fs::read_dir(directory)
            .map(|entries| {
                entries
                    .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    /// Encodes: #750 review finding 2 and re-review finding 1. install_release switches
    /// `current` without restarting tl3d, and collect_releases may then delete the release a
    /// running controller started from. The controller stages its own copy, so neither the
    /// switch nor the deletion changes what it stages.
    #[test]
    fn a_running_controller_stages_its_own_copy_of_its_release() {
        let f = crate::git_tests::Repo::new();
        let root = f.root.path().join("tl3");
        let configured = releases(&root);
        let copies = f.root.path().join("records-harness");
        let mut running = Harness::load(&configured, &copies);
        assert_eq!(
            staged_release(&f, &mut running, "input-first").unwrap(),
            release("a")
        );
        switch_current(&root, "b");
        fs::remove_dir_all(root.join("releases/a")).unwrap();
        assert_eq!(
            staged_release(&f, &mut running, "input-running").unwrap(),
            release("a"),
            "release a was switched away and collected, and the running controller still stages it"
        );
        let mut restarted = Harness::load(&configured, &copies);
        assert_eq!(
            staged_release(&f, &mut restarted, "input-restarted").unwrap(),
            release("b"),
            "after the restart the controller copies the new release"
        );
        assert_eq!(
            listing(&copies).len(),
            1,
            "the owner keeps only its own copy"
        );
    }

    /// Encodes: #750 re-review finding 2. A controller that started before any harness was
    /// installed stays unavailable; it never stages whatever `current` later points to.
    #[test]
    fn a_harness_missing_at_startup_stays_unavailable() {
        let f = crate::git_tests::Repo::new();
        let root = f.root.path().join("tl3");
        let configured = root.join("current/harness/src/tl3_harness");
        let copies = f.root.path().join("records-harness");
        let mut started = Harness::load(&configured, &copies);
        assert!(matches!(started, Harness::Unavailable(_)), "{started:?}");
        releases(&root);
        assert!(
            matches!(
                staged_release(&f, &mut started, "input-late"),
                Err(Error::Invalid(HARNESS_UNAVAILABLE))
            ),
            "an installed harness does not reach a controller that started without one"
        );
    }

    /// Encodes: #750 re-review finding 2: a copy that fails is the same unavailable state,
    /// not a fallback to the configured path, and it stays so.
    #[test]
    fn a_failed_copy_is_unavailable() {
        let f = crate::git_tests::Repo::new();
        let root = f.root.path().join("tl3");
        let configured = releases(&root);
        let copies = f.root.path().join("records-harness");
        fs::write(&copies, "not a directory").unwrap();
        let mut started = Harness::load(&configured, &copies);
        for attempt in ["input-failed", "input-again"] {
            assert!(matches!(
                staged_release(&f, &mut started, attempt),
                Err(Error::Invalid(HARNESS_UNAVAILABLE))
            ));
        }
        assert!(matches!(started, Harness::Unavailable(_)), "{started:?}");
    }

    /// Encodes: #750 round 3 finding 1. Adapters register before Controller::open claims
    /// ownership. A second controller that starts while the first runs, and never wins
    /// ownership, must not touch the copies directory: loading only reads the release.
    #[test]
    fn a_second_controller_never_touches_the_owners_copies() {
        let f = crate::git_tests::Repo::new();
        let root = f.root.path().join("tl3");
        let configured = releases(&root);
        let copies = f.root.path().join("records-harness");
        let mut owner = Harness::load(&configured, &copies);
        assert_eq!(
            staged_release(&f, &mut owner, "input-owner").unwrap(),
            release("a")
        );
        let before = listing(&copies);
        switch_current(&root, "b");
        let _second = Harness::load(&configured, &copies);
        assert_eq!(
            listing(&copies),
            before,
            "the second controller wrote to the copies"
        );
        assert_eq!(
            staged_release(&f, &mut owner, "input-owner-again").unwrap(),
            release("a"),
            "the owner still stages its own copy"
        );
    }

    /// Encodes: #750 round 3 finding 2. The release root is resolved once, before any module
    /// is read, so a `current` switch between module reads cannot mix two releases.
    #[test]
    fn a_current_switch_between_module_reads_never_mixes_releases() {
        let f = crate::git_tests::Repo::new();
        let root = f.root.path().join("tl3");
        let configured = releases(&root);
        let copies = f.root.path().join("records-harness");
        let mut switched = false;
        let mut harness = Harness::load_observed(&configured, &copies, &mut |_| {
            if !switched {
                switch_current(&root, "b");
                switched = true;
            }
        });
        assert!(switched, "the harness was read");
        assert_eq!(
            staged_release(&f, &mut harness, "input-mixed").unwrap(),
            release("a")
        );
    }

    /// Encodes: #750 round 3 finding 3. A digest directory left half-written (a crash after
    /// the rename, or damage) is verified before reuse and never staged as it is.
    #[test]
    fn a_truncated_copy_is_never_staged() {
        let f = crate::git_tests::Repo::new();
        let root = f.root.path().join("tl3");
        let configured = releases(&root);
        let copies = f.root.path().join("records-harness");
        let copy = Harness::load(&configured, &copies).path().unwrap();
        fs::write(copy.join("release.py"), "RELEA").unwrap();
        let mut restarted = Harness::load(&configured, &copies);
        assert_eq!(
            staged_release(&f, &mut restarted, "input-truncated").unwrap(),
            release("a"),
            "the damaged copy is rebuilt from the release"
        );
    }

    /// Encodes: #750 round 3 finding 4. Failed publications, and partial copies abandoned by
    /// earlier processes, never accumulate: the owner sweeps abandoned partials and removes
    /// its own on failure, and a success keeps only the current copy.
    #[test]
    fn repeated_failed_startups_leave_no_partial_copies() {
        let f = crate::git_tests::Repo::new();
        let root = f.root.path().join("tl3");
        let configured = releases(&root);
        let copies = f.root.path().join("records-harness");
        fs::create_dir_all(copies.join(".partial-4242-1")).unwrap();
        fs::write(copies.join(".partial-4242-1/first.py"), "abandoned").unwrap();
        for _ in 0..3 {
            let mut failing = Harness::load(&configured, &copies);
            let failed = failing.path_with(&mut |_| Err(std::io::Error::other("injected")));
            assert!(matches!(failed, Err(Error::Invalid(HARNESS_UNAVAILABLE))));
            assert!(
                listing(&copies)
                    .iter()
                    .all(|name| !name.starts_with(".partial-")),
                "partial copies left behind: {:?}",
                listing(&copies)
            );
        }
        let mut working = Harness::load(&configured, &copies);
        let copy = working.path().unwrap();
        assert_eq!(
            listing(&copies),
            [copy.file_name().unwrap().to_string_lossy().into_owned()]
        );
    }

    /// Encodes: #750 round 4 finding 2. A crash after the rename, or a failed fsync of the
    /// parent, leaves a completed copy behind. The next owner reclaims every completed copy
    /// except the one it is about to use, so interrupted startups across releases leave at
    /// most one.
    #[test]
    fn interrupted_startups_leave_at_most_one_completed_copy() {
        let f = crate::git_tests::Repo::new();
        let root = f.root.path().join("tl3");
        let configured = releases(&root);
        let copies = f.root.path().join("records-harness");
        for release in ["a", "b", "a", "b"] {
            switch_current(&root, release);
            let mut interrupted = Harness::load(&configured, &copies);
            let failed = interrupted.path_with(&mut |step| match step {
                Publication::Renamed => Err(std::io::Error::other("crashed after the rename")),
                Publication::Written => Ok(()),
            });
            assert!(matches!(failed, Err(Error::Invalid(HARNESS_UNAVAILABLE))));
            assert!(
                listing(&copies).len() <= 1,
                "completed copies pile up: {:?}",
                listing(&copies)
            );
        }
        let mut working = Harness::load(&configured, &copies);
        assert_eq!(
            staged_release(&f, &mut working, "input-after").unwrap(),
            release("b")
        );
        assert_eq!(listing(&copies).len(), 1);
    }

    #[test]
    fn hostile_check_stage() {
        crate::git_tests::hostile(
            concat!(module_path!(), "::hostile_check_stage").trim_start_matches("tl3_controller::"),
            || {
                let f = crate::git_tests::Repo::new();
                let staged = f.root.path().join("input");
                let harness = f.root.path().join("harness");
                fs::create_dir(&harness).unwrap();
                fs::write(harness.join("__init__.py"), "").unwrap();
                let mut delivery = f.delivery();
                delivery.checks.as_mut().unwrap().selected_failures =
                    vec!["tests/test_old.py".into()];
                let changed = stage(
                    &staged,
                    &harness,
                    &delivery,
                    "sha256:fixture",
                    &[],
                    &Default::default(),
                )
                .unwrap();
                assert_eq!(changed, ["README.md"]);
                assert!(staged.join("served.bundle").is_file());
                let request: serde_json::Value =
                    serde_json::from_slice(&fs::read(staged.join("request.json")).unwrap())
                        .unwrap();
                assert_eq!(
                    request["selected_failures"],
                    serde_json::json!(["tests/test_old.py"])
                );
                assert_eq!(
                    crate::git_tests::git(&staged, &["show", "HEAD:request.json"]),
                    fs::read_to_string(staged.join("request.json")).unwrap()
                );
            },
        );
    }
}
