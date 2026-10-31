use crate::{Error, Result, executor::Delivery};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};
use tl3_worktrees::{CommitId, Worktrees};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CheckRun {
    pub repository: PathBuf,
    pub base_sha: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewed_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reproducing_unavailable: Option<String>,
    pub plan: Vec<Check>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Check {
    pub name: String,
    pub kind: Kind,
    pub tests: Vec<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Demonstrating,
    Regression,
    BehaviorPreserving,
    Reproducing,
}

pub(super) fn stage(
    root: &Path,
    harness: &Path,
    delivery: &Delivery,
    image: &str,
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
    let base = CommitId::new(run.base_sha.clone())?;
    let reviewed = run
        .reviewed_sha
        .as_ref()
        .map(|sha| CommitId::new(sha.clone()))
        .transpose()?;
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
    if checkout.verify_commit(&base)? != base {
        return Err(Error::Invalid("checks require an exact base"));
    }
    let changed = checkout
        .changed_paths_since_merge_base(&base, &head)?
        .into_iter()
        .map(|path| {
            path.into_os_string()
                .into_string()
                .map_err(|_| Error::Invalid("changed path must be UTF-8"))
        })
        .collect::<Result<Vec<_>>>()?;
    checkout.bundle_check_input(&root.join("served.bundle"), &base, reviewed.as_ref())?;
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
    fs::write(
        root.join("request.json"),
        serde_json::to_vec(&serde_json::json!({
            "effect_key": delivery.key, "base": run.base_sha, "head": head.as_str(),
            "reviewed": run.reviewed_sha,
            "plan": run.plan, "image": image,
        }))?,
    )?;
    Worktrees::commit_check_input(root)?;
    Ok(changed)
}
