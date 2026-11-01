use super::*;
use tl3_kernel::{Role, World};

struct Workers(adapters::workers::Fake);
impl Executor for Workers {
    fn execute(&mut self, delivery: &Delivery) -> tl3_controller::Result<Completion> {
        match delivery.output {
            Out::Effect { kind: EffectKind::StartRun(Role::Plan), .. } => {
                Ok(Completion::confirmed(vec![Observation::Plan {
                    node: delivery.node(),
                    generation: delivery.generation,
                    paths: vec![format!("leaf-{}.rs", delivery.node().0)],
                    fits: true,
                }]))
            }
            Out::Effect { kind: EffectKind::StartRun(Role::Make), .. } => {
                Ok(Completion::confirmed(vec![]))
            }
            _ => self.0.execute(delivery),
        }
    }
    fn reconcile(&mut self, delivery: &Delivery) -> tl3_controller::Result<Reconciliation> {
        self.0.reconcile(delivery)
    }
}
fn registry(root: &Path) -> Registry {
    let mut registry = adapters::simulation(root).unwrap();
    registry.register(Upstream::Workers, Workers(adapters::workers::Fake::new(root)));
    registry
}
fn boot(slots: u8) -> Bootstrap {
    Bootstrap {
        repos: vec![Repository { owner: "owner".into(), name: "gate-migration".into(), installation: 1 }],
        rob: 42,
        operations: None,
        slots,
        pool: 4,
    }
}
fn open_at(settings: &tl3_config::Store, root: &Path, slots: u8) -> Controller {
    Controller::open(Store::open(settings).unwrap(), registry(root), boot(slots)).unwrap()
}
fn seed(settings: &tl3_config::Store, root: &Path) -> Snapshot {
    let mut c = open_at(settings, root, 1);
    for number in 1..=3 {
        c.intake(LabelledIssue {
            delivery: format!("gate-{number}"), owner: "owner".into(), repo: "gate-migration".into(),
            installation: 1, number, title: format!("Disjoint leaf {number}"), actor: 42, label: "tl3".into(),
        }).unwrap();
        drain(&mut c);
    }
    let w = c.world().unwrap();
    assert_eq!(w.gates[0].w, 1);
    assert_eq!(w.nodes.iter().filter(|n| n.holds).count(), 1);
    assert!(w.nodes[1..].iter().all(|n| !n.holds && n.ticket.is_some()));
    c.snapshot().unwrap()
}
fn verify_migrated(c: &mut Controller, before: &Snapshot) -> Snapshot {
    let w: World = c.world().unwrap();
    assert_eq!(w.gates[0].w, 3, "configured W must reconcile the stored gate");
    assert!(w.valid() && w.is_settled());
    assert!(w.nodes.iter().all(|n| n.holds && n.phase == Phase::Making));
    let after = c.snapshot().unwrap();
    assert_eq!(after.revision.get(), before.revision.get() + 1);
    let added: Vec<_> = after.pending_effects.iter()
        .filter(|e| !before.pending_effects.iter().any(|old| old.key == e.key)).collect();
    assert_eq!(added.len(), 2, "both admitted leaves need durable effects");
    assert!(added.iter().all(|e| e.created_revision == after.revision));
    assert_eq!(after.credit, before.credit);
    after
}
pub(super) fn reopen(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let before = seed(settings, root.path());
    let mut c = open_at(settings, root.path(), 3);
    let after = verify_migrated(&mut c, &before);
    drop(c);
    let mut c = open_at(settings, root.path(), 3);
    assert_eq!(c.snapshot().unwrap(), after, "second open must write nothing");
}
pub(super) fn downgrade(settings: &tl3_config::Store) {
    let root = tempfile::tempdir().unwrap();
    let before = seed(settings, root.path());
    let mut c = open_at(settings, root.path(), 3);
    verify_migrated(&mut c, &before);
    let after = c.snapshot().unwrap();
    drop(c);
    let result = Controller::open(Store::open(settings).unwrap(), registry(root.path()), boot(1));
    let Err(error) = result else { panic!("active gate capacity must not be silently lowered"); };
    assert!(error.to_string().contains("limits.w") && error.to_string().contains("in flight"), "{error}");
    assert_eq!(Store::open(settings).unwrap().load().unwrap(), after);
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
            loop { std::thread::park(); }
        }
    });
    Controller::open(store, registry(root), boot(3)).unwrap();
}
pub(super) fn crash(settings: &tl3_config::Store, point: &str) {
    let root = tempfile::tempdir().unwrap();
    let before = seed(settings, root.path());
    let config = root.path().join("store.json");
    fs::write(&config, serde_json::to_vec(settings).unwrap()).unwrap();
    let mut process = Command::new(env::current_exe().unwrap())
        .arg("--gate-migration-child").arg(&config).arg(root.path()).arg(point)
        .stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn().unwrap();
    let mut ready = false;
    for line in BufReader::new(process.stdout.take().unwrap()).lines() {
        if line.unwrap() == "MIGRATION_READY" { ready = true; break; }
    }
    if !ready {
        let status = process.wait().unwrap();
        panic!("migration never reached {point}; child exited {status}");
    }
    let during = Store::open(settings).unwrap().load().unwrap();
    if point != "AfterCommit" {
        assert_eq!(during, before, "uncommitted capacity or effects became visible");
    }
    process.kill().unwrap();
    let status = process.wait().unwrap();
    use std::os::unix::process::ExitStatusExt;
    assert_eq!(status.signal(), Some(9));
    let mut c = open_at(settings, root.path(), 3);
    let after = verify_migrated(&mut c, &before);
    drop(c);
    let mut c = open_at(settings, root.path(), 3);
    assert_eq!(c.snapshot().unwrap(), after, "crash recovery duplicated the migration");
}
