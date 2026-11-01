use super::*;
use tl3_kernel::RepoId;

fn added() -> Repository {
    Repository {
        owner: "RobTand".into(),
        name: "prismabuild".into(),
        installation: 2,
    }
}
fn two_repos() -> Bootstrap {
    let mut boot = bootstrap();
    boot.repos.insert(0, added());
    boot
}
fn open_at(settings: &tl3_config::Store, root: &Path, boot: Bootstrap) -> Controller {
    Controller::open(
        Store::open(settings).unwrap(),
        adapters::simulation(root).unwrap(),
        boot,
    )
    .unwrap()
}
fn close(mut c: Controller, settings: &tl3_config::Store) -> Snapshot {
    let snapshot = c.snapshot().unwrap();
    drop(c);
    controller_lock_barrier(settings);
    snapshot
}
fn seed(settings: &tl3_config::Store, root: &Path, requests: u64) -> Snapshot {
    let mut c = open_at(settings, root, bootstrap());
    for number in 1..=requests {
        let mut issue = labelled();
        issue.number = number;
        issue.delivery = format!("repository-seed-{number}");
        assert!(c.intake(issue).unwrap());
    }
    close(c, settings)
}
fn issue(repo: &Repository) -> LabelledIssue {
    let mut issue = labelled();
    issue.owner = repo.owner.clone();
    issue.repo = repo.name.clone();
    issue.installation = repo.installation;
    issue.delivery = format!("{}/{}:{}", repo.owner, repo.name, repo.installation);
    issue
}
fn repositories(snapshot: &Snapshot) -> Vec<(u64, Repository)> {
    let mut repos: Vec<_> = snapshot
        .rows
        .iter()
        .filter(|r| r.table == "repos")
        .map(|r| {
            (
                r.id.parse().unwrap(),
                serde_json::from_value(r.value.clone()).unwrap(),
            )
        })
        .collect();
    repos.sort_by_key(|(index, _)| *index);
    repos
}
fn assert_reopened(settings: &tl3_config::Store, root: &Path, boot: Bootstrap, before: Snapshot) {
    let mut c = open_at(settings, root, boot);
    assert_eq!(
        c.snapshot().unwrap(),
        before,
        "second open must write nothing"
    );
    assert!(c.world().unwrap().valid());
}

pub(super) fn add_reorder(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let before = seed(settings, root.path(), 1);
    let mut c = open_at(settings, root.path(), two_repos());
    let after = c.snapshot().unwrap();
    assert_eq!(
        repositories(&after),
        vec![(0, bootstrap().repos[0].clone()), (1, added())],
        "append the missing repository without reordering stored indices"
    );
    assert_eq!(after.revision.get(), before.revision.get() + 1);
    let world = c.world().unwrap();
    assert_eq!(world.gates.len(), 2);
    assert!(world.valid());
    for table in ["nodes", "requests", "allowances"] {
        assert_eq!(
            after
                .rows
                .iter()
                .filter(|r| r.table == table)
                .collect::<Vec<_>>(),
            before
                .rows
                .iter()
                .filter(|r| r.table == table)
                .collect::<Vec<_>>(),
            "adding a repository must not change existing {table}"
        );
    }
    assert_eq!(after.pending_effects, before.pending_effects);
    assert_eq!(after.credit, before.credit);
    assert!(
        after
            .interned
            .iter()
            .any(|i| i.kind == tl3_store::InternKind::Repo
                && i.scope == "robtand"
                && i.value == "prismabuild")
    );
    assert!(c.intake(issue(&added())).unwrap());
    let world = c.world().unwrap();
    assert_eq!(world.nodes[0].repo, RepoId(0));
    assert_eq!(world.nodes[1].repo, RepoId(1));
    assert!(world.valid());
    let after = close(c, settings);
    assert_reopened(settings, root.path(), two_repos(), after);
}

pub(super) fn case_identity(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let before = seed(settings, root.path(), 1);
    let mut boot = bootstrap();
    boot.repos[0].owner = "robtand".into();
    boot.repos[0].name = "TL3-SANDBOX".into();
    let mut c = open_at(settings, root.path(), boot.clone());
    assert_eq!(
        c.snapshot().unwrap(),
        before,
        "case-only config changes are a no-op"
    );
    let mut repeated = issue(&boot.repos[0]);
    repeated.number = 1;
    assert!(c.intake(repeated).unwrap());
    assert_eq!(
        c.world().unwrap().nodes.len(),
        1,
        "case changes must not duplicate a request"
    );
    let mut next = issue(&boot.repos[0]);
    next.number = 2;
    next.delivery = "case-identity-next".into();
    assert!(c.intake(next).unwrap());
    assert!(c.world().unwrap().nodes.iter().all(|n| n.repo == RepoId(0)));
    let after = close(c, settings);
    assert_reopened(settings, root.path(), boot, after);
}

pub(super) fn append_three(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    seed(settings, root.path(), 1);
    let mut boot = two_repos();
    let mut third = added();
    third.name = "third-repository".into();
    boot.repos.insert(0, third.clone());
    let mut c = open_at(settings, root.path(), boot.clone());
    let w = c.world().unwrap();
    assert_eq!(w.gates.len(), 3, "append all missing gates");
    assert!(
        w.valid(),
        "the larger gates vector must preserve every invariant"
    );
    assert!(c.intake(issue(&third)).unwrap());
    assert!(c.intake(issue(&added())).unwrap());
    let w = c.world().unwrap();
    assert_eq!(w.nodes[1].repo, RepoId(1));
    assert_eq!(w.nodes[2].repo, RepoId(2));
    assert!(w.valid());
    let after = close(c, settings);
    assert_reopened(settings, root.path(), boot, after);
}

pub(super) fn remove_active(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let before = seed(settings, root.path(), 1);
    let mut boot = bootstrap();
    boot.repos = vec![added()];
    let result = Controller::open(
        Store::open(settings).unwrap(),
        adapters::simulation(root.path()).unwrap(),
        boot,
    );
    let Err(error) = result else {
        panic!("removing a repository with open requests must refuse startup");
    };
    assert!(
        error.to_string().contains("open requests") && error.to_string().contains("tl3-sandbox"),
        "{error}"
    );
    controller_lock_barrier(settings);
    assert_eq!(
        Store::open(settings).unwrap().load().unwrap(),
        before,
        "validate removals before interning or committing additions"
    );
}

pub(super) fn remove_idle(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    seed(settings, root.path(), 0);
    let mut boot = bootstrap();
    boot.repos = vec![added()];
    let mut c = open_at(settings, root.path(), boot.clone());
    assert_eq!(
        repositories(&c.snapshot().unwrap()).len(),
        2,
        "retain the unserved stored repository"
    );
    let before = c.snapshot().unwrap();
    assert!(
        !c.intake(labelled()).unwrap(),
        "removed repository issues must be ignored"
    );
    let mut unknown = labelled();
    unknown.repo = "unknown".into();
    assert!(!c.intake(unknown).unwrap());
    assert_eq!(c.snapshot().unwrap(), before);
    assert!(c.intake(issue(&added())).unwrap());
    drain(&mut c);
    assert_eq!(
        c.world().unwrap().nodes[0].phase,
        Phase::Closed(Outcome::Merged)
    );
    assert_eq!(c.world().unwrap().nodes[0].repo, RepoId(1));
    let after = close(c, settings);
    assert_reopened(settings, root.path(), boot, after);
}

pub(super) fn installation(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    seed(settings, root.path(), 0);
    let mut boot = bootstrap();
    boot.repos[0].installation = 99;
    let mut c = open_at(settings, root.path(), boot.clone());
    assert_eq!(
        repositories(&c.snapshot().unwrap()).len(),
        2,
        "a different installation is a different repository binding"
    );
    assert!(!c.intake(labelled()).unwrap());
    assert!(c.intake(issue(&boot.repos[0])).unwrap());
    assert_eq!(c.world().unwrap().nodes[0].repo, RepoId(1));
    let after = close(c, settings);
    assert_reopened(settings, root.path(), boot, after);
}

pub(super) fn child(args: &[String]) {
    let settings: tl3_config::Store = serde_json::from_slice(&fs::read(&args[2]).unwrap()).unwrap();
    let point = args[4].clone();
    let mut store = Store::open(&settings).unwrap();
    store.set_boundary_hook(move |at, key, _| {
        if (key.starts_with("controller:gates:") || key == "intern:repo:robtand:prismabuild")
            && format!("{at:?}") == point
        {
            park();
        }
    });
    let mut boot = two_repos();
    boot.pool = 3;
    Controller::open(
        store,
        adapters::simulation(Path::new(&args[3])).unwrap(),
        boot,
    )
    .unwrap();
}

pub(super) fn crash(settings: &tl3_config::Store, point: &str) {
    let root = tempfile::tempdir().unwrap();
    let before = seed(settings, root.path(), 3);
    let config = root.path().join("store.json");
    fs::write(
        &config,
        serde_json::to_vec(&json!({
            "host": settings.host, "port":settings.port, "database":settings.database,
            "user":settings.user, "password_file":settings.password_file,
            "connect_timeout_secs":settings.connect_timeout_secs,
        }))
        .unwrap(),
    )
    .unwrap();
    let mut child = Command::new(env::current_exe().unwrap())
        .arg("--repository-migration-child")
        .arg(&config)
        .arg(root.path())
        .arg(point)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut ready = false;
    for line in BufReader::new(child.stdout.take().unwrap()).lines() {
        if line.unwrap() == "READY" {
            ready = true;
            break;
        }
    }
    if !ready {
        panic!(
            "repository migration never reached {point}; child exited {}",
            child.wait().unwrap()
        );
    }
    let during = Store::open(settings).unwrap().load().unwrap();
    if point != "AfterCommit" {
        assert_eq!(during.revision, before.revision);
        assert_eq!(during.rows, before.rows, "partial world became visible");
        assert_eq!(during.pending_effects, before.pending_effects);
    }
    child.kill().unwrap();
    use std::os::unix::process::ExitStatusExt;
    assert_eq!(child.wait().unwrap().signal(), Some(9));
    controller_lock_barrier(settings);
    let mut boot = two_repos();
    boot.pool = 3;
    let mut c = open_at(settings, root.path(), boot.clone());
    let world = c.world().unwrap();
    assert_eq!(
        world.gates.len(),
        2,
        "recover the appended repository and gate"
    );
    assert_eq!(world.pool_capacity, 3);
    assert!(world.valid());
    let after = c.snapshot().unwrap();
    assert_eq!(after.revision.get(), before.revision.get() + 1);
    assert_eq!(repositories(&after).len(), 2);
    assert_eq!(after.credit, before.credit);
    let effects: Vec<_> = after
        .pending_effects
        .iter()
        .filter(|e| !before.pending_effects.iter().any(|old| old.key == e.key))
        .collect();
    assert_eq!(
        effects.len(),
        2,
        "recover exactly two newly admitted agent runs"
    );
    assert!(effects.iter().all(|e| e.created_revision == after.revision));
    let after = close(c, settings);
    assert_reopened(settings, root.path(), boot, after);
}
