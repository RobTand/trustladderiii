use super::*;
use tempfile::TempDir;

const TOKEN: &str = "tl3_FAKE_INSTALLATION_SECRET_7x9";

struct Fixture {
    _temp: TempDir,
    repo: PathBuf,
    remote: Remote,
    bare: PathBuf,
    worktree: PathBuf,
    manager: Worktrees,
    name: BranchName,
    token: InstallationToken,
    upstream_emits_secret: bool,
}

impl Fixture {
    fn new() -> Self {
        let temp = TempDir::new().unwrap();
        let repo = temp.path().join("repo");
        let bare = temp.path().join("remote.git");
        let worktree = temp.path().join("leaf\nwith spaces");
        fs::create_dir(&repo).unwrap();
        git(&repo, ["init", "--initial-branch=main"]);
        git(&repo, ["config", "user.name", "Test"]);
        git(&repo, ["config", "user.email", "test@localhost"]);
        write_commit(&repo, "shared.txt", "base\n", "base");
        git(
            &repo,
            [OsStr::new("init"), OsStr::new("--bare"), bare.as_os_str()],
        );
        let manager = Worktrees::open(&repo).unwrap();
        Self {
            remote: Remote::local(&bare).unwrap(),
            manager,
            repo,
            bare,
            worktree,
            name: BranchName::new("tl3/issue-29").unwrap(),
            token: InstallationToken::new(TOKEN).unwrap(),
            upstream_emits_secret: false,
            _temp: temp,
        }
    }

    fn checkout(&self) -> Branch {
        let branch = self.manager.create_branch(&self.name).unwrap();
        self.manager
            .create_worktree(&self.name, &self.worktree)
            .unwrap();
        branch
    }

    fn assert_clean(&self) {
        assert!(
            git(
                &self.worktree,
                ["status", "--porcelain=v1", "--untracked-files=all"]
            )
            .is_empty()
        );
        assert!(!self.manager.rebase_active(&self.worktree).unwrap());
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        let encoded = STANDARD.encode(format!("x-access-token:{TOKEN}"));
        let invocations = self.manager.invocations.borrow();
        assert!(!invocations.is_empty());
        // Every Git process launched by the adapter passes this one execution boundary.
        for invocation in invocations.iter() {
            for argument in &invocation.argv {
                assert_no_secret(argument.as_bytes(), &encoded);
            }
            if !self.upstream_emits_secret {
                assert_no_secret(&invocation.stdout, &encoded);
                assert_no_secret(&invocation.stderr, &encoded);
            }
            for (key, value) in &invocation.environment {
                assert!(!key.as_bytes().starts_with(b"GIT_TRACE"));
                if key == "GIT_CONFIG_VALUE_2" {
                    assert_eq!(
                        value.as_deref(),
                        Some(OsStr::new(&format!("Authorization: Basic {encoded}")))
                    );
                }
            }
        }
        for root in [&self.manager.common, &self.bare] {
            for name in ["config", "logs/HEAD"] {
                if let Ok(bytes) = fs::read(root.join(name)) {
                    assert_no_secret(&bytes, &encoded);
                }
            }
        }
        let urls = git(&self.repo, ["remote", "-v"]);
        assert_no_secret(&urls, &encoded);
    }
}

fn assert_no_secret(bytes: &[u8], encoded: &str) {
    for secret in [TOKEN.as_bytes(), encoded.as_bytes()] {
        assert!(
            !bytes.windows(secret.len()).any(|w| w == secret),
            "credential leaked"
        );
    }
}

fn git<I, S>(path: &Path, args: I) -> Vec<u8>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = raw_git(path, args);
    assert!(
        output.status.success(),
        "fixture git failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn raw_git<I, S>(path: &Path, args: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new("git");
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_EDITOR", "true")
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(path);
    let encoded = STANDARD.encode(format!("x-access-token:{TOKEN}"));
    for argument in command.get_args() {
        assert_no_secret(argument.as_bytes(), &encoded);
    }
    command.output().unwrap()
}

fn write_commit(repo: &Path, file: &str, contents: &str, message: &str) {
    fs::write(repo.join(file), contents).unwrap();
    git(repo, ["add", "--all"]);
    git(repo, ["commit", "-m", message]);
}

#[test]
fn branch_base_is_durable_across_retries_and_main_advances() {
    let f = Fixture::new();
    let branch = f.manager.create_branch(&f.name).unwrap();
    write_commit(&f.repo, "main.txt", "main advance\n", "advance main");
    let reopened = Worktrees::open(&f.repo).unwrap();
    assert_eq!(reopened.create_branch(&f.name).unwrap(), branch);
    assert_eq!(f.manager.head(&f.name).unwrap(), branch.base);
    assert_ne!(f.manager.resolve("refs/heads/main").unwrap(), branch.base);
}

#[test]
fn existing_branch_without_base_or_worktree_converges() {
    let f = Fixture::new();
    let base = f.manager.resolve("refs/heads/main").unwrap();
    git(&f.repo, ["branch", f.name.as_str()]);
    write_commit(&f.repo, "main.txt", "new main\n", "advance main");
    let branch = f.checkout();
    assert_eq!(branch.base, base);
    assert_eq!(f.manager.create_branch(&f.name).unwrap(), branch);
    assert_eq!(
        f.manager.create_worktree(&f.name, &f.worktree).unwrap(),
        f.worktree
    );
    f.assert_clean();
}

#[test]
fn base_without_branch_recreates_the_original_branch() {
    let f = Fixture::new();
    let branch = f.manager.create_branch(&f.name).unwrap();
    git(&f.repo, ["update-ref", "-d", &f.name.reference()]);
    write_commit(&f.repo, "main.txt", "new main\n", "advance main");
    assert_eq!(f.manager.create_branch(&f.name).unwrap(), branch);
    assert_eq!(f.manager.head(&f.name).unwrap(), branch.base);
}

#[test]
fn empty_unregistered_directory_and_missing_registered_directory_converge() {
    let f = Fixture::new();
    fs::create_dir(&f.worktree).unwrap();
    f.checkout();
    fs::remove_dir_all(&f.worktree).unwrap();
    f.manager.create_worktree(&f.name, &f.worktree).unwrap();
    f.assert_clean();
    assert_eq!(
        f.manager
            .checkouts()
            .unwrap()
            .iter()
            .filter(|c| c.path == f.worktree)
            .count(),
        1
    );
}

#[test]
fn lost_registration_is_rebuilt_without_discarding_files() {
    let f = Fixture::new();
    f.checkout();
    let admin = gitlink(&fs::read(f.worktree.join(".git")).unwrap()).unwrap();
    fs::remove_dir_all(admin).unwrap();
    fs::write(f.worktree.join("shared.txt"), "uncommitted work\n").unwrap();
    f.manager.create_worktree(&f.name, &f.worktree).unwrap();
    assert_eq!(
        fs::read_to_string(f.worktree.join("shared.txt")).unwrap(),
        "uncommitted work\n"
    );
    assert_eq!(
        f.manager
            .checkouts()
            .unwrap()
            .iter()
            .filter(|c| c.path == f.worktree)
            .count(),
        1
    );
    assert!(matches!(
        f.manager.remove_worktree(&f.name, &f.worktree),
        Err(Error::DirtyWorktree)
    ));
    assert!(f.worktree.exists());
}

#[test]
fn interrupted_orphan_registration_converges() {
    for phase in 0..4 {
        let f = Fixture::new();
        f.checkout();
        let admin = gitlink(&fs::read(f.worktree.join(".git")).unwrap()).unwrap();
        fs::remove_dir_all(admin).unwrap();
        let stage = f
            .manager
            .common
            .join("tl3-worktree-staging")
            .join(f.name.as_str());
        fs::create_dir_all(stage.parent().unwrap()).unwrap();
        git(
            &f.repo,
            [
                OsStr::new("worktree"),
                OsStr::new("add"),
                OsStr::new("--no-checkout"),
                stage.as_os_str(),
                OsStr::new(f.name.as_str()),
            ],
        );
        if phase >= 1 {
            fs::copy(stage.join(".git"), f.worktree.join(".git")).unwrap();
        }
        if phase >= 2 {
            git(&stage, ["read-tree", &f.name.reference()]);
            git(
                &f.repo,
                [
                    OsStr::new("worktree"),
                    OsStr::new("repair"),
                    f.worktree.as_os_str(),
                ],
            );
        }
        if phase >= 3 {
            fs::remove_file(stage.join(".git")).unwrap();
        }
        f.manager.create_worktree(&f.name, &f.worktree).unwrap();
        f.assert_clean();
        assert!(!stage.exists());
    }
}

#[test]
fn clean_removal_and_branch_deletion_are_repeatable() {
    let f = Fixture::new();
    f.checkout();
    assert!(matches!(
        f.manager.delete_local_branch(&f.name),
        Err(Error::WorktreeOccupied)
    ));
    f.manager.remove_worktree(&f.name, &f.worktree).unwrap();
    f.manager.remove_worktree(&f.name, &f.worktree).unwrap();
    assert!(!f.worktree.exists());
    f.manager.delete_local_branch(&f.name).unwrap();
    f.manager.delete_local_branch(&f.name).unwrap();
    assert!(
        f.manager
            .resolve_optional(&f.name.reference())
            .unwrap()
            .is_none()
    );
    assert!(
        f.manager
            .resolve_optional(&f.name.base_ref())
            .unwrap()
            .is_none()
    );
}

#[test]
fn removal_recovers_a_missing_directory_or_lost_registration() {
    let f = Fixture::new();
    f.checkout();
    fs::remove_dir_all(&f.worktree).unwrap();
    f.manager.remove_worktree(&f.name, &f.worktree).unwrap();
    f.manager.create_worktree(&f.name, &f.worktree).unwrap();
    let admin = gitlink(&fs::read(f.worktree.join(".git")).unwrap()).unwrap();
    fs::remove_dir_all(admin).unwrap();
    f.manager.remove_worktree(&f.name, &f.worktree).unwrap();
    f.manager.remove_worktree(&f.name, &f.worktree).unwrap();
    assert!(!f.worktree.exists());
}

#[test]
fn unrelated_directories_and_checkouts_are_preserved() {
    let f = Fixture::new();
    fs::create_dir(&f.worktree).unwrap();
    fs::write(f.worktree.join("precious"), "keep me").unwrap();
    assert!(matches!(
        f.manager.create_worktree(&f.name, &f.worktree),
        Err(Error::WorktreeOccupied)
    ));
    assert!(matches!(
        f.manager.remove_worktree(&f.name, &f.worktree),
        Err(Error::WorktreeOccupied)
    ));
    assert_eq!(
        fs::read_to_string(f.worktree.join("precious")).unwrap(),
        "keep me"
    );
    assert!(matches!(
        f.manager.create_worktree(&f.name, &f.repo),
        Err(Error::WorktreeOccupied)
    ));
}

#[test]
fn push_and_remote_deletion_converge_after_the_write_already_happened() {
    let f = Fixture::new();
    f.checkout();
    write_commit(&f.worktree, "leaf.txt", "leaf\n", "leaf work");
    let head = f.manager.head(&f.name).unwrap();
    for _ in 0..2 {
        assert_eq!(f.manager.push(&f.name, &f.remote, &f.token).unwrap(), head);
        assert_eq!(
            f.manager.remote_head(&f.name, &f.remote, &f.token).unwrap(),
            Some(head.clone())
        );
    }
    write_commit(&f.worktree, "next.txt", "next\n", "next work");
    let next = f.manager.push(&f.name, &f.remote, &f.token).unwrap();
    assert_ne!(head, next);
    for _ in 0..2 {
        f.manager
            .delete_remote_branch(&f.name, &f.remote, &f.token)
            .unwrap();
        assert_eq!(
            f.manager.remote_head(&f.name, &f.remote, &f.token).unwrap(),
            None
        );
    }
}

#[test]
fn clean_rebase_and_leased_push_are_repeatable() {
    let f = Fixture::new();
    f.checkout();
    write_commit(&f.worktree, "leaf.txt", "leaf\n", "leaf work");
    let old = f.manager.push(&f.name, &f.remote, &f.token).unwrap();
    write_commit(&f.repo, "main.txt", "main\n", "main work");
    let result = f.manager.rebase_onto_main(&f.name, &f.worktree).unwrap();
    let RebaseResult::Clean { head } = result else {
        panic!("expected clean rebase")
    };
    assert_ne!(head, old);
    assert_eq!(
        f.manager.rebase_onto_main(&f.name, &f.worktree).unwrap(),
        RebaseResult::Clean { head: head.clone() }
    );
    for _ in 0..2 {
        assert_eq!(
            f.manager
                .push_rebased(&f.name, &f.remote, &f.token, &old)
                .unwrap(),
            head
        );
    }
    f.assert_clean();
}

#[test]
fn conflicting_rebase_returns_files_and_restores_the_old_head() {
    let f = Fixture::new();
    f.checkout();
    write_commit(&f.worktree, "shared.txt", "leaf version\n", "leaf work");
    let old = f.manager.head(&f.name).unwrap();
    write_commit(&f.repo, "shared.txt", "main version\n", "main work");
    for _ in 0..2 {
        assert_eq!(
            f.manager.rebase_onto_main(&f.name, &f.worktree).unwrap(),
            RebaseResult::Conflicted {
                head: old.clone(),
                files: vec![PathBuf::from("shared.txt")]
            }
        );
        assert_eq!(f.manager.head(&f.name).unwrap(), old);
        assert_eq!(
            fs::read_to_string(f.worktree.join("shared.txt")).unwrap(),
            "leaf version\n"
        );
        f.assert_clean();
    }
}

#[test]
fn interrupted_rebase_is_aborted_before_retry() {
    let f = Fixture::new();
    f.checkout();
    write_commit(&f.worktree, "shared.txt", "leaf version\n", "leaf work");
    let old = f.manager.head(&f.name).unwrap();
    write_commit(&f.repo, "shared.txt", "main version\n", "main work");
    assert!(!raw_git(&f.worktree, ["rebase", "main"]).status.success());
    assert!(f.manager.rebase_active(&f.worktree).unwrap());
    assert_eq!(
        f.manager.rebase_onto_main(&f.name, &f.worktree).unwrap(),
        RebaseResult::Conflicted {
            head: old,
            files: vec![PathBuf::from("shared.txt")]
        }
    );
    f.assert_clean();
}

#[test]
fn dirty_rebase_is_rejected_without_changing_files() {
    let f = Fixture::new();
    f.checkout();
    fs::write(f.worktree.join("shared.txt"), "unsaved\n").unwrap();
    assert!(matches!(
        f.manager.rebase_onto_main(&f.name, &f.worktree),
        Err(Error::DirtyWorktree)
    ));
    assert_eq!(
        fs::read_to_string(f.worktree.join("shared.txt")).unwrap(),
        "unsaved\n"
    );
}

#[test]
fn changed_files_include_both_rename_paths_deletions_and_unusual_names() {
    let f = Fixture::new();
    let branch = f.checkout();
    git(&f.worktree, ["mv", "shared.txt", "renamed.txt"]);
    fs::write(f.worktree.join("line\nbreak.txt"), "new\n").unwrap();
    let binary_name = OsString::from_vec(b"non-utf8-\xff".to_vec());
    fs::write(f.worktree.join(&binary_name), "new\n").unwrap();
    git(&f.worktree, ["add", "--all"]);
    git(&f.worktree, ["commit", "-m", "rename and add"]);
    let head = f.manager.head(&f.name).unwrap();
    let actual = f.manager.changed_files(&branch.base, &head).unwrap();
    let mut expected = vec![
        PathBuf::from("shared.txt"),
        PathBuf::from("renamed.txt"),
        PathBuf::from("line\nbreak.txt"),
        PathBuf::from(binary_name),
    ];
    expected.sort();
    assert_eq!(actual, expected);
    assert_eq!(
        f.manager.changed_files(&head, &head).unwrap(),
        Vec::<PathBuf>::new()
    );
}

#[test]
fn a_remote_push_by_somebody_else_is_preserved() {
    let f = Fixture::new();
    let branch = f.checkout();
    f.manager.push(&f.name, &f.remote, &f.token).unwrap();
    write_commit(&f.repo, "main.txt", "someone else\n", "other work");
    let other = f.manager.resolve("refs/heads/main").unwrap();
    git(
        &f.bare,
        [
            "fetch",
            f.repo.to_str().unwrap(),
            &format!("refs/heads/main:{}", f.name.reference()),
        ],
    );
    write_commit(&f.worktree, "leaf.txt", "leaf\n", "leaf work");
    assert!(f.manager.push(&f.name, &f.remote, &f.token).is_err());
    assert!(
        f.manager
            .push_rebased(&f.name, &f.remote, &f.token, &branch.base)
            .is_err()
    );
    assert_eq!(
        f.manager.remote_head(&f.name, &f.remote, &f.token).unwrap(),
        Some(other)
    );
}

#[test]
fn git_errors_omit_raw_output_even_if_it_contains_a_token() {
    let mut f = Fixture::new();
    f.checkout();
    // A bad remote hook can echo credentials. Only the exit code crosses the API boundary.
    let hook = f.bare.join("hooks/pre-receive");
    fs::write(
        &hook,
        format!("#!/bin/sh\nprintf '%s\\n' '{TOKEN}' >&2\nexit 1\n"),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    let error = f.manager.push(&f.name, &f.remote, &f.token).unwrap_err();
    assert_no_secret(
        format!("{error:?} {error}").as_bytes(),
        &STANDARD.encode(format!("x-access-token:{TOKEN}")),
    );
    assert!(
        f.manager
            .invocations
            .borrow()
            .iter()
            .any(|invocation| invocation
                .stderr
                .windows(TOKEN.len())
                .any(|w| w == TOKEN.as_bytes())),
        "remote must emit the fake token to exercise diagnostic suppression"
    );
    f.upstream_emits_secret = true;
}

#[test]
fn invalid_inputs_are_rejected_without_echoing_credentials() {
    let f = Fixture::new();
    for value in [
        "",
        "main..leaf",
        "-option",
        "a\nb",
        "a.lock",
        "a/.hidden",
        "a//b",
        "HEAD",
        "a@{b",
        "a\\b",
    ] {
        assert!(BranchName::new(value).is_err());
    }
    for value in [
        "-option",
        "main",
        "a".repeat(39).as_str(),
        "g".repeat(40).as_str(),
    ] {
        assert!(CommitId::new(value).is_err());
    }
    for value in [
        format!("https://x-access-token:{TOKEN}@github.com/a/b.git"),
        format!("https://github.com/a/b?token={TOKEN}"),
        "http://github.com/a/b".into(),
        "ext::bad".into(),
    ] {
        let error = Remote::https(&value).err().unwrap();
        assert_no_secret(format!("{error:?} {error}").as_bytes(), TOKEN);
    }
    assert!(Remote::https("https://github.com/RobTand/tl3-sandbox.git").is_ok());
    let main = BranchName::new("main").unwrap();
    assert!(matches!(
        f.manager.create_branch(&main),
        Err(Error::MainBranch)
    ));
    assert!(matches!(
        f.manager.delete_local_branch(&main),
        Err(Error::MainBranch)
    ));
    assert!(matches!(
        f.manager.delete_remote_branch(&main, &f.remote, &f.token),
        Err(Error::MainBranch)
    ));
}
