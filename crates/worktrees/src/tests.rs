use super::*;
use std::os::unix::ffi::OsStrExt;
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
        let root = temp.path().join("worktrees");
        fs::create_dir(&root).unwrap();
        let worktree = root.join("leaf\nwith spaces");
        fs::create_dir(&repo).unwrap();
        git(&repo, ["init", "--initial-branch=main"]);
        git(&repo, ["config", "user.name", "Test"]);
        git(&repo, ["config", "user.email", "test@localhost"]);
        write_commit(&repo, "shared.txt", "base\n", "base");
        git(
            &repo,
            [OsStr::new("init"), OsStr::new("--bare"), bare.as_os_str()],
        );
        let manager = Worktrees::open(&repo, &root).unwrap();
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

    fn branch(&self) -> Branch {
        self.manager
            .create_branch_at(
                &self.name,
                &self.manager.resolve("refs/heads/main").unwrap(),
            )
            .unwrap()
    }

    fn checkout(&self) -> Branch {
        let branch = self.branch();
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
        let invocations = self.manager.invocations.lock().unwrap();
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
    let branch = f.branch();
    write_commit(&f.repo, "main.txt", "main advance\n", "advance main");
    let reopened = Worktrees::open(&f.repo, &f.manager.root).unwrap();
    assert_eq!(
        reopened
            .create_branch_at(&f.name, &reopened.resolve("refs/heads/main").unwrap())
            .unwrap(),
        branch
    );
    assert_eq!(
        f.manager.branch_state(&f.name).unwrap().unwrap().0,
        branch.base
    );
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
    assert_eq!(f.branch(), branch);
    assert_eq!(
        f.manager.create_worktree(&f.name, &f.worktree).unwrap(),
        f.worktree
    );
    f.assert_clean();
}

#[test]
fn base_without_branch_recreates_the_original_branch() {
    let f = Fixture::new();
    let branch = f.branch();
    git(&f.repo, ["update-ref", "-d", &f.name.reference()]);
    write_commit(&f.repo, "main.txt", "new main\n", "advance main");
    assert_eq!(f.branch(), branch);
    assert_eq!(
        f.manager.branch_state(&f.name).unwrap().unwrap().0,
        branch.base
    );
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
        Err(Error::WorktreePath)
    ));
}

#[test]
fn conflicting_rebase_returns_files_and_restores_the_old_head() {
    let f = Fixture::new();
    f.checkout();
    write_commit(&f.worktree, "shared.txt", "leaf version\n", "leaf work");
    let old = f.manager.branch_state(&f.name).unwrap().unwrap().0;
    write_commit(&f.repo, "shared.txt", "main version\n", "main work");
    for _ in 0..2 {
        assert_eq!(
            f.manager
                .rebase_onto(
                    &f.name,
                    &f.worktree,
                    &f.manager.resolve("refs/heads/main").unwrap()
                )
                .unwrap(),
            RebaseResult::Conflicted {
                head: old.clone(),
                files: vec![PathBuf::from("shared.txt")]
            }
        );
        assert_eq!(f.manager.branch_state(&f.name).unwrap().unwrap().0, old);
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
    let old = f.manager.branch_state(&f.name).unwrap().unwrap().0;
    write_commit(&f.repo, "shared.txt", "main version\n", "main work");
    assert!(!raw_git(&f.worktree, ["rebase", "main"]).status.success());
    assert!(f.manager.rebase_active(&f.worktree).unwrap());
    assert_eq!(
        f.manager
            .rebase_onto(
                &f.name,
                &f.worktree,
                &f.manager.resolve("refs/heads/main").unwrap()
            )
            .unwrap(),
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
        f.manager.rebase_onto(
            &f.name,
            &f.worktree,
            &f.manager.resolve("refs/heads/main").unwrap()
        ),
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
    let head = f.manager.branch_state(&f.name).unwrap().unwrap().0;
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
    let error = f
        .manager
        .publish(
            &f.name,
            &f.remote,
            Some(&f.token),
            &f.manager.branch_state(&f.name).unwrap().unwrap().0,
            None,
        )
        .unwrap_err();
    assert_no_secret(
        format!("{error:?} {error}").as_bytes(),
        &STANDARD.encode(format!("x-access-token:{TOKEN}")),
    );
    assert!(
        f.manager
            .invocations
            .lock()
            .unwrap()
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
        "https://example.com/a/b.git".into(),
        "https://github.com.example.com/a/b.git".into(),
        "ext::bad".into(),
    ] {
        let error = Remote::https(&value).err().unwrap();
        assert_no_secret(format!("{error:?} {error}").as_bytes(), TOKEN);
    }
    assert!(Remote::https("https://github.com/RobTand/tl3-sandbox.git").is_ok());
    let main = BranchName::new("main").unwrap();
    assert!(matches!(
        f.manager
            .create_branch_at(&main, &f.manager.resolve("refs/heads/main").unwrap()),
        Err(Error::MainBranch)
    ));
    assert!(matches!(
        f.manager.delete_local_branch(&main),
        Err(Error::MainBranch)
    ));
    assert!(matches!(
        f.manager.delete_remote_branch_at(
            &main,
            &f.remote,
            Some(&f.token),
            &f.manager.resolve("refs/heads/main").unwrap()
        ),
        Err(Error::MainBranch)
    ));
}

#[test]
fn rebased_base_excludes_main_changes_from_the_footprint() {
    let f = Fixture::new();
    let original = f.checkout();
    write_commit(&f.worktree, "leaf.txt", "leaf\n", "leaf work");
    write_commit(&f.repo, "main.txt", "main\n", "main work");
    write_commit(&f.repo, "other.txt", "other\n", "more main work");
    let main = f.manager.resolve("refs/heads/main").unwrap();
    let RebaseResult::Clean { base, head } = f
        .manager
        .rebase_onto(
            &f.name,
            &f.worktree,
            &f.manager.resolve("refs/heads/main").unwrap(),
        )
        .unwrap()
    else {
        panic!("expected clean rebase")
    };
    assert_ne!(base, original.base);
    assert_eq!(base, main);
    assert_eq!(f.branch().base, base);
    assert_eq!(
        f.manager.changed_files(&base, &head).unwrap(),
        vec![PathBuf::from("leaf.txt")]
    );
    assert_eq!(f.manager.branch_state(&f.name).unwrap().unwrap().0, head);
    f.assert_clean();
}

#[test]
fn ignored_build_files_allow_rebase_and_removal() {
    let f = Fixture::new();
    write_commit(&f.repo, ".gitignore", "target/\n", "ignore build files");
    f.checkout();
    write_commit(&f.worktree, "leaf.txt", "leaf\n", "leaf work");
    fs::create_dir(f.worktree.join("target")).unwrap();
    fs::write(f.worktree.join("target/output"), "build output").unwrap();
    write_commit(&f.repo, "main.txt", "main\n", "main work");
    assert!(matches!(
        f.manager
            .rebase_onto(
                &f.name,
                &f.worktree,
                &f.manager.resolve("refs/heads/main").unwrap()
            )
            .unwrap(),
        RebaseResult::Clean { .. }
    ));
    assert_eq!(
        fs::read_to_string(f.worktree.join("target/output")).unwrap(),
        "build output"
    );
    f.manager.remove_worktree(&f.name, &f.worktree).unwrap();
    f.manager.remove_worktree(&f.name, &f.worktree).unwrap();
    assert!(!f.worktree.exists());
}

#[test]
fn destinations_outside_the_root_are_rejected_before_creating_directories() {
    let f = Fixture::new();
    // Exercise a direct path, traversal, and a symlink to an existing outside root.
    let outside = f.repo.parent().unwrap().join("outside");
    fs::create_dir(&outside).unwrap();
    let escape = f.manager.root.join("escape");
    std::os::unix::fs::symlink(&outside, &escape).unwrap();
    let paths = [
        outside.join("missing/leaf"),
        f.manager.root.join("../outside/missing/leaf"),
        escape.join("missing/leaf"),
        f.manager.root.clone(),
    ];
    for path in paths {
        assert!(matches!(
            f.manager.create_worktree(&f.name, &path),
            Err(Error::WorktreePath)
        ));
        assert!(matches!(
            f.manager.remove_worktree(&f.name, &path),
            Err(Error::WorktreePath)
        ));
        assert!(!outside.join("missing").exists());
        assert!(
            f.manager
                .resolve_optional(&f.name.reference())
                .unwrap()
                .is_none()
        );
    }
    f.checkout();
    let admin = gitlink(&fs::read(f.worktree.join(".git")).unwrap()).unwrap();
    // Even a registered checkout moved outside the root must not be removed.
    let moved = outside.join("registered");
    git(
        &f.repo,
        [
            OsStr::new("worktree"),
            OsStr::new("move"),
            f.worktree.as_os_str(),
            moved.as_os_str(),
        ],
    );
    assert!(admin.exists());
    assert!(matches!(
        f.manager.remove_worktree(&f.name, &moved),
        Err(Error::WorktreePath)
    ));
    assert!(moved.join("shared.txt").exists());
}

#[test]
fn symlinked_root_and_nested_destinations_are_supported() {
    let f = Fixture::new();
    let alias = f.repo.parent().unwrap().join("root-alias");
    std::os::unix::fs::symlink(&f.manager.root, &alias).unwrap();
    let manager = Worktrees::open(&f.repo, &alias).unwrap();
    let path = alias.join("new/nested/leaf");
    let actual = manager.create_worktree(&f.name, &path).unwrap();
    assert_eq!(actual, f.manager.root.join("new/nested/leaf"));
    manager.remove_worktree(&f.name, &path).unwrap();
    assert!(!actual.exists());
}

#[test]
fn concurrent_creates_and_removal_share_one_repository_mutex() {
    use std::sync::Barrier;
    let f = Fixture::new();
    // A second handle through a linked checkout must use the same common-dir key.
    f.checkout();
    let other = Worktrees::open(&f.worktree, &f.manager.root).unwrap();
    assert!(Arc::ptr_eq(&f.manager.mutex, &other.mutex));
    f.manager.remove_worktree(&f.name, &f.worktree).unwrap();
    // Reopen through the main checkout because the linked one was just removed.
    let other = Worktrees::open(&f.repo, &f.manager.root).unwrap();
    let barrier = Barrier::new(2);
    std::thread::scope(|scope| {
        let one = scope.spawn(|| {
            barrier.wait();
            f.manager.create_worktree(&f.name, &f.worktree)
        });
        let two = scope.spawn(|| {
            barrier.wait();
            other.create_worktree(&f.name, &f.worktree)
        });
        assert_eq!(one.join().unwrap().unwrap(), f.worktree);
        assert_eq!(two.join().unwrap().unwrap(), f.worktree);
    });
    f.assert_clean();
    let other_name = BranchName::new("tl3/other").unwrap();
    let other_path = f.manager.root.join("other");
    std::thread::scope(|scope| {
        let create = scope.spawn(|| {
            barrier.wait();
            other.create_worktree(&other_name, &other_path)
        });
        let remove = scope.spawn(|| {
            barrier.wait();
            f.manager.remove_worktree(&f.name, &f.worktree)
        });
        assert_eq!(create.join().unwrap().unwrap(), other_path);
        remove.join().unwrap().unwrap();
    });
    assert!(!f.worktree.exists());
    assert!(other_path.join(".git").is_file());
    assert_eq!(
        f.manager.branch_state(&other_name).unwrap().unwrap().0,
        other.branch_state(&other_name).unwrap().unwrap().0
    );
}

#[test]
fn detached_rebase_recovery_converges_before_and_after_ref_publication() {
    for phase in 0..5 {
        let f = Fixture::new();
        let original = f.checkout();
        write_commit(&f.worktree, "leaf.txt", "leaf\n", "leaf work");
        let old = f.manager.branch_state(&f.name).unwrap().unwrap().0;
        write_commit(&f.repo, "main.txt", "main\n", "main work");
        let main = f.manager.resolve("refs/heads/main").unwrap();
        let marker = f.manager.rebase_marker(&f.worktree).unwrap();
        fs::write(&marker, f.name.reference()).unwrap();
        if phase >= 1 {
            git(&f.worktree, ["checkout", "--detach", old.as_str()]);
        }
        if phase >= 2 {
            git(&f.worktree, ["rebase", main.as_str()]);
        }
        if phase >= 3 {
            let new = commit_line(&git(&f.worktree, ["rev-parse", "HEAD"])).unwrap();
            f.manager
                .update_refs(&format!(
                    "update {} {} {}\nupdate {} {} {}\n",
                    f.name.reference(),
                    new.as_str(),
                    old.as_str(),
                    f.name.base_ref(),
                    main.as_str(),
                    original.base.as_str()
                ))
                .unwrap();
        }
        if phase >= 4 {
            git(&f.worktree, ["symbolic-ref", "HEAD", &f.name.reference()]);
        }
        let RebaseResult::Clean { base, head } = f
            .manager
            .rebase_onto(
                &f.name,
                &f.worktree,
                &f.manager.resolve("refs/heads/main").unwrap(),
            )
            .unwrap()
        else {
            panic!("expected clean rebase")
        };
        assert_eq!(base, main);
        assert_eq!(
            f.manager.changed_files(&base, &head).unwrap(),
            vec![PathBuf::from("leaf.txt")]
        );
        assert!(!marker.exists());
        f.assert_clean();
    }
}

#[test]
fn interrupted_detached_rebase_aborts_and_preserves_the_base() {
    let f = Fixture::new();
    let original = f.checkout();
    write_commit(&f.worktree, "shared.txt", "leaf version\n", "leaf work");
    let old = f.manager.branch_state(&f.name).unwrap().unwrap().0;
    write_commit(&f.repo, "shared.txt", "main version\n", "main work");
    let marker = f.manager.rebase_marker(&f.worktree).unwrap();
    fs::write(&marker, f.name.reference()).unwrap();
    git(&f.worktree, ["checkout", "--detach", old.as_str()]);
    assert!(!raw_git(&f.worktree, ["rebase", "main"]).status.success());
    assert_eq!(
        f.manager
            .rebase_onto(
                &f.name,
                &f.worktree,
                &f.manager.resolve("refs/heads/main").unwrap()
            )
            .unwrap(),
        RebaseResult::Conflicted {
            head: old,
            files: vec![PathBuf::from("shared.txt")]
        }
    );
    assert_eq!(f.branch().base, original.base);
    assert!(!marker.exists());
    f.assert_clean();
}

#[test]
fn local_transport_has_no_credential_environment() {
    let f = Fixture::new();
    f.checkout();
    f.manager
        .publish(
            &f.name,
            &f.remote,
            None,
            &f.manager.branch_state(&f.name).unwrap().unwrap().0,
            None,
        )
        .unwrap();
    // Even an accidentally supplied token is withheld from local transport.
    f.manager
        .remote_head(&f.name, &f.remote, Some(&f.token))
        .unwrap();
    for invocation in f.manager.invocations.lock().unwrap().iter() {
        assert!(
            invocation
                .environment
                .iter()
                .all(|(key, _)| key != "GIT_CONFIG_COUNT")
        );
    }
}

#[test]
fn auth_header_is_scoped_to_github() {
    let f = Fixture::new();
    f.manager
        .execute(&f.repo, ["version"], Some(&f.token), None)
        .unwrap();
    let invocations = f.manager.invocations.lock().unwrap();
    let last = invocations.last().unwrap();
    assert!(
        last.environment
            .iter()
            .any(|(key, value)| key == "GIT_CONFIG_KEY_2"
                && value.as_deref() == Some(OsStr::new("http.https://github.com/.extraHeader")))
    );
    let remote = Remote::https("https://github.com/owner/repo.git").unwrap();
    assert!(remote.credentials(Some(&f.token)).is_some());
    assert!(f.remote.credentials(Some(&f.token)).is_none());
}
