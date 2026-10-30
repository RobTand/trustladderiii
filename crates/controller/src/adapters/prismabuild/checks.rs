use crate::{Error, Result, executor::Delivery};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CheckRun {
    pub repository: PathBuf,
    pub base_sha: String,
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
}

pub(super) fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git")
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_AUTHOR_NAME", "TL3")
        .env("GIT_AUTHOR_EMAIL", "tl3@example.invalid")
        .env("GIT_COMMITTER_NAME", "TL3")
        .env("GIT_COMMITTER_EMAIL", "tl3@example.invalid")
        .env("GIT_AUTHOR_DATE", "2000-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2000-01-01T00:00:00Z")
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()?;
    if !output.status.success() {
        return Err(Error::Invalid("check input Git operation failed"));
    }
    Ok(output.stdout)
}

fn sha(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
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
    if !sha(head)
        || !sha(&run.base_sha)
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
    git(
        root,
        &[
            "clone",
            "--quiet",
            "--no-local",
            "--no-checkout",
            "--template=",
            "--",
            run.repository
                .to_str()
                .ok_or(Error::Invalid("repository path must be UTF-8"))?,
            "served",
        ],
    )?;
    let served = root.join("served");
    git(&served, &["checkout", "--quiet", "--detach", head])?;
    let base = git(
        &served,
        &[
            "rev-parse",
            "--verify",
            &format!("{}^{{commit}}", run.base_sha),
        ],
    )?;
    if base != format!("{}\n", run.base_sha).as_bytes()
        || !git(&served, &["status", "--porcelain", "--untracked-files=all"])?.is_empty()
    {
        return Err(Error::Invalid(
            "checks require a clean checkout and exact base",
        ));
    }
    let paths = git(
        &served,
        &[
            "diff",
            "--no-ext-diff",
            "--no-renames",
            "--name-only",
            "-z",
            &run.base_sha,
            head,
            "--",
        ],
    )?;
    let changed = paths
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| {
            String::from_utf8(p.to_vec()).map_err(|_| Error::Invalid("changed path must be UTF-8"))
        })
        .collect::<Result<Vec<_>>>()?;
    // Both commits must survive PB materialization, even when base is not head's ancestor.
    git(
        &served,
        &["update-ref", "refs/heads/tl3-base", &run.base_sha],
    )?;
    git(
        &served,
        &[
            "bundle",
            "create",
            "../served.bundle",
            "HEAD",
            "refs/heads/tl3-base",
        ],
    )?;
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
            "effect_key": delivery.key, "base": run.base_sha, "head": head,
            "plan": run.plan, "image": image,
        }))?,
    )?;
    git(root, &["init", "--quiet", "--template="])?;
    git(root, &["add", "--force", "--all"])?;
    git(
        root,
        &["commit", "--quiet", "-m", "Freeze controller check input"],
    )?;
    Ok(changed)
}
