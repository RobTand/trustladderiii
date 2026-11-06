#[path = "support/ownership.rs"]
mod startup_ownership;
#[allow(dead_code)]
#[path = "../../store/tests/support/mod.rs"]
mod support;
#[allow(dead_code)]
mod worktree_fixture;

use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};
use tl3_controller::{
    adapters::worktrees::Real,
    executor::{Delivery, Executor, Observation},
};
use tl3_kernel::EffectKind;
use worktree_fixture::{Fixture, commit, git};

const GENERATOR: &str = r#"import json
from pathlib import Path
values = [int(p.read_text()) for p in sorted(Path('src').glob('*.txt'))]
Path('gen/manifest.json').write_text(json.dumps({'count': sum(values)}) + '\n')
"#;

// Only the fleet transport/container seam is replaced. The existing executor,
// real configured command, artifact validation and journaled import all run.
const TRANSPORT: &str = r#"import hashlib, json, sys
from pathlib import Path
from tl3_harness import generator
from tl3_workers import pb_generators
key = 'a' * 64
ending = None
generator.require_admission = lambda: None
generator.container_command = lambda tree, container, argv, cidfile: argv
def command(args):
    global ending
    if args[1].endswith('pbrun.py'):
        root = Path(args[args.index('--cwd') + 1])
        contract = json.loads((root / 'generator.json').read_text())
        result = generator.run(root / 'checkout', generator.Container('sha256:' + '1' * 64), **contract)
        raw = (generator.PREFIX + json.dumps(result) + '\n').encode()
        log = Path(sys.argv[-1]).parent / 'artifact.log'
        log.write_bytes(raw)
        ending = {'action_key': key, 'status': 'executed', 'exit_code': 0, 'succeeded': True,
                  'result': {'verified': True, 'receipt_sha256': 'b' * 64,
                             'stdout': {'path': str(log), 'bytes': len(raw), 'sha256': hashlib.sha256(raw).hexdigest()}}}
        return 0, {'schema': 'prismabuild.submission.v1', 'action_key': key}
    return 0, {'schema': 'prismabuild.endings.v1', 'complete': True, 'endings': [ending]}
pb_generators._json_command = command
pb_generators.main()
"#;

struct Generated {
    _root: tempfile::TempDir,
    f: Fixture,
    adapter: Real,
    prepare: Delivery,
    checkout: PathBuf,
    config: tl3_config::Config,
}
impl Generated {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let mut f = Fixture::create(root.path());
        for dir in ["src", "gen", "tools"] {
            fs::create_dir(f.source.join(dir)).unwrap();
        }
        fs::write(f.source.join("src/a.txt"), "0\n").unwrap();
        fs::write(f.source.join("src/b.txt"), "0\n").unwrap();
        fs::write(f.source.join("gen/manifest.json"), "{\"count\": 0}\n").unwrap();
        commit(&f.source, "tools/gen_manifest.py", GENERATOR);
        git(&f.source, &["push", f.remote.to_str().unwrap(), "main"]);
        let repo = &mut f.repositories[0];
        repo.generated_files = vec!["gen/**/*.json".into()];
        repo.generator = Some("python3 tools/gen_manifest.py".into());
        let mut config =
            tl3_config::Config::parse(include_str!("../../config/example.toml")).unwrap();
        config.github.repos = f.repositories.clone();
        config.prismabuild.container_image = format!("sha256:{}", "1".repeat(64));
        config.planning.protected_paths.clear();
        let python =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../workers/.venv/bin/python");
        let script = root.path().join("transport.py");
        fs::write(&script, TRANSPORT).unwrap();
        let launcher = root.path().join("python");
        fs::write(
            &launcher,
            format!(
                "#!/bin/sh\nexec '{}' '{}' \"$@\"\n",
                python.display(),
                script.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&launcher, fs::Permissions::from_mode(0o755)).unwrap();
        config.workers.python = launcher;
        let mut adapter = f.adapter().with_generation(&config).unwrap();
        let mut prepare = f.delivery(EffectKind::PrepareWorktree, 1);
        prepare.epoch = 0;
        prepare.key = "fixture:prepare:0".into();
        prepare.paths = vec!["src/a.txt".into(), "gen/manifest.json".into()];
        adapter.await_execute(&prepare).unwrap();
        let checkout = adapter.checkout(&prepare);
        Self {
            _root: root,
            f,
            adapter,
            prepare,
            checkout,
            config,
        }
    }
    fn rebase(&self, seq: u64) -> Delivery {
        let mut delivery = self.f.delivery(EffectKind::StartRebase, seq);
        delivery.epoch = 0;
        delivery.paths = self.prepare.paths.clone();
        delivery.head_sha = Some(self.f.head(&delivery));
        delivery
    }
    fn upstream(&self) {
        commit(&self.f.source, "src/b.txt", "1\n");
        // The upstream independently generated the same count as our leaf.
        commit(&self.f.source, "gen/manifest.json", "{\"count\": 1}\n");
        git(
            &self.f.source,
            &["push", self.f.remote.to_str().unwrap(), "main"],
        );
    }
    fn leaf(&self) {
        commit(&self.checkout, "src/a.txt", "1\n");
        commit(&self.checkout, "gen/manifest.json", "{\"count\": 1}\n");
    }
}

#[test]
fn clean_rebase_regenerates_exact_combined_tree_before_observing() {
    let mut g = Generated::new();
    g.leaf();
    g.upstream();
    let delivery = g.rebase(2);
    let completion = g.adapter.await_execute(&delivery).unwrap();
    let head = completion
        .observations
        .iter()
        .find_map(|o| match o {
            Observation::Rebased { sha, .. } => Some(sha),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        git(&g.f.repo, &["show", &format!("{head}:gen/manifest.json")]),
        "{\"count\": 2}",
        "a clean rebase must regenerate before Rebased"
    );
    assert_eq!(git(&g.checkout, &["status", "--porcelain"]), "");
    assert_eq!(
        serde_json::to_value(g.adapter.await_execute(&delivery).unwrap()).unwrap(),
        serde_json::to_value(completion).unwrap()
    );
}

#[test]
fn independent_freshness_check_refuses_stale_output_without_mutating_leaf() {
    let mut g = Generated::new();
    commit(&g.checkout, "src/a.txt", "1\n");
    let delivery = g.rebase(2);
    let before = g.f.head(&delivery);
    let report = g
        .adapter
        .await_check_generation(&delivery)
        .unwrap()
        .expect("configured generation must be independently checked");
    assert!(report.completed);
    assert_eq!(report.changed, vec!["gen/manifest.json"]);
    assert_eq!(g.f.head(&delivery), before);
    assert_eq!(
        fs::read_to_string(g.checkout.join("gen/manifest.json")).unwrap(),
        "{\"count\": 0}\n"
    );
}

#[test]
fn failing_generator_cannot_report_a_clean_rebase() {
    let mut g = Generated::new();
    commit(
        &g.checkout,
        "tools/gen_manifest.py",
        "raise SystemExit(7)\n",
    );
    g.upstream();
    let delivery = g.rebase(2);
    let completion = g.adapter.await_execute(&delivery).unwrap();
    assert!(
        !completion
            .observations
            .iter()
            .any(|o| matches!(o, Observation::Rebased { .. })),
        "generator failure must become RebaseNeedsChanges, never RebaseClean"
    );
    assert!(
        !completion.observations.is_empty(),
        "failure must reach the leaf's repair path"
    );
}

#[test]
fn queue_failure_updates_and_regenerates_before_maker_repair() {
    let mut g = Generated::new();
    g.leaf();
    g.upstream();
    let delivery = g.rebase(2);
    let report = g
        .adapter
        .await_prepare_generated_repair(&delivery)
        .unwrap()
        .expect("queue repair must update configured generator branches");
    assert!(report.completed, "{report:?}");
    assert_eq!(
        git(
            &g.f.repo,
            &["show", &format!("{}:gen/manifest.json", report.head)]
        ),
        "{\"count\": 2}"
    );
    let mut check = delivery.clone();
    check.head_sha = Some(report.head);
    assert!(
        g.adapter
            .await_check_generation(&check)
            .unwrap()
            .unwrap()
            .changed
            .is_empty()
    );
}

use serde_json::json;
use tl3_controller::{
    Bootstrap, Controller, LabelledIssue, adapters,
    executor::{Completion, Reconciliation, Upstream},
};
use tl3_kernel::{HeadId, NodeId, Phase};
use tl3_store::{Commit, RowChange, Store};

type Dispatches = std::sync::Arc<std::sync::Mutex<Vec<(Delivery, String, String)>>>;
type Checkouts = std::sync::Arc<std::sync::Mutex<std::collections::BTreeMap<u64, PathBuf>>>;

struct Controlled {
    c: Controller,
    settings: tl3_config::Store,
    _db: support::Database,
    g: Generated,
    sequence: u64,
    dispatched: Dispatches,
    checkouts: Checkouts,
}
impl Controlled {
    fn new(mut g: Generated) -> Self {
        let mut db = support::Database::start();
        let settings = db.create("regeneration");
        fs::write(g._root.path().join("hold-merges"), "").unwrap();
        let mut registry = adapters::simulation(g._root.path()).unwrap();
        let adapter = std::mem::replace(
            &mut g.adapter,
            Real::new(g._root.path().join("unused"), 1).unwrap(),
        );
        registry.register(Upstream::Worktrees, adapter);
        let dispatched = std::sync::Arc::default();
        let checkouts: Checkouts = std::sync::Arc::default();
        checkouts.lock().unwrap().insert(0, g.checkout.clone());
        registry.register(
            Upstream::Workers,
            InspectMaker {
                checkouts: std::sync::Arc::clone(&checkouts),
                dispatched: std::sync::Arc::clone(&dispatched),
            },
        );
        let mut c = Controller::open(
            Store::open(&settings).unwrap(),
            registry,
            Bootstrap {
                repos: vec![worktree_fixture::identity()],
                rob: 42,
                slots: 3,
                pool: 16,
                fail_fast: false,
                operations: None,
            },
        )
        .unwrap();
        c.set_repository_generation(&g.config.github.repos);
        let identity = worktree_fixture::identity();
        c.intake(LabelledIssue {
            delivery: "generator-intake".into(),
            owner: identity.owner,
            repo: identity.name,
            installation: identity.installation,
            number: 42,
            title: "Generic generation".into(),
            actor: 42,
            label: "tl3".into(),
            labels: vec![],
        })
        .unwrap();
        let mut f = Self {
            c,
            settings,
            _db: db,
            g,
            sequence: 0,
            dispatched,
            checkouts,
        };
        f.arm(Phase::Planning { must_split: false }, None);
        let generation = f.c.world().unwrap().nodes[0].generation;
        f.observe(Observation::Plan {
            node: NodeId(0),
            generation,
            paths: f.g.prepare.paths.clone(),
            fits: true,
        });
        f
    }
    fn arm(&mut self, phase: Phase, sha: Option<&str>) {
        self.arm_at(0, phase, sha, self.g.prepare.paths.clone());
    }
    fn arm_at(&mut self, index: u64, phase: Phase, sha: Option<&str>, paths: Vec<String>) {
        let mut n = self.c.world().unwrap().nodes[index as usize];
        let mut store = Store::open(&self.settings).unwrap();
        n.phase = phase;
        n.pos = if matches!(
            phase,
            Phase::Checking | Phase::Planning { .. } | Phase::Rebasing
        ) {
            1
        } else {
            0
        };
        n.head = sha
            .map(|sha| HeadId(store.intern_commit(&index.to_string(), sha).unwrap().get() as u64));
        n.checked = None;
        n.approved = None;
        if matches!(phase, Phase::Queued | Phase::Publishing) {
            n.checked = n.head;
            n.approved = n.head;
        }
        n.effect = None;
        n.ticket = None;
        n.job = (phase == Phase::Checking).then_some(n.generation);
        n.run = matches!(phase, Phase::Planning { .. } | Phase::Rebasing).then_some(n.generation);
        n.pool = n.run.is_some() && phase != Phase::Rebasing;
        n.pr_open = phase == Phase::Queued;
        n.had_pr = n.pr_open;
        n.holds = !matches!(phase, Phase::Planning { .. });
        n.worktree = n.holds;
        n.paused = false;
        n.waiting_incident = false;
        n.credit.ready = true;
        n.credit.start = 8;
        let snapshot = store.load().unwrap();
        self.sequence += 1;
        let mut commit = Commit::new(format!("arm-{}", self.sequence), json!(null), &snapshot);
        commit.rows.push(RowChange::Put {
            table: "nodes".into(),
            id: index.to_string(),
            value: json!(n),
        });
        // The full declared scope is the authorization input, independently of locks.
        commit.rows.push(RowChange::Put {
            table: "declared_scopes".into(),
            id: index.to_string(),
            value: json!(paths),
        });
        store.commit(&commit).unwrap();
    }
    fn observe(&mut self, observation: Observation) {
        self.sequence += 1;
        self.c
            .observe(&format!("case-{}", self.sequence), observation)
            .unwrap();
        for _ in 0..1500 {
            if !self
                .c
                .snapshot()
                .unwrap()
                .rows
                .iter()
                .any(|row| row.table == "inbox" || row.table == "generation_checks")
            {
                return;
            }
            tick(&mut self.c);
        }
        panic!("observation not consumed");
    }
    fn check(&mut self, sha: &str) {
        self.check_at(0, sha, self.g.prepare.paths.clone());
    }
    fn check_at(&mut self, index: u64, sha: &str, paths: Vec<String>) {
        let generation = self.c.world().unwrap().nodes[index as usize].generation;
        self.observe(Observation::Checks {
            node: NodeId(index),
            generation,
            sha: sha.into(),
            changed_paths: paths,
            failed_tests: vec![],
            report: true,
            pass: true,
            results: vec![],
            base_sha: git(&self.g.f.source, &["rev-parse", "HEAD"]),
            action_key: "real-independent-check".into(),
        });
    }
}

#[test]
fn controller_checking_routes_stale_outputs_to_leaf_repair() {
    let g = Generated::new();
    let sha = commit(&g.checkout, "src/a.txt", "1\n");
    let mut f = Controlled::new(g);
    f.arm(Phase::Checking, Some(&sha));
    f.check(&sha);
    let n = f.c.world().unwrap().nodes[0];
    assert_ne!(
        n.checked, n.head,
        "stale generated output must not gain a checked marker"
    );
    assert_ne!(n.phase, Phase::Reviewing);
    let snapshot = f.c.snapshot().unwrap();
    let feedback = snapshot
        .rows
        .iter()
        .find(|row| row.table == "maker_findings")
        .unwrap()
        .value
        .to_string();
    assert!(
        feedback.contains("gen/manifest.json") && feedback.contains("run_generator"),
        "{feedback}"
    );
    assert!(
        !snapshot
            .pending_effects
            .iter()
            .any(|effect| effect.payload["output"]["Effect"]["kind"] == "Push")
    );
}

#[test]
fn controller_generator_failure_is_a_leaf_rebase_failure() {
    let mut g = Generated::new();
    commit(
        &g.checkout,
        "tools/gen_manifest.py",
        "raise SystemExit(7)\n",
    );
    g.upstream();
    let delivery = g.rebase(2);
    let old_head = delivery.head_sha.clone().unwrap();
    let completion = g.adapter.await_execute(&delivery).unwrap();
    let failed = completion
        .observations
        .into_iter()
        .find_map(|observation| match observation {
            Observation::RebaseGenerationFailed { report, .. } => Some(report),
            _ => None,
        })
        .expect("generation failure must be a leaf observation");
    let mut f = Controlled::new(g);
    f.arm(Phase::Rebasing, Some(&old_head));
    let generation = f.c.world().unwrap().nodes[0].generation;
    f.observe(Observation::RebaseGenerationFailed {
        node: NodeId(0),
        generation,
        report: failed,
    });
    assert!(matches!(
        f.c.world().unwrap().nodes[0].phase,
        Phase::Making | Phase::Ready(tl3_kernel::Resume::Making)
    ));
    assert!(f.c.snapshot().unwrap().rows.iter().any(
        |row| row.table == "maker_findings" && row.value.to_string().contains("run_generator")
    ));
}

struct InspectMaker {
    checkouts: Checkouts,
    dispatched: Dispatches,
}
impl Executor for InspectMaker {
    fn execute(&mut self, delivery: &Delivery) -> tl3_controller::Result<Completion> {
        if !matches!(
            delivery.output,
            tl3_controller::executor::Output::Effect {
                kind: EffectKind::StartRun(tl3_kernel::Role::Make),
                ..
            }
        ) {
            return Ok(Completion::confirmed(vec![]));
        }
        let checkouts = self.checkouts.lock().unwrap();
        let checkout = checkouts.get(&delivery.node().0).unwrap();
        self.dispatched.lock().unwrap().push((
            delivery.clone(),
            git(checkout, &["rev-parse", "HEAD"]),
            fs::read_to_string(checkout.join("gen/manifest.json")).unwrap(),
        ));
        Ok(Completion::confirmed(vec![]))
    }
    fn reconcile(&mut self, _: &Delivery) -> tl3_controller::Result<Reconciliation> {
        Ok(Reconciliation::Absent)
    }
}

#[test]
fn required_queue_check_refuses_stale_tree_then_updates_repairs_and_rechecks() {
    let g = Generated::new();
    g.leaf();
    let old = g.f.head(&g.prepare);
    let old_base = git(&g.checkout, &["rev-parse", "HEAD~2"]);
    g.upstream();
    let target = git(&g.f.source, &["rev-parse", "HEAD"]);
    let queue = g._root.path().join("queue");
    git(
        g._root.path(),
        &[
            "clone",
            g.f.remote.to_str().unwrap(),
            queue.to_str().unwrap(),
        ],
    );
    git(
        &queue,
        &[
            "fetch",
            g.f.repo.to_str().unwrap(),
            g.f.branch(&g.prepare).as_str(),
        ],
    );
    git(&queue, &["merge", "--no-edit", "FETCH_HEAD"]);
    assert!(
        std::process::Command::new("python3")
            .args(["tools/gen_manifest.py"])
            .current_dir(&queue)
            .status()
            .unwrap()
            .success()
    );
    let required = std::process::Command::new("git")
        .args(["diff", "--exit-code", "--", "gen/"])
        .current_dir(&queue)
        .output()
        .unwrap();
    assert!(
        !required.status.success(),
        "the required queue-tree check must refuse the clean but stale merge"
    );
    // The queue never pushes that rejected tree to main.
    assert_eq!(git(&g.f.remote, &["rev-parse", "main"]), target);
    let mut f = Controlled::new(g);
    f.arm(Phase::Queued, Some(&old));
    let run =
        serde_json::from_value(json!({"repository":f.g.checkout,"base_sha":old_base,"plan":[]}))
            .unwrap();
    f.observe(Observation::CheckPlan {
        node: NodeId(0),
        sha: old.clone(),
        run,
    });
    f.observe(Observation::CiFailed {
        node: NodeId(0),
        infra: false,
        checks: vec![tl3_controller::findings::CiFailure {
            name: "generated outputs".into(),
            summary: "Required queue-tree regeneration changed gen/manifest.json".into(),
            url: None,
        }],
    });
    assert_ne!(f.c.world().unwrap().nodes[0].phase, Phase::Queued);
    for _ in 0..1500 {
        if !f.dispatched.lock().unwrap().is_empty() {
            break;
        }
        tick(&mut f.c);
    }
    let (delivery, head, manifest) = f.dispatched.lock().unwrap().last().unwrap().clone();
    assert_eq!(
        manifest.trim(),
        "{\"count\": 2}",
        "update and generation must finish before maker dispatch"
    );
    assert_ne!(head, old);
    assert_eq!(
        delivery.checks.unwrap().base_sha.as_str(),
        target,
        "upstream files must not become out-of-scope leaf changes"
    );
    assert!(
        f.c.snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|row| row.table == "generation_repair_evidence")
    );
    let generation = f.c.world().unwrap().nodes[0].generation;
    f.observe(Observation::Made {
        node: NodeId(0),
        generation,
        sha: head.clone(),
    });
    assert_eq!(f.c.world().unwrap().nodes[0].phase, Phase::Checking);
    f.check(&head);
    let n = f.c.world().unwrap().nodes[0];
    assert_eq!(n.phase, Phase::Reviewing);
    assert_eq!(n.checked, n.head);
}

#[test]
fn queue_repair_preserves_non_generator_and_conflicted_update_routes() {
    for generator in [false, true] {
        let mut g = Generated::new();
        g.leaf();
        let old = g.f.head(&g.prepare);
        if generator {
            commit(&g.f.source, "src/a.txt", "2\n");
            git(&g.f.source, &["push", g.f.remote.to_str().unwrap(), "main"]);
        } else {
            g.upstream();
            g.config.github.repos[0].generator = None;
            g.adapter = g.adapter.with_generation(&g.config).unwrap();
        }
        let mut f = Controlled::new(g);
        f.arm(Phase::Queued, Some(&old));
        f.observe(Observation::CiFailed {
            node: NodeId(0),
            infra: false,
            checks: vec![],
        });
        for _ in 0..1500 {
            if !f.dispatched.lock().unwrap().is_empty() {
                break;
            }
            tick(&mut f.c);
        }
        let (_, head, manifest) = f.dispatched.lock().unwrap().last().unwrap().clone();
        assert_eq!(
            head, old,
            "non-generator/conflict must keep the existing maker route"
        );
        assert_eq!(manifest.trim(), "{\"count\": 1}");
        assert_eq!(git(&f.g.checkout, &["status", "--porcelain"]), "");
        if !generator {
            assert!(
                !f.c.snapshot()
                    .unwrap()
                    .rows
                    .iter()
                    .any(|row| row.table == "generation_repair_evidence")
            );
        }
    }
}

#[test]
fn generation_recovery_after_record_install_and_before_observation() {
    use tl3_controller::adapters::worktrees::Boundary;
    if let Some(root) = std::env::var_os("TL3_GENERATION_CRASH_ROOT") {
        let stage = std::env::var("TL3_GENERATION_CRASH_STAGE").unwrap();
        let mut f = Fixture::open(std::path::Path::new(&root));
        f.repositories[0].generated_files = vec!["gen/**/*.json".into()];
        f.repositories[0].generator = Some("python3 tools/gen_manifest.py".into());
        let mut config =
            tl3_config::Config::parse(include_str!("../../config/example.toml")).unwrap();
        config.github.repos = f.repositories.clone();
        config.workers.python = f.root.join("python");
        config.prismabuild.container_image = format!("sha256:{}", "1".repeat(64));
        config.planning.protected_paths.clear();
        let mut adapter = f.adapter().with_generation(&config).unwrap();
        let mut delivery = f.delivery(EffectKind::StartRebase, 2);
        delivery.epoch = 0;
        delivery.paths = vec!["src/a.txt".into(), "gen/manifest.json".into()];
        delivery.head_sha = Some(f.head(&delivery));
        await_generation(|| {
            adapter.execute_with(&delivery, |boundary| {
                if matches!(
                    (stage.as_str(), boundary),
                    ("record", Boundary::AfterGenerationRecorded)
                        | ("install", Boundary::AfterGenerationInstalled)
                        | ("observe", Boundary::AfterApply)
                ) {
                    std::process::exit(77);
                }
            })
        })
        .unwrap();
        panic!("crash boundary was not reached");
    }
    for stage in ["record", "install", "observe"] {
        let mut g = Generated::new();
        g.leaf();
        g.upstream();
        let delivery = g.rebase(2);
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "generation_recovery_after_record_install_and_before_observation",
                "--nocapture",
            ])
            .env("TL3_GENERATION_CRASH_ROOT", g._root.path())
            .env("TL3_GENERATION_CRASH_STAGE", stage)
            .output()
            .unwrap();
        assert_eq!(
            child.status.code(),
            Some(77),
            "{}",
            String::from_utf8_lossy(&child.stderr)
        );
        let completion = g.adapter.await_execute(&delivery).unwrap();
        let head = completion
            .observations
            .iter()
            .find_map(|observation| match observation {
                Observation::Rebased { sha, .. } => Some(sha),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            git(&g.f.repo, &["show", &format!("{head}:gen/manifest.json")]),
            "{\"count\": 2}"
        );
        assert_eq!(git(&g.checkout, &["status", "--porcelain"]), "");
        assert_eq!(
            serde_json::to_value(g.adapter.await_execute(&delivery).unwrap()).unwrap(),
            serde_json::to_value(completion).unwrap()
        );
    }
}

fn reviewed(f: &mut Controlled, index: u64, sha: &str) {
    for _ in 0..100 {
        let n = f.c.world().unwrap().nodes[index as usize];
        if n.phase == Phase::Reviewing && n.run == Some(n.generation) && n.pos == 1 {
            break;
        }
        assert!(f.c.drive_once().unwrap(), "review did not start: {n:?}");
    }
    let n = f.c.world().unwrap().nodes[index as usize];
    assert_eq!(n.run, Some(n.generation));
    assert_eq!(n.pos, 1);
    let generation = f.c.world().unwrap().nodes[index as usize].generation;
    f.observe(Observation::Reviewed {
        node: NodeId(index),
        generation,
        sha: sha.into(),
    });
    assert_eq!(
        f.c.world().unwrap().nodes[index as usize].phase,
        Phase::Publishing
    );
}
fn queued(f: &mut Controlled, indices: &[usize]) {
    for _ in 0..250 {
        let world = f.c.world().unwrap();
        if indices
            .iter()
            .all(|index| world.nodes[*index].phase == Phase::Queued)
        {
            return;
        }
        assert!(
            f.c.drive_once().unwrap(),
            "publication stopped before queue admission: nodes={:?}",
            f.c.world().unwrap().nodes
        );
    }
    panic!("publication did not reach the held queue");
}
fn required_generation_check(path: &std::path::Path) -> bool {
    std::process::Command::new("python3")
        .arg("tools/gen_manifest.py")
        .current_dir(path)
        .status()
        .unwrap()
        .success()
        && std::process::Command::new("git")
            .args(["diff", "--exit-code", "--quiet"])
            .current_dir(path)
            .status()
            .unwrap()
            .success()
        && git(path, &["ls-files", "--others", "--exclude-standard"]).is_empty()
}

#[test]
fn two_authored_leaves_hold_slots_rebase_regenerate_and_merge_fresh_trees() {
    let g = Generated::new();
    g.leaf();
    let mut f = Controlled::new(g);
    let paths = [
        vec!["src/a.txt".into(), "gen/manifest.json".into()],
        vec!["src/b.txt".into(), "gen/manifest.json".into()],
    ];
    let identity = worktree_fixture::identity();
    f.c.intake(LabelledIssue {
        delivery: "second-generator-leaf".into(),
        owner: identity.owner,
        repo: identity.name,
        installation: identity.installation,
        number: 43,
        title: "Disjoint authored source".into(),
        actor: 42,
        label: "tl3".into(),
        labels: vec![],
    })
    .unwrap();
    f.arm_at(
        1,
        Phase::Planning { must_split: false },
        None,
        paths[1].clone(),
    );
    let generation = f.c.world().unwrap().nodes[1].generation;
    f.observe(Observation::Plan {
        node: NodeId(1),
        generation,
        paths: paths[1].clone(),
        fits: true,
    });
    let world = f.c.world().unwrap();
    assert!(
        world.nodes[0].holds && world.nodes[1].holds,
        "disjoint authored leaves must hold slots concurrently"
    );
    let lock0: Vec<_> = world.files(world.nodes[0].footprint).unwrap().collect();
    let lock1: Vec<_> = world.files(world.nodes[1].footprint).unwrap().collect();
    assert!(
        !lock0.iter().any(|file| lock1.contains(file)),
        "shared generated outputs must not be exclusive locks"
    );
    let snapshot = f.c.snapshot().unwrap();
    assert_eq!(
        snapshot
            .rows
            .iter()
            .filter(|row| row.table == "declared_scopes"
                && row
                    .value
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|path| path == "gen/manifest.json"))
            .count(),
        2
    );

    let mut real = f.g.f.adapter().with_generation(&f.g.config).unwrap();
    let mut second = f.g.prepare.clone();
    second.key = "e2e:prepare:1".into();
    second.issue = 43;
    second.paths = paths[1].clone();
    second.output = tl3_controller::executor::Output::Effect {
        node: NodeId(1),
        seq: 100,
        kind: EffectKind::PrepareWorktree,
        head: None,
    };
    real.await_execute(&second).unwrap();
    let checkout1 = real.checkout(&second);
    f.checkouts.lock().unwrap().insert(1, checkout1.clone());
    commit(&checkout1, "src/b.txt", "1\n");
    commit(&checkout1, "gen/manifest.json", "{\"count\": 1}\n");
    f.g.f.advance_main(false);
    let mut heads = Vec::new();
    for (index, initial) in [f.g.prepare.clone(), second.clone()]
        .into_iter()
        .enumerate()
    {
        let current = f.g.f.head(&initial);
        f.arm_at(
            index as u64,
            Phase::Rebasing,
            Some(&current),
            paths[index].clone(),
        );
        let generation = f.c.world().unwrap().nodes[index].generation;
        let mut rebase = initial;
        rebase.key = format!("e2e:rebase:{index}");
        rebase.generation = generation;
        rebase.head_sha = Some(current);
        rebase.output = tl3_controller::executor::Output::Effect {
            node: NodeId(index as u64),
            seq: 101,
            kind: EffectKind::StartRebase,
            head: None,
        };
        let completion = real.await_execute(&rebase).unwrap();
        let rebased = completion
            .observations
            .into_iter()
            .find(|o| matches!(o, Observation::Rebased { .. }))
            .unwrap();
        let head = match &rebased {
            Observation::Rebased { sha, .. } => sha.clone(),
            _ => unreachable!(),
        };
        f.observe(rebased);
        assert_eq!(f.c.world().unwrap().nodes[index].phase, Phase::Checking);
        f.check_at(index as u64, &head, paths[index].clone());
        assert_eq!(f.c.world().unwrap().nodes[index].phase, Phase::Reviewing);
        reviewed(&mut f, index as u64, &head);
        heads.push(head);
    }
    queued(&mut f, &[0, 1]);
    for (index, delivery) in [f.g.prepare.clone(), second.clone()].iter().enumerate() {
        assert_eq!(
            git(
                &f.g.f.remote,
                &["rev-parse", f.g.f.branch(delivery).as_str()]
            ),
            heads[index]
        );
    }
    // The required queue check accepts the first leaf's exact merge tree.
    git(
        &f.g.f.source,
        &[
            "fetch",
            f.g.f.remote.to_str().unwrap(),
            f.g.f.branch(&f.g.prepare).as_str(),
        ],
    );
    git(&f.g.f.source, &["merge", "--no-edit", "FETCH_HEAD"]);
    assert!(required_generation_check(&f.g.f.source));
    git(
        &f.g.f.source,
        &["push", f.g.f.remote.to_str().unwrap(), "main"],
    );
    let first_main = git(&f.g.f.remote, &["rev-parse", "main"]);
    f.observe(Observation::Merged {
        node: NodeId(0),
        sha: heads[0].clone(),
        checks_passed: true,
    });

    // Both independently fresh leaves merge cleanly, but the combined output is stale.
    let queue = f.g._root.path().join("second-queue-tree");
    git(
        f.g._root.path(),
        &[
            "clone",
            f.g.f.remote.to_str().unwrap(),
            queue.to_str().unwrap(),
        ],
    );
    git(
        &queue,
        &[
            "fetch",
            f.g.f.remote.to_str().unwrap(),
            f.g.f.branch(&second).as_str(),
        ],
    );
    git(&queue, &["merge", "--no-edit", "FETCH_HEAD"]);
    assert!(!required_generation_check(&queue));
    assert_eq!(
        git(&f.g.f.remote, &["rev-parse", "main"]),
        first_main,
        "stale queue tree must never reach main"
    );
    f.dispatched.lock().unwrap().clear();
    f.observe(Observation::CiFailed {
        node: NodeId(1),
        infra: false,
        checks: vec![tl3_controller::findings::CiFailure {
            name: "generated outputs".into(),
            summary: "gen/manifest.json is stale on the required queue tree".into(),
            url: None,
        }],
    });
    for _ in 0..150 {
        if f.dispatched
            .lock()
            .unwrap()
            .iter()
            .any(|(delivery, _, _)| delivery.node() == NodeId(1))
        {
            break;
        }
        tick(&mut f.c);
    }
    let (_, repaired, manifest) = f
        .dispatched
        .lock()
        .unwrap()
        .iter()
        .find(|(delivery, _, _)| delivery.node() == NodeId(1))
        .unwrap()
        .clone();
    assert_eq!(manifest.trim(), "{\"count\": 2}");
    let generation = f.c.world().unwrap().nodes[1].generation;
    f.observe(Observation::Made {
        node: NodeId(1),
        generation,
        sha: repaired.clone(),
    });
    f.check_at(1, &repaired, paths[1].clone());
    assert_eq!(f.c.world().unwrap().nodes[1].phase, Phase::Reviewing);
    reviewed(&mut f, 1, &repaired);
    queued(&mut f, &[1]);
    git(
        &f.g.f.source,
        &[
            "fetch",
            f.g.f.remote.to_str().unwrap(),
            f.g.f.branch(&second).as_str(),
        ],
    );
    git(&f.g.f.source, &["merge", "--no-edit", "FETCH_HEAD"]);
    assert!(required_generation_check(&f.g.f.source));
    git(
        &f.g.f.source,
        &["push", f.g.f.remote.to_str().unwrap(), "main"],
    );
    f.observe(Observation::Merged {
        node: NodeId(1),
        sha: repaired,
        checks_passed: true,
    });
    assert!(
        f.c.world()
            .unwrap()
            .nodes
            .iter()
            .take(2)
            .all(|n| n.merged.is_some())
    );
    assert_eq!(
        git(&f.g.f.remote, &["show", "main:gen/manifest.json"]),
        "{\"count\": 2}"
    );
    assert_eq!(git(&f.g.f.remote, &["show", "main:src/a.txt"]), "1");
    assert_eq!(git(&f.g.f.remote, &["show", "main:src/b.txt"]), "1");
}

#[test]
fn r1_blocked_generation_allows_another_input_and_stop() {
    let mut g = Generated::new();
    let script = g._root.path().join("transport.py");
    let started = g._root.path().join("generation-started");
    let released = g._root.path().join("generation-released");
    fs::write(&script, format!(
        "import time\nfrom pathlib import Path\nPath({:?}).touch()\nwhile not Path({:?}).exists(): time.sleep(0.01)\n{}",
        started.to_str().unwrap(), released.to_str().unwrap(), TRANSPORT
    )).unwrap();
    let sha = commit(&g.checkout, "src/a.txt", "1\n");
    let mut f = Controlled::new(g);
    f.arm(Phase::Checking, Some(&sha));
    let generation = f.c.world().unwrap().nodes[0].generation;
    f.c.observe(
        "blocked-check",
        Observation::Checks {
            node: NodeId(0),
            generation,
            action_key: "blocked-generator".into(),
            base_sha: git(&f.g.f.source, &["rev-parse", "HEAD"]),
            sha,
            changed_paths: f.g.prepare.paths.clone(),
            failed_tests: vec![],
            report: true,
            pass: true,
            results: vec![],
        },
    )
    .unwrap();
    let (done, receive) = std::sync::mpsc::channel();
    let release = released.clone();
    // A watchdog releases the fixture even if the old drive loop blocks.
    let watchdog = std::thread::spawn(move || {
        let _ = receive.recv_timeout(std::time::Duration::from_secs(5));
        fs::write(release, "").unwrap();
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    while !started.exists() && std::time::Instant::now() < deadline {
        f.c.drive_once().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let identity = worktree_fixture::identity();
    f.c.intake(LabelledIssue {
        delivery: "other-leaf-during-generation".into(),
        owner: identity.owner,
        repo: identity.name,
        installation: identity.installation,
        number: 43,
        title: "Other leaf".into(),
        actor: 42,
        label: "tl3".into(),
        labels: vec![],
    })
    .unwrap();
    f.c.observe(
        "stop-during-generation",
        Observation::Steering {
            node: NodeId(0),
            command: tl3_kernel::Command::Stop,
        },
    )
    .unwrap();
    for _ in 0..1500 {
        f.c.drive_once().unwrap();
        if !f
            .c
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|row| row.table == "inbox")
        {
            break;
        }
    }
    let responsive = started.exists() && !released.exists();
    done.send(()).ok();
    watchdog.join().unwrap();
    assert!(
        responsive,
        "generator admission/completion blocked the controller before another input or Stop could be processed"
    );
    let world = f.c.world().unwrap();
    assert!(
        world.nodes.len() >= 2,
        "another leaf must be admitted while generation is blocked"
    );
    assert_ne!(
        world.nodes[0].phase,
        Phase::Checking,
        "Stop must leave Checking while generation is blocked"
    );
}

#[test]
fn r1_mandatory_protection_survives_empty_configured_paths() {
    for path in ["pyproject.toml", ".github/generated.json"] {
        let mut g = Generated::new();
        let command = format!(
            "from pathlib import Path\np = Path({path:?})\np.parent.mkdir(parents=True, exist_ok=True)\np.write_text('generated\\n')\n"
        );
        commit(&g.checkout, "tools/gen_manifest.py", &command);
        g.config.github.repos[0].generated_files.push(path.into());
        g.adapter = g.adapter.with_generation(&g.config).unwrap();
        let mut delivery = g.rebase(2);
        delivery.paths.push(path.into());
        let completion = g.adapter.await_execute(&delivery).unwrap();
        assert!(
            completion
                .observations
                .iter()
                .any(|o| matches!(o, Observation::RebaseGenerationFailed { .. })),
            "mandatory protected output {path} must fail, even when declared"
        );
        assert!(
            !g.checkout.join(path).exists(),
            "protected output must never be installed"
        );
    }
}

#[test]
fn r1_undeclared_output_is_durable_failure_and_queue_repair_keeps_running() {
    for queue in [false, true] {
        let g = Generated::new();
        g.leaf();
        g.upstream();
        commit(
            &g.f.source,
            "tools/gen_manifest.py",
            &(GENERATOR.to_owned() + "Path('gen/new.json').write_text('{}\\n')\n"),
        );
        git(&g.f.source, &["push", g.f.remote.to_str().unwrap(), "main"]);
        let old = g.f.head(&g.prepare);
        let mut f = Controlled::new(g);
        if queue {
            f.arm(Phase::Queued, Some(&old));
            f.observe(Observation::CiFailed {
                node: NodeId(0),
                infra: false,
                checks: vec![],
            });
            for _ in 0..1500 {
                if !f.dispatched.lock().unwrap().is_empty() {
                    break;
                }
                tick(&mut f.c);
            }
            assert!(
                !f.dispatched.lock().unwrap().is_empty(),
                "queue failure must dispatch maker repair for scope growth"
            );
        } else {
            f.arm(Phase::Rebasing, Some(&old));
            let mut adapter = f.g.f.adapter().with_generation(&f.g.config).unwrap();
            let delivery = f.g.rebase(2);
            let completion = adapter
                .await_execute(&delivery)
                .expect("undeclared output must return failure evidence");
            let report = completion
                .observations
                .into_iter()
                .find_map(|o| match o {
                    Observation::RebaseGenerationFailed { report, .. } => Some(report),
                    _ => None,
                })
                .expect("must use RebaseNeedsChanges route");
            assert!(!report.completed && report.message.contains("gen/new.json"));
            let generation = f.c.world().unwrap().nodes[0].generation;
            f.observe(Observation::RebaseGenerationFailed {
                node: NodeId(0),
                generation,
                report,
            });
        }
        assert!(matches!(
            f.c.world().unwrap().nodes[0].phase,
            Phase::Making | Phase::Ready(tl3_kernel::Resume::Making)
        ));
        let snapshot = f.c.snapshot().unwrap();
        assert!(
            snapshot
                .rows
                .iter()
                .any(|row| ["generation_repair_evidence", "maker_findings"]
                    .contains(&row.table.as_str())
                    && row.value.to_string().contains("gen/new.json")),
            "repair must name the undeclared output"
        );
        assert!(
            !f.g.checkout.join("gen/new.json").exists(),
            "undeclared output must never be installed"
        );
    }
}

#[test]
fn r1_running_executor_survives_crash_and_replays_without_blocking() {
    if let Some(root) = std::env::var_os("TL3_RUNNING_GENERATOR_ROOT") {
        let mut f = Fixture::open(std::path::Path::new(&root));
        f.repositories[0].generated_files = vec!["gen/**/*.json".into()];
        f.repositories[0].generator = Some("python3 tools/gen_manifest.py".into());
        let mut config =
            tl3_config::Config::parse(include_str!("../../config/example.toml")).unwrap();
        config.github.repos = f.repositories.clone();
        config.workers.python = f.root.join("python");
        config.prismabuild.container_image = format!("sha256:{}", "1".repeat(64));
        config.planning.protected_paths.clear();
        let mut adapter = f.adapter().with_generation(&config).unwrap();
        let mut delivery = f.delivery(EffectKind::StartRebase, 2);
        delivery.epoch = 0;
        delivery.paths = vec!["src/a.txt".into(), "gen/manifest.json".into()];
        delivery.head_sha = Some(f.head(&delivery));
        loop {
            match adapter.execute(&delivery) {
                Err(tl3_controller::Error::GenerationPending) => {
                    std::thread::sleep(std::time::Duration::from_millis(10))
                }
                result => panic!(
                    "executor should stay blocked until the parent kills this controller: {result:?}"
                ),
            }
        }
    }
    let mut g = Generated::new();
    g.leaf();
    g.upstream();
    let started = g._root.path().join("executor-started");
    let released = g._root.path().join("executor-released");
    let block = format!(
        "        import time\n        Path({:?}).touch()\n        while not Path({:?}).exists(): time.sleep(0.01)\n",
        started.to_str().unwrap(),
        released.to_str().unwrap()
    );
    fs::write(
        g._root.path().join("transport.py"),
        TRANSPORT.replace("        root = Path", &(block + "        root = Path")),
    )
    .unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "r1_running_executor_survives_crash_and_replays_without_blocking",
            "--nocapture",
        ])
        .env("TL3_RUNNING_GENERATOR_ROOT", g._root.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while !started.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(started.exists(), "existing executor did not start");
    let delivery = g.rebase(2);
    let first = g.adapter.execute(&delivery);
    assert!(
        matches!(first, Err(tl3_controller::Error::GenerationPending)),
        "replay must yield while the old executor is live"
    );
    fs::write(&released, "").unwrap();
    let completion = g.adapter.await_execute(&delivery).unwrap();
    let head = completion
        .observations
        .iter()
        .find_map(|o| match o {
            Observation::Rebased { sha, .. } => Some(sha),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        git(&g.f.repo, &["show", &format!("{head}:gen/manifest.json")]),
        "{\"count\": 2}"
    );
    assert_eq!(git(&g.checkout, &["status", "--porcelain"]), "");
}

#[test]
fn r1_undeclared_freshness_output_routes_checks_to_repair() {
    let g = Generated::new();
    let sha = commit(
        &g.checkout,
        "tools/gen_manifest.py",
        &(GENERATOR.to_owned() + "Path('gen/new.json').write_text('{}\\n')\n"),
    );
    let mut f = Controlled::new(g);
    f.arm(Phase::Checking, Some(&sha));
    f.check(&sha);
    let n = f.c.world().unwrap().nodes[0];
    assert_ne!(n.checked, n.head);
    assert!(matches!(
        n.phase,
        Phase::Making | Phase::Ready(tl3_kernel::Resume::Making)
    ));
    assert!(
        f.c.snapshot()
            .unwrap()
            .rows
            .iter()
            .any(|row| row.table == "maker_findings"
                && row.value.to_string().contains("gen/new.json"))
    );
    assert!(!f.g.checkout.join("gen/new.json").exists());
}

fn await_generation<T>(
    mut operation: impl FnMut() -> tl3_controller::Result<T>,
) -> tl3_controller::Result<T> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        match operation() {
            Err(tl3_controller::Error::GenerationPending)
                if std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(std::time::Duration::from_millis(10))
            }
            result => return result,
        }
    }
}
trait AwaitGeneration {
    fn await_execute(&mut self, delivery: &Delivery) -> tl3_controller::Result<Completion>;
    fn await_check_generation(
        &mut self,
        delivery: &Delivery,
    ) -> tl3_controller::Result<Option<tl3_controller::generation::Report>>;
    fn await_prepare_generated_repair(
        &mut self,
        delivery: &Delivery,
    ) -> tl3_controller::Result<Option<tl3_controller::generation::Report>>;
}
impl AwaitGeneration for Real {
    fn await_execute(&mut self, delivery: &Delivery) -> tl3_controller::Result<Completion> {
        await_generation(|| self.execute(delivery))
    }
    fn await_check_generation(
        &mut self,
        delivery: &Delivery,
    ) -> tl3_controller::Result<Option<tl3_controller::generation::Report>> {
        await_generation(|| self.check_generation(delivery))
    }
    fn await_prepare_generated_repair(
        &mut self,
        delivery: &Delivery,
    ) -> tl3_controller::Result<Option<tl3_controller::generation::Report>> {
        await_generation(|| self.prepare_generated_repair(delivery))
    }
}
fn tick(controller: &mut Controller) {
    if !controller.drive_once().unwrap() {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn r1_failed_executor_evidence_is_bounded_and_retained() {
    let mut g = Generated::new();
    let script = g._root.path().join("transport.py");
    fs::write(&script, "import json\nprint(json.dumps({'status': 'failed', 'action_key': 'e' * 64, 'message': 'executor refused gen/manifest.json', 'diagnostics': 'x' * 20000}))\n").unwrap();
    let delivery = g.rebase(2);
    let completion = g.adapter.await_execute(&delivery).unwrap();
    assert!(
        completion
            .observations
            .iter()
            .any(|o| matches!(o, Observation::RebaseGenerationFailed { .. }))
    );
    let root = g.f.repo.join("tl3-controller/generation");
    let operation = fs::read_dir(&root).unwrap().next().unwrap().unwrap().path();
    let contract = fs::read_dir(operation)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let path = contract.join("failed.json");
    let bytes =
        fs::read(&path).expect("failure evidence promised by the repair message must be durable");
    assert!(bytes.len() < 20000, "failure evidence must be bounded");
    let evidence: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(evidence["action_key"], "e".repeat(64));
    assert_eq!(evidence["message"], "executor refused gen/manifest.json");
    assert!(evidence["diagnostics"].as_str().unwrap().len() <= 16000);
    g.adapter.await_execute(&delivery).unwrap();
    assert_eq!(
        fs::read(path).unwrap(),
        bytes,
        "replay must retain failure evidence"
    );
}
