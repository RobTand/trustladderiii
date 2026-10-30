use crate::{
    Error, Result,
    adapters::prismabuild::{CheckRun, Kind},
};
use std::{path::Path, process::Command};
use tl3_worktrees::BranchName;

fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let result = Command::new("git")
        .current_dir(repo)
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
            "--no-replace-objects",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()?;
    if !result.status.success() {
        return Err(Error::Invalid(
            "cannot replay the reviewed tree on the new base",
        ));
    }
    Ok(String::from_utf8(result.stdout)
        .map_err(|_| Error::Invalid("Git output must be UTF-8"))?
        .trim()
        .into())
}

fn prefix(branch: &BranchName) -> String {
    // Heads are fetched by the private check/review clones; arbitrary refs are not.
    format!("refs/heads/tl3-reviewed/{}/", branch.as_str())
}

pub(super) fn pin(repo: &Path, branch: &BranchName, reviewed: &str) -> Result<()> {
    git(
        repo,
        &[
            "update-ref",
            &format!("{}{reviewed}", prefix(branch)),
            reviewed,
        ],
    )?;
    Ok(())
}

pub(super) fn rebase(repo: &Path, branch: &BranchName, run: &mut CheckRun, target: &str) {
    let Some(reviewed) = run.reviewed_sha.as_deref() else {
        return;
    };
    let replay = || -> Result<String> {
        // Replay the reviewed patch without touching the maker's index or worktree.
        let tree = git(
            repo,
            &[
                "merge-tree",
                "--write-tree",
                &format!("--merge-base={}", run.base_sha),
                target,
                reviewed,
            ],
        )?;
        let sha = git(
            repo,
            &[
                "commit-tree",
                &tree,
                "-p",
                target,
                "-m",
                "Replay reviewed state for reproducing checks",
            ],
        )?;
        pin(repo, branch, &sha)?;
        Ok(sha)
    };
    match replay() {
        Ok(sha) => run.reviewed_sha = Some(sha),
        Err(_) => {
            run.reviewed_sha = None;
            run.plan.retain(|check| check.kind != Kind::Reproducing);
            run.reproducing_unavailable = Some("Reproducing checks were dropped because the reviewed tree could not be replayed on the new base. Judge this reason under each original finding's goal or standing rule.".into());
        }
    }
}

pub(super) fn remove(repo: &Path, branch: &BranchName) -> Result<()> {
    for reference in git(
        repo,
        &["for-each-ref", "--format=%(refname)", &prefix(branch)],
    )?
    .lines()
    {
        git(repo, &["update-ref", "-d", reference])?;
    }
    Ok(())
}
