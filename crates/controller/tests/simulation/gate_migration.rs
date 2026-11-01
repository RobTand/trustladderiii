use super::*;
use tl3_kernel::{Role, World};

struct Workers(adapters::workers::Fake);
impl Executor for Workers {
    fn execute(&mut self, delivery: &Delivery) -> tl3_controller::Result<Completion> {
        match delivery.output {
            Out::Effect {
                kind: EffectKind::StartRun(Role::Plan | Role::Make),
                ..
            } => Ok(Completion::confirmed(vec![])),
            _ => self.0.execute(delivery),
        }
    }
    fn reconcile(&mut self, delivery: &Delivery) -> tl3_controller::Result<Reconciliation> {
        match delivery.output {
            Out::Effect {
                kind: EffectKind::StartRun(Role::Plan | Role::Make),
                ..
            } => Ok(Reconciliation::Pending),
            _ => self.0.reconcile(delivery),
        }
    }
}
fn registry(root: &Path) -> Registry {
    let mut registry = adapters::simulation(root).unwrap();
    registry.register(
        Upstream::Workers,
        Workers(adapters::workers::Fake::new(root)),
    );
    registry
}
fn boot(slots: u8) -> Bootstrap {
    Bootstrap {
        repos: vec![Repository {
            owner: "owner".into(),
            name: "gate-migration".into(),
            installation: 1,
        }],
        rob: 42,
        operations: None,
        slots,
        pool: 4,
    }
}
fn close(mut c: Controller, settings: &tl3_config::Store) -> Snapshot {
    let snapshot = c.snapshot().unwrap();
    drop(c);
    controller_lock_barrier(settings);
    snapshot
}
fn open_with_pool(settings: &tl3_config::Store, root: &Path, slots: u8, pool: u8) -> Controller {
    let mut bootstrap = boot(slots);
    bootstrap.pool = pool;
    Controller::open(Store::open(settings).unwrap(), registry(root), bootstrap).unwrap()
}
fn open_at(settings: &tl3_config::Store, root: &Path, slots: u8) -> Controller {
    Controller::open(Store::open(settings).unwrap(), registry(root), boot(slots)).unwrap()
}
fn seed(settings: &tl3_config::Store, root: &Path, slots: u8) -> Snapshot {
    seed_with_look(settings, root, slots, 3).0
}
fn seed_with_look(
    settings: &tl3_config::Store,
    root: &Path,
    slots: u8,
    count: u64,
) -> (Snapshot, tl3_store::WorldRow) {
    let mut c = open_at(settings, root, slots);
    for number in 1..=count {
        assert!(
            c.intake(LabelledIssue {
                delivery: format!("gate-{number}"),
                owner: "owner".into(),
                repo: "gate-migration".into(),
                installation: 1,
                number,
                title: format!("Disjoint leaf {number}"),
                actor: 42,
                label: "tl3".into(),
            })
            .unwrap()
        );
        drain(&mut c);
    }
    let ended = c
        .snapshot()
        .unwrap()
        .rows
        .into_iter()
        .find(|r| r.table == "allowances" && matches!(serde_json::from_value::<Delivery>(r.value["delivery"].clone()).unwrap().output, Out::Effect { node, .. } if node == NodeId(count - 1)))
        .unwrap();
    // Finish all plans after admission, so W=1 has two already-planned waiters.
    assert!(
        c.world()
            .unwrap()
            .nodes
            .iter()
            .all(|n| matches!(n.phase, Phase::Planning { .. }))
    );
    for node in 0..count {
        let generation = c.world().unwrap().nodes[node as usize].generation;
        c.observe(
            &format!("plan-{node}"),
            Observation::Plan {
                node: NodeId(node),
                generation,
                paths: vec![format!("leaf-{node}.rs")],
                fits: true,
            },
        )
        .unwrap();
        drain(&mut c);
    }
    if count == 4 {
        c.observe(
            "stop-legacy-look-owner",
            Observation::Kernel(Event::Command {
                node: NodeId(3),
                id: 1,
                command: Control::Stop,
            }),
        )
        .unwrap();
        drain(&mut c);
        assert!(c.world().unwrap().nodes[3].phase.is_closed());
    }
    let w = c.world().unwrap();
    assert_eq!(w.gates[0].w, slots);
    assert_eq!(
        w.nodes.iter().filter(|n| n.holds).count(),
        usize::from(slots),
        "{w:?}"
    );
    if slots == 1 {
        assert!(
            w.nodes[1..]
                .iter()
                .all(|n| !n.holds && (n.ticket.is_some() || n.phase.is_closed()))
        );
    }
    assert!(
        !c.snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "allowances" && r.id == ended.id)
    );
    (close(c, settings), ended)
}
fn verify_migrated(c: &mut Controller, before: &Snapshot) -> Snapshot {
    let w: World = c.world().unwrap();
    assert_eq!(
        w.gates[0].w, 3,
        "configured W must reconcile the stored gate"
    );
    assert!(w.valid() && w.is_settled());
    assert!(
        w.nodes
            .iter()
            .filter(|n| !n.phase.is_closed())
            .all(|n| n.holds && n.phase == Phase::Making)
    );
    let after = c.snapshot().unwrap();
    assert_eq!(after.revision.get(), before.revision.get() + 1);
    let added: Vec<_> = after
        .pending_effects
        .iter()
        .filter(|e| !before.pending_effects.iter().any(|old| old.key == e.key))
        .collect();
    assert_eq!(added.len(), 2, "both admitted leaves need durable effects");
    assert!(added.iter().all(|e| e.created_revision == after.revision));
    assert_eq!(after.credit, before.credit);
    after
}
pub(super) fn reopen(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let before = seed(settings, root.path(), 1);
    let mut c = open_at(settings, root.path(), 3);
    let after = verify_migrated(&mut c, &before);
    close(c, settings);
    let mut c = open_at(settings, root.path(), 3);
    assert_eq!(
        c.snapshot().unwrap(),
        after,
        "second open must write nothing"
    );
}
pub(super) fn downgrade(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let before = seed(settings, root.path(), 3);
    let result = Controller::open(
        Store::open(settings).unwrap(),
        registry(root.path()),
        boot(1),
    );
    let Err(error) = result else {
        panic!("active gate capacity must not be silently lowered");
    };
    assert!(
        error.to_string().contains("limits.w") && error.to_string().contains("in flight"),
        "{error}"
    );
    assert_eq!(Store::open(settings).unwrap().load().unwrap(), before);
}
pub(super) fn idle_downgrade(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let mut c = open_at(settings, root.path(), 3);
    let before = c.snapshot().unwrap();
    close(c, settings);
    let mut c = open_at(settings, root.path(), 1);
    assert_eq!(c.world().unwrap().gates[0].w, 1);
    assert_eq!(
        c.snapshot().unwrap().revision.get(),
        before.revision.get() + 1
    );
    let after = c.snapshot().unwrap();
    close(c, settings);
    assert_eq!(open_at(settings, root.path(), 1).snapshot().unwrap(), after);
}
pub(super) fn child(args: &[String]) {
    let settings: tl3_config::Store = serde_json::from_slice(&fs::read(&args[2]).unwrap()).unwrap();
    let root = Path::new(&args[3]);
    let point = args[4].clone();
    let mut store = Store::open(&settings).unwrap();
    store.set_boundary_hook(move |at, key, _| {
        if key.starts_with("controller:gates:") && format!("{at:?}") == point {
            println!("MIGRATION_READY");
            std::io::stdout().flush().unwrap();
            loop {
                std::thread::park();
            }
        }
    });
    let mut bootstrap = boot(3);
    bootstrap.pool = args[5].parse().unwrap();
    Controller::open(store, registry(root), bootstrap).unwrap();
}
pub(super) fn crash(settings: &tl3_config::Store, point: &str) {
    crash_with_pool(settings, point, false);
}
pub(super) fn pool_crash(settings: &tl3_config::Store, point: &str) {
    crash_with_pool(settings, point, true);
}
fn crash_with_pool(settings: &tl3_config::Store, point: &str, pool: bool) {
    let root = tempfile::tempdir().unwrap();
    let before = if pool {
        seed_agents(settings, root.path(), 1, 1)
    } else {
        seed(settings, root.path(), 1)
    };
    let config = root.path().join("store.json");
    fs::write(
        &config,
        serde_json::to_vec(&json!({
            "host":settings.host, "port":settings.port, "database":settings.database,
            "user":settings.user, "password_file":settings.password_file,
            "connect_timeout_secs":settings.connect_timeout_secs,
        }))
        .unwrap(),
    )
    .unwrap();
    let mut process = Command::new(env::current_exe().unwrap())
        .arg("--gate-migration-child")
        .arg(&config)
        .arg(root.path())
        .arg(point)
        .arg(if pool { "3" } else { "4" })
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut ready = false;
    for line in BufReader::new(process.stdout.take().unwrap()).lines() {
        if line.unwrap() == "MIGRATION_READY" {
            ready = true;
            break;
        }
    }
    if !ready {
        let status = process.wait().unwrap();
        panic!("migration never reached {point}; child exited {status}");
    }
    let during = Store::open(settings).unwrap().load().unwrap();
    if point != "AfterCommit" {
        assert_eq!(
            during, before,
            "uncommitted capacity or effects became visible"
        );
    }
    process.kill().unwrap();
    let status = process.wait().unwrap();
    use std::os::unix::process::ExitStatusExt;
    assert_eq!(status.signal(), Some(9));
    controller_lock_barrier(settings);
    let capacity = if pool { 3 } else { 4 };
    let mut c = open_with_pool(settings, root.path(), 3, capacity);
    let after = if pool {
        verify_agents(&mut c, &before)
    } else {
        verify_migrated(&mut c, &before)
    };
    close(c, settings);
    let mut c = open_with_pool(settings, root.path(), 3, capacity);
    assert_eq!(
        c.snapshot().unwrap(),
        after,
        "crash recovery duplicated the migration"
    );
}

fn seed_agents(settings: &tl3_config::Store, root: &Path, slots: u8, pool: u8) -> Snapshot {
    let mut c = open_with_pool(settings, root, slots, pool);
    for number in 1..=3 {
        c.intake(LabelledIssue {
            delivery: format!("agent-{number}"),
            owner: "owner".into(),
            repo: "gate-migration".into(),
            installation: 1,
            number,
            title: format!("Agent waiter {number}"),
            actor: 42,
            label: "tl3".into(),
        })
        .unwrap();
        drain(&mut c);
    }
    let world = c.world().unwrap();
    assert_eq!(
        world.nodes.iter().filter(|n| n.pool).count(),
        usize::from(pool.min(3))
    );
    assert!(world.nodes.iter().all(|n| matches!(
        n.phase,
        Phase::Ingesting | Phase::Auditing | Phase::Planning { .. }
    )));
    close(c, settings)
}
fn verify_agents(c: &mut Controller, before: &Snapshot) -> Snapshot {
    let w = c.world().unwrap();
    assert!(
        w.nodes.iter().filter(|n| n.pool).count() > 1,
        "configured agent pool must start more than one agent run"
    );
    assert_eq!(w.pool_capacity, 3);
    assert_eq!(w.gates[0].w, 3);
    assert!(w.valid() && w.is_settled());
    assert!(w.nodes.iter().all(|n| n.pool && n.run.is_some()));
    let after = c.snapshot().unwrap();
    assert_eq!(
        after.revision.get(),
        before.revision.get() + 1,
        "W and pool must migrate in one commit"
    );
    let added: Vec<_> = after
        .pending_effects
        .iter()
        .filter(|e| !before.pending_effects.iter().any(|old| old.key == e.key))
        .collect();
    assert_eq!(added.len(), 2);
    assert!(added.iter().all(|e| {
        e.created_revision == after.revision
            && matches!(
                serde_json::from_value::<Delivery>(e.payload.clone())
                    .unwrap()
                    .output,
                Out::Effect {
                    kind: EffectKind::StartRun(_),
                    ..
                }
            )
    }));
    assert_eq!(after.credit, before.credit);
    after
}
pub(super) fn pool_reopen(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let before = seed_agents(settings, root.path(), 1, 1);
    let mut c = open_with_pool(settings, root.path(), 3, 3);
    let after = verify_agents(&mut c, &before);
    drain(&mut c);
    assert_eq!(
        c.world()
            .unwrap()
            .nodes
            .iter()
            .filter(|n| n.run.is_some())
            .count(),
        3
    );
    assert!(
        c.world()
            .unwrap()
            .nodes
            .iter()
            .all(|n| matches!(n.phase, Phase::Planning { .. })),
        "all three planners must be running together"
    );
    let dispatched = close(c, settings);
    assert!(dispatched.revision >= after.revision);
    let mut c = open_with_pool(settings, root.path(), 3, 3);
    assert_eq!(
        c.snapshot().unwrap(),
        dispatched,
        "second open must write nothing"
    );
}
pub(super) fn pool_downgrade(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let before = seed_agents(settings, root.path(), 1, 3);
    let mut bootstrap = boot(3);
    bootstrap.pool = 1;
    let result = Controller::open(
        Store::open(settings).unwrap(),
        registry(root.path()),
        bootstrap,
    );
    let Err(error) = result else {
        panic!("active agent capacity must not be silently lowered");
    };
    assert!(
        error.to_string().contains("agent_pool") && error.to_string().contains("in flight"),
        "{error}"
    );
    assert_eq!(
        Store::open(settings).unwrap().load().unwrap(),
        before,
        "neither W nor pool may partially commit"
    );
}
pub(super) fn pool_idle_downgrade(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let c = open_with_pool(settings, root.path(), 3, 3);
    let before = close(c, settings);
    let mut c = open_with_pool(settings, root.path(), 3, 1);
    assert_eq!(c.world().unwrap().pool_capacity, 1);
    let after = close(c, settings);
    assert_eq!(after.revision.get(), before.revision.get() + 1);
    let mut c = open_with_pool(settings, root.path(), 3, 1);
    assert_eq!(c.snapshot().unwrap(), after);
}
pub(super) fn ended_look_and_gate(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let (before, ended) = seed_with_look(settings, root.path(), 1, 4);
    let mut store = Store::open(settings).unwrap();
    let mut commit = Commit::new("legacy-ended-plan-look", json!({"fixture": true}), &before);
    let key = ended.id.clone();
    commit.rows.push(RowChange::Put {
        table: ended.table,
        id: ended.id,
        value: ended.value,
    });
    store.commit(&commit).unwrap();
    let before = store.load().unwrap();
    drop(store);
    let mut c = open_at(settings, root.path(), 3);
    let after = verify_migrated(&mut c, &before);
    assert!(
        !after
            .rows
            .iter()
            .any(|r| r.table == "allowances" && r.id == key),
        "startup must retire the ended look while admitting waiters"
    );
    close(c, settings);
    let mut c = open_at(settings, root.path(), 3);
    assert_eq!(c.snapshot().unwrap(), after);
}

pub(super) fn q48_ticket_order(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let mut c = open_at(settings, root.path(), 1);
    for number in 1..=4 {
        c.intake(LabelledIssue {
            delivery: format!("q48-order-{number}"),
            owner: "owner".into(),
            repo: "gate-migration".into(),
            installation: 1,
            number,
            title: format!("Ticket order {number}"),
            actor: 42,
            label: "tl3".into(),
        })
        .unwrap();
        drain(&mut c);
    }
    for (node, files) in [
        (0, vec!["0"]),
        (3, vec!["0", "1"]),
        (1, vec!["1", "2"]),
        (2, vec!["5"]),
    ] {
        let generation = c.world().unwrap().nodes[node].generation;
        c.observe(
            &format!("q48-order-plan-{node}"),
            Observation::Plan {
                node: NodeId(node as u64),
                generation,
                paths: files.into_iter().map(str::to_owned).collect(),
                fits: true,
            },
        )
        .unwrap();
        drain(&mut c);
    }
    let before = c.world().unwrap();
    let oldest = before.nodes[3].ticket.unwrap();
    assert!(oldest < before.nodes[1].ticket.unwrap());
    assert!(before.nodes[1].ticket < before.nodes[2].ticket);
    assert_eq!(before.nodes.iter().filter(|n| n.holds).count(), 1);
    close(c, settings);

    let mut c = open_at(settings, root.path(), 3);
    let w = c.world().unwrap();
    assert!(
        w.nodes[2].holds,
        "W reconciliation must admit the truly disjoint leaf"
    );
    assert!(!w.nodes[1].holds && !w.nodes[3].holds);
    assert_eq!(w.nodes[3].ticket, Some(oldest));
    let after = close(c, settings);
    let mut c = open_at(settings, root.path(), 3);
    assert_eq!(c.snapshot().unwrap(), after);
    c.observe(
        "q48-release-blocker",
        Observation::Kernel(Event::Command {
            node: NodeId(0),
            id: 1,
            command: Control::Stop,
        }),
    )
    .unwrap();
    drain(&mut c);
    let w = c.world().unwrap();
    assert!(w.nodes[0].phase.is_closed());
    assert!(w.nodes[3].holds, "the oldest ticket gets the spare slot");
    assert!(w.nodes[2].holds && !w.nodes[1].holds);
    assert!(w.valid() && w.is_settled());
}
