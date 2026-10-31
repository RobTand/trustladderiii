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
            unavailable(
                run,
                "the reviewed tree could not be replayed on the new base",
            );
        }
    }
}

pub(super) fn unavailable(run: &mut CheckRun, reason: &str) {
    run.reviewed_sha = None;
    run.plan.retain(|check| check.kind != Kind::Reproducing);
    run.reproducing_unavailable = Some(format!(
        "Reproducing checks were dropped because {reason}. Judge this reason under each original finding's goal or standing rule."
    ));
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, process::Command};

    #[test]
    fn hostile_environment_cannot_redirect_reviewed_pin() {
        if let Some(repo) = std::env::var_os("TL3_PIN_REPO") {
            pin(
                Path::new(&repo),
                &BranchName::new("tl3/test").unwrap(),
                &std::env::var("TL3_PIN_SHA").unwrap(),
            )
            .unwrap();
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        let foreign = temp.path().join("foreign");
        fs::create_dir(&repo).unwrap();
        let git = |path: &Path, args: &[&str]| {
            let out = Command::new("git")
                .current_dir(path)
                .env_clear()
                .env("PATH", "/usr/local/bin:/usr/bin:/bin")
                .args([
                    "-c",
                    "user.name=Fixture",
                    "-c",
                    "user.email=fixture@example.invalid",
                    "-c",
                    "core.hooksPath=/dev/null",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8(out.stdout).unwrap().trim().to_owned()
        };
        git(&repo, &["init", "--quiet"]);
        git(
            &repo,
            &["commit", "--allow-empty", "--quiet", "-m", "fixture"],
        );
        let sha = git(&repo, &["rev-parse", "HEAD"]);
        git(
            temp.path(),
            &[
                "clone",
                "--quiet",
                repo.to_str().unwrap(),
                foreign.to_str().unwrap(),
            ],
        );
        let result = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "adapters::worktrees::reproducing::tests::hostile_environment_cannot_redirect_reviewed_pin", "--nocapture"])
            .env("TL3_PIN_REPO", &repo).env("TL3_PIN_SHA", &sha)
            .env("GIT_DIR", foreign.join(".git"))
            .env("GIT_WORK_TREE", &foreign)
            .env("GIT_INDEX_FILE", foreign.join("hostile-index"))
            .env("GIT_CONFIG_COUNT", "1")
            .env("GIT_CONFIG_KEY_0", "protocol.allow")
            .env("GIT_CONFIG_VALUE_0", "always")
            .output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stdout)
        );
        let refs = git(
            &repo,
            &[
                "for-each-ref",
                "--format=%(objectname)",
                "refs/heads/tl3-reviewed/",
            ],
        );
        assert_eq!(
            refs, sha,
            "reviewed pin must be written to the intended repository"
        );
        assert!(
            git(
                &foreign,
                &[
                    "for-each-ref",
                    "--format=%(refname)",
                    "refs/heads/tl3-reviewed/"
                ]
            )
            .is_empty(),
            "foreign repository must remain unchanged"
        );
    }
}
