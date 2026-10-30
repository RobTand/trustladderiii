mod worktree_fixture;
use std::{
    env, fs,
    io::{BufRead, BufReader, Write},
    os::unix::process::ExitStatusExt,
    path::Path,
    process::{Command, Stdio},
};
use tl3_controller::{
    adapters::worktrees::{Boundary, Real},
    executor::{Completion, Delivery, Executor, Observation, Reconciliation},
};
use tl3_kernel::{Ack, EffectKind, Event, Reply};
use worktree_fixture::{Fixture, commit, git};

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
    for boundary in ["before", "after"] {
        let scratch = tempfile::tempdir().unwrap();
        let f = Fixture::create(scratch.path());
        let prepare = prepared(&f);
        let push = made(&f, &prepare, false);
        kill(&f, &push, boundary);
        let mut newer = prepare.clone();
        newer.key = "new-epoch".into();
        newer.epoch = 2;
        f.adapter().execute(&newer).unwrap();
        assert!(f.adapter().execute(&push).is_err());
        let result = f.adapter().reconcile(&push).unwrap();
        if boundary == "before" {
            assert!(matches!(result, Reconciliation::Pending));
            assert!(git(&f.remote, &["for-each-ref", "refs/heads/tl3"]).is_empty());
        } else {
            found(result);
            assert_eq!(
                git(
                    &f.remote,
                    &["rev-parse", Real::branch(&push).unwrap().as_str()]
                ),
                push.head_sha.unwrap()
            );
        }
    }
}

fn main() {
    let args: Vec<_> = env::args().collect();
    if args.get(1).is_some_and(|s| s == "--child") {
        child(&args);
        return;
    }
    crashes();
    conflicts();
    fences_and_unknowns();
    leases_and_exact_heads();
    old_epoch_partial_attempts();
    println!(
        "worktree lifecycle, 14 SIGKILL boundaries, fencing, unknown outcomes and leases passed; no skips"
    );
}
