use crate::{
    Result,
    adapters::prismabuild::{CheckRun, Kind},
};
use std::path::Path;
use tl3_worktrees::{BranchName, CommitId, Worktrees};

pub(super) fn pin(repo: &Path, branch: &BranchName, reviewed: &CommitId) -> Result<()> {
    Worktrees::open(repo, repo)?.pin_reviewed(branch, reviewed)?;
    Ok(())
}

pub(super) fn rebase(repo: &Path, branch: &BranchName, run: &mut CheckRun, target: &str) {
    let Some(reviewed) = run.reviewed_sha.as_ref() else {
        return;
    };
    let replay = || -> Result<CommitId> {
        let target = CommitId::new(target)?;
        Worktrees::open(repo, repo)?
            .replay_reviewed(branch, &run.base_sha, &target, reviewed)
            .map_err(Into::into)
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
    Worktrees::open(repo, repo)?.remove_reviewed(branch)?;
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
                &CommitId::new(std::env::var("TL3_PIN_SHA").unwrap()).unwrap(),
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
