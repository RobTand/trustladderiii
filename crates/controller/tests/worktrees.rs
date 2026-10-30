mod worktree_fixture;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{
    env, fs,
    io::{BufRead, BufReader, Read, Write},
    os::unix::{
        net::{UnixListener, UnixStream},
        process::ExitStatusExt,
    },
    path::Path,
    process::{Command, Stdio},
};
use tl3_controller::{
    adapters::worktrees::{Boundary, Real},
    executor::{Completion, Delivery, Executor, Observation, Reconciliation},
};
use tl3_kernel::{Ack, EffectKind, Event, Reply};
use worktree_fixture::{Fixture, commit, git};
static VERIFIED_KILLS: AtomicUsize = AtomicUsize::new(0);

fn found(result: Reconciliation) -> Completion {
    match result {
        Reconciliation::Found(completion) => completion,
        _ => panic!("expected terminal Git evidence"),
    }
}
fn same(a: &Completion, b: &Completion) {
    assert_eq!(
        serde_json::to_value(a).unwrap(),
        serde_json::to_value(b).unwrap()
    );
}
fn child(args: &[String]) {
    let fixture = Fixture::open(Path::new(&args[2]));
    let delivery: Delivery =
        serde_json::from_slice(&fs::read(fixture.root.join("delivery.json")).unwrap()).unwrap();
    let boundary = if args[3] == "before" {
        Boundary::BeforeApply
    } else {
        Boundary::AfterApply
    };
    fixture
        .adapter()
        .execute_with(&delivery, |at| {
            if at == boundary {
                println!("parked");
                std::io::stdout().flush().unwrap();
                loop {
                    std::thread::park();
                }
            }
        })
        .unwrap();
    panic!("crash boundary was not reached");
}
fn kill(fixture: &Fixture, delivery: &Delivery, boundary: &str) {
    fs::write(
        fixture.root.join("delivery.json"),
        serde_json::to_vec(delivery).unwrap(),
    )
    .unwrap();
    let mut child = Command::new(env::current_exe().unwrap())
        .args(["--child", fixture.root.to_str().unwrap(), boundary])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    assert_eq!(line.trim(), "parked");
    child.kill().unwrap();
    assert_eq!(child.wait().unwrap().signal(), Some(9));
    VERIFIED_KILLS.fetch_add(1, Ordering::Relaxed);
}
fn prepared(f: &Fixture) -> Delivery {
    let delivery = f.delivery(EffectKind::PrepareWorktree, 1);
    f.adapter().execute(&delivery).unwrap();
    delivery
}
fn made(f: &Fixture, prepare: &Delivery, conflict: bool) -> Delivery {
    let checkout = f.adapter().checkout(prepare);
    let head = commit(
        &checkout,
        if conflict { "shared.txt" } else { "leaf.txt" },
        "leaf change\n",
    );
    let mut push = f.delivery(EffectKind::Push, 2);
    push.head_sha = Some(head);
    push
}
fn assert_removed(f: &Fixture, delivery: &Delivery) {
    assert!(!f.adapter().checkout(delivery).exists());
    assert!(
        git(
            &f.repo,
            &["for-each-ref", "refs/heads/tl3", "refs/tl3/bases/tl3"]
        )
        .is_empty()
    );
    assert!(git(&f.remote, &["for-each-ref", "refs/heads/tl3"]).is_empty());
    assert_eq!(
        git(&f.repo, &["worktree", "list", "--porcelain"])
            .matches("worktree ")
            .count(),
        1
    );
}
fn crashes() {
    for boundary in ["before", "after"] {
        for kind in [
            EffectKind::PrepareWorktree,
            EffectKind::Push,
            EffectKind::StartRebase,
            EffectKind::RemoveWorktree,
        ] {
            let scratch = tempfile::tempdir().unwrap();
            let f = Fixture::create(scratch.path());
            let delivery = if kind == EffectKind::PrepareWorktree {
                f.delivery(kind, 1)
            } else {
                let prepare = prepared(&f);
                let push = made(&f, &prepare, false);
                if kind == EffectKind::Push {
                    push
                } else {
                    f.adapter().execute(&push).unwrap();
                    if kind == EffectKind::StartRebase {
                        f.advance_main(false);
                    }
                    f.delivery(kind, 3)
                }
            };
            let target = git(&f.remote, &["rev-parse", "main"]);
            kill(&f, &delivery, boundary);
            let before = if boundary == "after" && kind != EffectKind::RemoveWorktree {
                Some(f.head(&delivery))
            } else {
                None
            };
            // Main moving after an interrupted operation must not change its recorded target.
            if kind == EffectKind::StartRebase {
                f.advance_main(false);
            }
            let completion = found(f.adapter().reconcile(&delivery).unwrap());
            assert!(matches!(completion.ack, Ack::Confirmed));
            same(&completion, &f.adapter().execute(&delivery).unwrap());
            same(
                &completion,
                &found(f.adapter().reconcile(&delivery).unwrap()),
            );
            if let Some(before) = before {
                assert_eq!(before, f.head(&delivery));
            }
            match kind {
                EffectKind::PrepareWorktree => {
                    assert!(f.adapter().checkout(&delivery).join(".git").is_file())
                }
                EffectKind::Push => assert_eq!(
                    git(
                        &f.remote,
                        &["rev-parse", Real::branch(&delivery).unwrap().as_str()]
                    ),
                    delivery.head_sha.unwrap()
                ),
                EffectKind::StartRebase => {
                    assert_eq!(
                        git(
                            &f.repo,
                            &[
                                "rev-parse",
                                &format!(
                                    "refs/tl3/bases/{}",
                                    Real::branch(&delivery).unwrap().as_str()
                                )
                            ]
                        ),
                        target
                    );
                    assert!(matches!(
                        completion.observations.as_slice(),
                        [Observation::Rebased { .. }]
                    ));
                    assert_eq!(
                        git(
                            &f.repo,
                            &[
                                "rev-list",
                                "--count",
                                &format!(
                                    "{}..{}",
                                    git(
                                        &f.repo,
                                        &[
                                            "rev-parse",
                                            &format!(
                                                "refs/tl3/bases/{}",
                                                Real::branch(&delivery).unwrap().as_str()
                                            )
                                        ]
                                    ),
                                    Real::branch(&delivery).unwrap().as_str()
                                )
                            ]
                        ),
                        "1"
                    );
                }
                EffectKind::RemoveWorktree => {
                    assert_removed(&f, &delivery);
                    // Replaying a completed prepare must never resurrect a torn down checkout.
                    f.adapter()
                        .execute(&f.delivery(EffectKind::PrepareWorktree, 1))
                        .unwrap();
                    assert_removed(&f, &delivery);
                }
                _ => unreachable!(),
            }
            println!("SIGKILL {kind:?} {boundary}: original key converges once");
        }
    }
}
fn conflicts() {
    for boundary in ["before", "after"] {
        let scratch = tempfile::tempdir().unwrap();
        let f = Fixture::create(scratch.path());
        let prepare = prepared(&f);
        let push = made(&f, &prepare, true);
        f.adapter().execute(&push).unwrap();
        f.advance_main(true);
        let rebase = f.delivery(EffectKind::StartRebase, 3);
        kill(&f, &rebase, boundary);
        let completion = found(f.adapter().reconcile(&rebase).unwrap());
        assert!(matches!(
            completion.observations.as_slice(),
            [Observation::Kernel(Event::Reply {
                reply: Reply::RebaseNeedsChanges,
                ..
            })]
        ));
        assert_eq!(f.head(&rebase), push.head_sha.unwrap());
        let checkout = f.adapter().checkout(&rebase);
        assert!(
            git(
                &checkout,
                &["status", "--porcelain", "--untracked-files=all"]
            )
            .is_empty()
        );
        assert_eq!(
            git(&checkout, &["symbolic-ref", "--short", "HEAD"]),
            Real::branch(&rebase).unwrap().as_str()
        );
        let remove = f.delivery(EffectKind::RemoveWorktree, 4);
        f.adapter().execute(&remove).unwrap();
        assert_removed(&f, &remove);
    }
}
fn fences_and_unknowns() {
    let scratch = tempfile::tempdir().unwrap();
    let f = Fixture::create(scratch.path());
    let prepare = prepared(&f);
    let mut newer = prepare.clone();
    newer.key = "new-epoch".into();
    newer.epoch = 2;
    f.adapter().execute(&newer).unwrap();
    assert!(f.adapter().execute(&prepare).is_err());
    assert!(matches!(
        f.adapter().reconcile(&prepare).unwrap(),
        Reconciliation::Found(_)
    ));
    let mut missing = f.delivery(EffectKind::Push, 9);
    missing.head_sha = Some(f.head(&prepare));
    assert!(matches!(
        f.adapter().reconcile(&missing).unwrap(),
        Reconciliation::Absent
    ));
    // A remote failure is unknown, never an invented acknowledgement or absence.
    missing.epoch = 2;
    let mut adapter = f.adapter();
    fs::rename(&f.remote, f.root.join("offline.git")).unwrap();
    assert!(adapter.execute(&missing).is_err());
    assert!(adapter.reconcile(&missing).is_err());
    fs::rename(f.root.join("offline.git"), &f.remote).unwrap();
    found(f.adapter().reconcile(&missing).unwrap());
}
fn leases_and_exact_heads() {
    let scratch = tempfile::tempdir().unwrap();
    let f = Fixture::create(scratch.path());
    let prepare = prepared(&f);
    let push = made(&f, &prepare, false);
    // Later local work cannot change the immutable head requested for this push.
    let checkout = f.adapter().checkout(&prepare);
    commit(&checkout, "later.txt", "later\n");
    f.adapter().execute(&push).unwrap();
    assert_eq!(
        git(
            &f.remote,
            &["rev-parse", Real::branch(&push).unwrap().as_str()]
        ),
        push.head_sha.clone().unwrap()
    );
    let external = f.advance_main(false);
    git(
        &f.remote,
        &[
            "update-ref",
            &format!("refs/heads/{}", Real::branch(&push).unwrap().as_str()),
            &external,
        ],
    );
    let mut next = f.delivery(EffectKind::Push, 3);
    next.head_sha = Some(f.head(&prepare));
    assert!(f.adapter().execute(&next).is_err());
    assert!(
        f.adapter()
            .execute(&f.delivery(EffectKind::RemoveWorktree, 4))
            .is_err()
    );
    assert_eq!(
        git(
            &f.remote,
            &["rev-parse", Real::branch(&push).unwrap().as_str()]
        ),
        external
    );
}
fn old_epoch_partial_attempts() {
    for kind in [EffectKind::Push, EffectKind::StartRebase] {
        for boundary in ["before", "after"] {
            let scratch = tempfile::tempdir().unwrap();
            let f = Fixture::create(scratch.path());
            let prepare = prepared(&f);
            let push = made(&f, &prepare, false);
            let delivery = if kind == EffectKind::Push {
                push.clone()
            } else {
                f.adapter().execute(&push).unwrap();
                f.advance_main(false);
                f.delivery(EffectKind::StartRebase, 3)
            };
            kill(&f, &delivery, boundary);
            let head = f.head(&delivery);
            // Opening the replacement fences stale handles before any new execute.
            let mut stale = f.adapter();
            let mut replacement = f.adapter_at(2);
            assert!(f.adapter().execute(&delivery).is_err());
            let result = stale.reconcile(&delivery).unwrap();
            same_reconciliation(&result, &replacement.reconcile(&delivery).unwrap());
            if boundary == "before" {
                assert!(matches!(result, Reconciliation::Absent));
                if kind == EffectKind::Push {
                    assert!(git(&f.remote, &["for-each-ref", "refs/heads/tl3"]).is_empty());
                }
            } else {
                let completion = found(result);
                if kind == EffectKind::Push {
                    assert_eq!(
                        git(
                            &f.remote,
                            &["rev-parse", Real::branch(&push).unwrap().as_str()]
                        ),
                        push.head_sha.unwrap()
                    );
                } else {
                    assert!(
                        matches!(completion.observations.as_slice(), [Observation::Rebased { sha, .. }] if sha == &head)
                    );
                }
            }
            assert_eq!(
                f.head(&delivery),
                head,
                "old-epoch reconciliation cannot change Git"
            );
        }
    }
}

fn same_reconciliation(a: &Reconciliation, b: &Reconciliation) {
    match (a, b) {
        (Reconciliation::Absent, Reconciliation::Absent) => {}
        (Reconciliation::Found(a), Reconciliation::Found(b)) => same(a, b),
        _ => panic!("reconciliation changed across the fence"),
    }
}

fn incomplete_prepare() {
    for changed_head in [false, true] {
        let scratch = tempfile::tempdir().unwrap();
        let f = Fixture::create(scratch.path());
        let prepare = f.delivery(EffectKind::PrepareWorktree, 1);
        kill(&f, &prepare, "before");
        let branch = Real::branch(&prepare).unwrap();
        let base = git(&f.remote, &["rev-parse", "main"]);
        let checkout = f.adapter().checkout(&prepare);
        fs::create_dir_all(checkout.parent().unwrap()).unwrap();
        git(&f.repo, &["branch", branch.as_str(), &base]);
        git(
            &f.repo,
            &[
                "update-ref",
                &format!("refs/tl3/bases/{}", branch.as_str()),
                &base,
            ],
        );
        git(
            &f.repo,
            &[
                "worktree",
                "add",
                "--no-checkout",
                checkout.to_str().unwrap(),
                branch.as_str(),
            ],
        );
        assert!(!checkout.join("shared.txt").exists());
        assert!(!git(&checkout, &["status", "--porcelain"]).is_empty());
        if changed_head {
            git(&checkout, &["reset", "--hard", "HEAD"]);
            commit(&checkout, "maker.txt", "maker ran unexpectedly\n");
            fs::remove_file(checkout.join("shared.txt")).unwrap();
            assert!(f.adapter().reconcile(&prepare).is_err());
            assert!(checkout.join("maker.txt").exists());
            assert!(!checkout.join("shared.txt").exists());
        } else {
            let completion = found(f.adapter().reconcile(&prepare).unwrap());
            assert!(matches!(completion.ack, Ack::Confirmed));
            assert_eq!(
                fs::read_to_string(checkout.join("shared.txt")).unwrap(),
                "base\n"
            );
            assert!(git(&checkout, &["status", "--porcelain"]).is_empty());
        }
    }
}

fn incomplete_remove() {
    for dirt in ["deletions", "modified", "untracked", "staged", "renamed"] {
        let scratch = tempfile::tempdir().unwrap();
        let f = Fixture::create(scratch.path());
        let prepare = prepared(&f);
        let checkout = f.adapter().checkout(&prepare);
        let push = made(&f, &prepare, false);
        f.adapter().execute(&push).unwrap();
        let remove = f.delivery(EffectKind::RemoveWorktree, 3);
        kill(&f, &remove, "before");
        fs::remove_file(checkout.join("shared.txt")).unwrap();
        match dirt {
            "modified" | "staged" => {
                fs::write(checkout.join("leaf.txt"), "unsaved\n").unwrap();
                if dirt == "staged" {
                    git(&checkout, &["add", "leaf.txt"]);
                }
            }
            "untracked" => {
                fs::write(checkout.join("new.txt"), "keep\n").unwrap();
            }
            "renamed" => {
                git(&checkout, &["mv", "leaf.txt", "renamed.txt"]);
            }
            _ => {}
        }
        if dirt == "deletions" {
            found(f.adapter().reconcile(&remove).unwrap());
            assert_removed(&f, &remove);
        } else {
            let status = git(
                &checkout,
                &["status", "--porcelain", "--untracked-files=all"],
            );
            assert!(f.adapter().reconcile(&remove).is_err());
            assert!(f.adapter().execute(&remove).is_err());
            assert_eq!(
                git(
                    &checkout,
                    &["status", "--porcelain", "--untracked-files=all"]
                ),
                status
            );
            assert!(checkout.exists());
        }
    }
    // Deletions without a preflight receipt must not authorize destructive recovery.
    let scratch = tempfile::tempdir().unwrap();
    let f = Fixture::create(scratch.path());
    let prepare = prepared(&f);
    let checkout = f.adapter().checkout(&prepare);
    fs::remove_file(checkout.join("shared.txt")).unwrap();
    assert!(
        f.adapter()
            .execute(&f.delivery(EffectKind::RemoveWorktree, 2))
            .is_err()
    );
    assert!(checkout.exists());
}

fn removal_without_gitlink() {
    for dirt in ["deletions", "modified", "untracked"] {
        let scratch = tempfile::tempdir().unwrap();
        let f = Fixture::create(scratch.path());
        let prepare = prepared(&f);
        let push = made(&f, &prepare, false);
        f.adapter().execute(&push).unwrap();
        let remove = f.delivery(EffectKind::RemoveWorktree, 3);
        kill(&f, &remove, "before");
        let checkout = f.adapter().checkout(&prepare);
        fs::remove_file(checkout.join(".git")).unwrap();
        fs::remove_file(checkout.join("shared.txt")).unwrap();
        match dirt {
            "modified" => {
                fs::write(checkout.join("leaf.txt"), "keep edit\n").unwrap();
            }
            "untracked" => {
                fs::write(checkout.join("new.txt"), "keep new\n").unwrap();
            }
            _ => {}
        }
        if dirt == "deletions" {
            found(f.adapter().reconcile(&remove).unwrap());
            assert_removed(&f, &remove);
        } else {
            assert!(f.adapter().reconcile(&remove).is_err());
            let file = if dirt == "modified" {
                "leaf.txt"
            } else {
                "new.txt"
            };
            assert!(
                fs::read_to_string(checkout.join(file))
                    .unwrap()
                    .starts_with("keep")
            );
            assert!(checkout.join(".git").is_file());
        }
    }
}

fn killed_git_add() {
    let scratch = tempfile::tempdir().unwrap();
    let f = Fixture::create(scratch.path());
    commit(&f.source, ".gitattributes", "shared.txt filter=park\n");
    git(&f.source, &["push", f.remote.to_str().unwrap(), "main"]);
    let prepare = f.delivery(EffectKind::PrepareWorktree, 1);
    kill(&f, &prepare, "before");
    let branch = Real::branch(&prepare).unwrap();
    let base = git(&f.remote, &["rev-parse", "main"]);
    let checkout = f.adapter().checkout(&prepare);
    fs::create_dir_all(checkout.parent().unwrap()).unwrap();
    git(&f.repo, &["branch", branch.as_str(), &base]);
    git(
        &f.repo,
        &[
            "update-ref",
            &format!("refs/tl3/bases/{}", branch.as_str()),
            &base,
        ],
    );
    let socket = f.root.join("smudge.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    // Git blocks in a real checkout filter after registering the worktree.
    git(
        &f.repo,
        &[
            "config",
            "filter.park.smudge",
            &format!(
                "'{}' --smudge '{}'",
                env::current_exe().unwrap().display(),
                socket.display()
            ),
        ],
    );
    git(&f.repo, &["config", "filter.park.required", "true"]);
    let mut child = Command::new("git")
        .current_dir(&f.repo)
        .env_clear()
        .env("PATH", env::var_os("PATH").unwrap())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args([
            "worktree",
            "add",
            "--",
            checkout.to_str().unwrap(),
            branch.as_str(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let (mut filter, _) = listener.accept().unwrap();
    let mut ready = [0];
    filter.read_exact(&mut ready).unwrap();
    assert_eq!(ready, [1]);
    assert!(checkout.join(".git").is_file());
    child.kill().unwrap();
    assert_eq!(child.wait().unwrap().signal(), Some(9));
    VERIFIED_KILLS.fetch_add(1, Ordering::Relaxed);
    filter.write_all(&[1]).unwrap();
    // EOF proves the smudge process has exited before recovery changes Git state.
    assert_eq!(filter.read(&mut ready).unwrap(), 0);
    git(&f.repo, &["config", "--remove-section", "filter.park"]);
    assert!(!checkout.join("shared.txt").exists());
    found(f.adapter().reconcile(&prepare).unwrap());
    assert_eq!(
        fs::read_to_string(checkout.join("shared.txt")).unwrap(),
        "base\n"
    );
    assert!(git(&checkout, &["status", "--porcelain"]).is_empty());
    println!("SIGKILL during git worktree add: populated before confirmation");
}

fn main() {
    let args: Vec<_> = env::args().collect();
    if args.get(1).is_some_and(|s| s == "--smudge") {
        let mut socket = UnixStream::connect(&args[2]).unwrap();
        socket.write_all(&[1]).unwrap();
        let mut release = [0];
        socket.read_exact(&mut release).unwrap();
        return;
    }
    if args.get(1).is_some_and(|s| s == "--child") {
        child(&args);
        return;
    }
    crashes();
    conflicts();
    fences_and_unknowns();
    leases_and_exact_heads();
    old_epoch_partial_attempts();
    incomplete_prepare();
    incomplete_remove();
    removal_without_gitlink();
    killed_git_add();
    println!(
        "worktree lifecycle, {} verified SIGKILL boundaries, fencing, unknown outcomes and leases passed; no skips",
        VERIFIED_KILLS.load(Ordering::Relaxed)
    );
}
