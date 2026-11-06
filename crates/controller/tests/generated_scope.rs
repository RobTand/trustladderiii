#[path = "support/ownership.rs"]
mod startup_ownership;
#[path = "../../store/tests/support/mod.rs"]
mod support;
#[allow(dead_code)]
mod worktree_fixture;

use serde_json::{Value, json};
use tl3_controller::{
    Bootstrap, Controller, LabelledIssue, Repository, adapters,
    executor::{Delivery, Executor, Observation},
};
use tl3_kernel::{NodeId, Phase};
use tl3_store::{Commit, RowChange, Store};

const OUTPUTS: [&str; 4] = [
    "BUILD.bazel",
    "src/prismabuild/BUILD.bazel",
    "tests/BUILD.bazel",
    "tools/BUILD.bazel",
];
const RECEIPT: [&str; 2] = [
    "src/prismabuild/generation_receipt.py",
    "tests/test_generation_receipt_capture.py",
];
const DIGEST: [&str; 2] = [
    "src/prismabuild/dependency_digest.py",
    "tests/test_dependency_digest_contract.py",
];

struct Fixture {
    controller: Option<Controller>,
    settings: tl3_config::Store,
    config: tl3_config::Config,
    root: tempfile::TempDir,
    _database: support::Database,
    sequence: usize,
}
impl Fixture {
    fn new(count: usize, patterns: &[&str], generator: bool) -> Self {
        let mut database = support::Database::start();
        let settings = database.create("generated_scope");
        let root = tempfile::tempdir().unwrap();
        let mut config =
            tl3_config::Config::parse(include_str!("../../config/example.toml")).unwrap();
        let repo = &mut config.github.repos[0];
        repo.owner = "owner".into();
        repo.name = "sandbox".into();
        repo.installation_id = 3;
        repo.generated_files = patterns.iter().map(|s| (*s).into()).collect();
        repo.generator = generator.then(|| "python3 tools/gen_manifest.py".into());
        let mut fixture = Self {
            controller: None,
            settings,
            config,
            root,
            _database: database,
            sequence: 0,
        };
        fixture.restart();
        for index in 0..count {
            fixture
                .c()
                .intake(LabelledIssue {
                    delivery: format!("label-{index}"),
                    owner: "owner".into(),
                    repo: "sandbox".into(),
                    installation: 3,
                    number: 2819 + index as u64,
                    title: "Generated scope".into(),
                    actor: 42,
                    label: "tl3".into(),
                    labels: vec![],
                })
                .unwrap();
        }
        for index in 0..count {
            fixture.arm(index, Phase::Planning { must_split: false });
        }
        fixture
    }
    fn c(&mut self) -> &mut Controller {
        self.controller.as_mut().unwrap()
    }
    fn restart(&mut self) {
        drop(self.controller.take());
        let mut controller = Controller::open(
            Store::open(&self.settings).unwrap(),
            adapters::simulation(self.root.path()).unwrap(),
            Bootstrap {
                repos: vec![Repository {
                    owner: "owner".into(),
                    name: "sandbox".into(),
                    installation: 3,
                }],
                rob: 42,
                slots: 3,
                pool: 16,
                fail_fast: false,
                operations: None,
            },
        )
        .unwrap();
        controller.set_repository_generation(&self.config.github.repos);
        self.controller = Some(controller);
    }
    fn arm(&mut self, index: usize, phase: Phase) {
        let mut node = self.c().world().unwrap().nodes[index];
        node.phase = phase;
        node.pos = if phase == Phase::Making {
            2
        } else if matches!(phase, Phase::Planning { .. }) {
            1
        } else {
            0
        };
        node.effect = None;
        node.ticket = None;
        node.job = None;
        node.run = if phase == Phase::Unplanned {
            None
        } else {
            Some(node.generation)
        };
        node.pool = node.run.is_some();
        node.holds = phase == Phase::Making;
        node.worktree = node.holds;
        node.paused = phase == Phase::Unplanned;
        node.waiting_incident = false;
        node.credit.ready = true;
        node.credit.start = 4;
        let mut store = Store::open(&self.settings).unwrap();
        let mut commit = Commit::new(
            format!("arm-{}", self.sequence),
            json!(null),
            &store.load().unwrap(),
        );
        self.sequence += 1;
        commit.rows.push(RowChange::Put {
            table: "nodes".into(),
            id: index.to_string(),
            value: json!(node),
        });
        store.commit(&commit).unwrap();
    }
    fn observe(&mut self, observation: Observation) {
        self.sequence += 1;
        let key = format!("observation-{}", self.sequence);
        self.c().observe(&key, observation).unwrap();
        for _ in 0..50 {
            if !self
                .c()
                .snapshot()
                .unwrap()
                .rows
                .iter()
                .any(|r| r.table == "inbox")
            {
                return;
            }
            assert!(self.c().drive_once().unwrap());
        }
        panic!("observation was not consumed");
    }
    fn plan(&mut self, index: usize, files: &[String]) {
        let generation = self.c().world().unwrap().nodes[index].generation;
        self.observe(Observation::Plan {
            node: NodeId(index as u64),
            generation,
            paths: files.to_vec(),
            fits: true,
        });
    }
    fn scope(&mut self, index: usize) -> Option<Value> {
        self.c()
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .find(|r| r.table == "declared_scopes" && r.id == index.to_string())
            .map(|r| r.value.clone())
    }
    fn locks(&mut self, index: usize) -> Vec<String> {
        let world = self.c().world().unwrap();
        let snapshot = self.c().snapshot().unwrap();
        world
            .files(world.nodes[index].footprint)
            .unwrap()
            .map(|file| {
                snapshot
                    .interned
                    .iter()
                    .find(|row| row.kind == tl3_store::InternKind::Path && row.id == file.0 as i64)
                    .unwrap()
                    .value
                    .clone()
            })
            .collect()
    }
    fn delivery(&mut self, index: usize) -> Delivery {
        self.c()
            .snapshot()
            .unwrap()
            .pending_effects
            .iter()
            .filter_map(|e| serde_json::from_value::<Delivery>(e.payload.clone()).ok())
            .rfind(|d| d.node() == NodeId(index as u64))
            .expect("kernel delivery")
    }
}
fn files(authored: &[&str], generated: &[&str]) -> Vec<String> {
    authored
        .iter()
        .chain(generated)
        .map(|s| (*s).into())
        .collect()
}

/// #845/Q27: the production controller must not serialize disjoint authored work.
#[test]
fn pb_2819_and_2822_hold_slots_concurrently() {
    let mut f = Fixture::new(2, &["**/BUILD.bazel"], true);
    f.plan(0, &files(&RECEIPT, &OUTPUTS));
    f.plan(1, &files(&DIGEST, &OUTPUTS));
    let world = f.c().world().unwrap();
    assert!(
        world.nodes[0].holds && world.nodes[1].holds,
        "PB #2819 and #2822 must hold slots together"
    );
    assert_eq!(f.locks(0).len(), 2);
}
/// #845/Q48: real authored overlap still excludes the younger leaf.
#[test]
fn pb_2819_and_2821_still_serialize() {
    let mut f = Fixture::new(2, &["**/BUILD.bazel"], true);
    for index in 0..2 {
        f.plan(index, &files(&RECEIPT, &OUTPUTS));
    }
    let world = f.c().world().unwrap();
    assert!(world.nodes[0].holds);
    assert!(!world.nodes[1].holds);
    assert!(world.nodes[1].ticket.is_some());
}
/// #845: generated overlap cannot fill the planning wait queue.
#[test]
fn generated_overlap_does_not_close_planning_admission() {
    let mut f = Fixture::new(5, &["gen/**/*.json"], true);
    for index in 0..4 {
        f.plan(
            index,
            &files(&[&format!("src/{index}.rs")], &["gen/shared.json"]),
        );
    }
    f.arm(4, Phase::Unplanned);
    f.observe(Observation::Steering {
        node: NodeId(4),
        command: tl3_kernel::Command::Resume,
    });
    assert!(
        matches!(
            f.c().world().unwrap().nodes[4].phase,
            Phase::Planning { .. }
        ),
        "only one capacity waiter remains, so planning stays open"
    );
}
/// #845: generic generation keeps declared scope and excludes only generated locks.
#[test]
fn generic_generator_preserves_scope_delivery_and_restart() {
    let mut f = Fixture::new(1, &["gen/**/*.json"], true);
    let declared = files(
        &["src/input.rs"],
        &["gen/manifest.json", "gen/nested/other.json"],
    );
    let generation = f.c().world().unwrap().nodes[0].generation;
    f.plan(0, &declared);
    let key = format!("observation-{}", f.sequence);
    assert_eq!(
        f.scope(0),
        Some(json!(declared)),
        "accepted PlanLeaf persists full declared scope"
    );
    assert_eq!(f.locks(0), ["src/input.rs"]);
    let delivery = f.delivery(0);
    assert_eq!(delivery.paths, declared);
    let before = f.c().world().unwrap();
    f.restart();
    assert_eq!(f.c().world().unwrap(), before);
    assert_eq!(f.scope(0), Some(json!(declared)));
    let revision = f.c().snapshot().unwrap().revision;
    f.c()
        .observe(
            &key,
            Observation::Plan {
                node: NodeId(0),
                generation,
                paths: declared.clone(),
                fits: true,
            },
        )
        .unwrap();
    assert_eq!(f.c().snapshot().unwrap().revision, revision);
    assert_eq!(f.c().world().unwrap(), before);
    assert_eq!(f.scope(0), Some(json!(declared)));
}
/// #845: legacy and no-generator leaves retain their original exclusive scope.
#[test]
fn no_generator_and_generated_only_fallback_keep_full_locks() {
    for generator in [false, true] {
        let mut f = Fixture::new(1, &["gen/**/*.json"], generator);
        let declared = if generator {
            files(&[], &["gen/manifest.json"])
        } else {
            files(&["src/input.rs"], &["gen/manifest.json"])
        };
        f.plan(0, &declared);
        let mut expected = declared.clone();
        expected.sort();
        let mut actual = f.locks(0);
        actual.sort();
        assert_eq!(actual, expected);
    }
}
/// #845: stale and non-fitting replies do not replace committed scope.
#[test]
fn rejected_or_stale_plan_does_not_write_scope() {
    let mut f = Fixture::new(1, &["gen/**/*.json"], true);
    let generation = f.c().world().unwrap().nodes[0].generation;
    for (generation, fits) in [(generation + 1, true), (generation, false)] {
        f.observe(Observation::Plan {
            node: NodeId(0),
            generation,
            paths: files(&["src/input.rs"], &["gen/manifest.json"]),
            fits,
        });
        assert!(f.scope(0).is_none());
    }
}
/// #845: growing files must retain declared generated paths across restart.
#[test]
fn maker_growth_repartitions_full_declared_scope() {
    let mut f = Fixture::new(1, &["gen/**/*.json"], true);
    f.plan(0, &files(&["src/input.rs"], &["gen/manifest.json"]));
    f.arm(0, Phase::Making);
    let generation = f.c().world().unwrap().nodes[0].generation;
    f.observe(Observation::WorkerReply { node: NodeId(0), generation, role: "maker".into(), model: None, plan_check: None, reply: json!({"kind":"needs_files","files":[{"path":"src/extra.rs","why":"input"},{"path":"gen/extra.json","why":"output"}],"fits":true}) });
    assert_eq!(
        f.scope(0),
        Some(json!([
            "src/input.rs",
            "gen/manifest.json",
            "src/extra.rs",
            "gen/extra.json"
        ]))
    );
    let mut locks = f.locks(0);
    locks.sort();
    assert_eq!(locks, ["src/extra.rs", "src/input.rs"]);
    f.restart();
    assert_eq!(f.scope(0).unwrap().as_array().unwrap().len(), 4);
}
/// #845: changing a declared generated file is valid; a matching undeclared file is not.
#[test]
fn checks_authorize_only_declared_generated_outputs() {
    for (path, authorized) in [("gen/manifest.json", true), ("gen/undeclared.json", false)] {
        let mut f = Fixture::new(1, &["gen/**/*.json"], true);
        f.plan(0, &files(&["src/input.rs"], &["gen/manifest.json"]));
        let head = {
            let mut store = Store::open(&f.settings).unwrap();
            store.intern_commit("0", &"a".repeat(40)).unwrap()
        };
        let mut node = f.c().world().unwrap().nodes[0];
        node.phase = Phase::Checking;
        node.pos = 1;
        node.run = None;
        node.pool = false;
        node.job = Some(node.generation);
        node.effect = None;
        node.head = Some(tl3_kernel::HeadId(head.get() as u64));
        node.worktree = true;
        let mut store = Store::open(&f.settings).unwrap();
        let mut commit = Commit::new("checking", json!(null), &store.load().unwrap());
        commit.rows.push(RowChange::Put {
            table: "nodes".into(),
            id: "0".into(),
            value: json!(node),
        });
        store.commit(&commit).unwrap();
        f.observe(Observation::Checks {
            node: NodeId(0),
            generation: node.generation,
            sha: "a".repeat(40),
            changed_paths: vec![path.into()],
            failed_tests: vec![],
            report: true,
            pass: true,
            results: vec![],
            base_sha: "b".repeat(40),
            action_key: "scope-check".into(),
        });
        let world = f.c().world().unwrap();
        assert_eq!(matches!(world.nodes[0].phase, Phase::Reviewing), authorized);
    }
}

/// #845: an existing schema upgrades without inventing declared scopes for old leaves.
#[test]
fn declared_scope_migration_upgrades_without_legacy_backfill() {
    let mut f = Fixture::new(1, &["gen/**/*.json"], true);
    let before = f.c().world().unwrap();
    let mut client = postgres::Config::new()
        .host(&f.settings.host)
        .port(f.settings.port)
        .user(&f.settings.user)
        .dbname(&f.settings.database)
        .password("tl3-disposable-test-password")
        .connect(postgres::NoTls)
        .unwrap();
    client.batch_execute("ALTER TABLE tl3_world DROP CONSTRAINT IF EXISTS tl3_declared_scope_files; DELETE FROM tl3_schema_migrations WHERE version > 24").unwrap();
    drop(f.controller.take());
    let mut owner = tl3_store::Ownership::claim(Store::connect(&f.settings).unwrap()).unwrap();
    owner.migrate().unwrap();
    owner.release().unwrap();
    let mut upgraded = Store::open(&f.settings).unwrap();
    assert!(
        client
            .query_one(
                "SELECT EXISTS (SELECT 1 FROM tl3_schema_migrations WHERE version=25)",
                &[]
            )
            .unwrap()
            .get::<_, bool>(0),
        "declared scope migration 25 must run"
    );
    assert!(
        !upgraded
            .load()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.table == "declared_scopes")
    );
    f.restart();
    assert_eq!(f.c().world().unwrap(), before);
    let snapshot = upgraded.load().unwrap();
    let mut invalid = Commit::new("invalid-declared-scope", json!(null), &snapshot);
    invalid.rows.push(RowChange::Put {
        table: "declared_scopes".into(),
        id: "0".into(),
        value: json!([42]),
    });
    invalid.rows.push(RowChange::Put {
        table: "scope-rollback".into(),
        id: "0".into(),
        value: json!(true),
    });
    assert!(upgraded.commit(&invalid).is_err());
    assert_eq!(upgraded.load().unwrap(), snapshot);
}

/// #845: legacy leaves keep full locks through growth until a fresh plan.
#[test]
fn legacy_scope_and_locks_stay_identical_until_replan() {
    let mut f = Fixture::new(1, &["gen/**/*.json"], false);
    f.plan(0, &files(&["src/input.rs"], &["gen/manifest.json"]));
    let mut store = Store::open(&f.settings).unwrap();
    let mut commit = Commit::new("legacy-scope", json!(null), &store.load().unwrap());
    commit.rows.push(RowChange::Delete {
        table: "declared_scopes".into(),
        id: "0".into(),
    });
    store.commit(&commit).unwrap();
    f.config.github.repos[0].generator = Some("python3 tools/gen_manifest.py".into());
    f.restart();
    f.arm(0, Phase::Making);
    let generation = f.c().world().unwrap().nodes[0].generation;
    f.observe(Observation::WorkerReply {node:NodeId(0),generation,role:"maker".into(),model:None,plan_check:None,reply:json!({"kind":"needs_files","files":[{"path":"gen/extra.json","why":"output"}],"fits":true})});
    assert!(f.scope(0).is_none());
    let mut locks = f.locks(0);
    locks.sort();
    assert_eq!(
        locks,
        ["gen/extra.json", "gen/manifest.json", "src/input.rs"]
    );
    f.arm(0, Phase::Planning { must_split: false });
    f.plan(0, &files(&["src/replanned.rs"], &["gen/new.json"]));
    assert_eq!(
        f.scope(0),
        Some(json!(["src/replanned.rs", "gen/new.json"]))
    );
    assert_eq!(f.locks(0), ["src/replanned.rs"]);
}

/// #845/deferred-post-freeze: the unchanged kernel conservatively unions old fallback locks.
#[test]
fn generated_only_leaf_retains_old_locks_after_authored_growth() {
    let mut f = Fixture::new(1, &["gen/**/*.json"], true);
    f.plan(0, &["gen/manifest.json".into()]);
    f.arm(0, Phase::Making);
    let generation = f.c().world().unwrap().nodes[0].generation;
    f.observe(Observation::WorkerReply {node:NodeId(0),generation,role:"maker".into(),model:None,plan_check:None,reply:json!({"kind":"needs_files","files":[{"path":"src/extra.rs","why":"input"}],"fits":true})});
    let mut locks = f.locks(0);
    locks.sort();
    assert_eq!(locks, ["gen/manifest.json", "src/extra.rs"]);
}

/// #845: schema-validated worker planning uses the same declared scope boundary.
#[test]
fn planner_worker_reply_persists_full_scope() {
    let mut f = Fixture::new(1, &["gen/**/*.json"], true);
    let generation = f.c().world().unwrap().nodes[0].generation;
    f.observe(Observation::WorkerReply {node:NodeId(0),generation,role:"planner".into(),model:None,plan_check:None,reply:json!({"kind":"leaf","files":["src/input.rs","gen/manifest.json"],"checks":[{"kind":"regression","node_ids":["tests/test_input.py::test_input"],"goal_ids":[0]}],"no_design_questions":true,"lands_on_its_own":true,"violations":[],"forecast":{"tokenizer":"o200k_base","brief_tokens":1,"file_tokens":{"src/input.rs":0,"gen/manifest.json":0},"overhead_tokens":0,"tokens":1,"fit_tokens":100,"context_tokens":150,"fits":true}})});
    assert_eq!(
        f.scope(0),
        Some(json!(["src/input.rs", "gen/manifest.json"]))
    );
    assert_eq!(f.locks(0), ["src/input.rs"]);
}

proptest::proptest! {
    /// #845: pin the shared matcher and the index-to-path adapter with duplicates and order.
    #[test]
    fn generic_matcher_preserves_partition_occurrences(
        inputs in proptest::collection::vec((0u8..4, "[a-z]{1,12}", proptest::bool::ANY), 0..80),
        configured in proptest::bool::ANY,
    ) {
        let declared: Vec<String> = inputs.iter().map(|(kind, name, nested)| match kind {
            0 => format!("gen/{}{name}.json", if *nested { "nested/" } else { "" }),
            1 => format!("src/{name}.json"),
            2 => format!("gen/{name}.txt"),
            _ => "gen/repeated.json".to_owned(),
        }).collect();
        let expected: Vec<_> = inputs.iter().map(|(kind, _, _)| configured && (*kind == 0 || *kind == 3)).collect();
        let partition = tl3_config::partition_files(&declared, &["gen/**/*.json".into()], configured);
        let authored: Vec<_> = declared.iter().zip(&expected).filter(|(_, generated)| !**generated).map(|(path, _)| path.clone()).collect();
        let generated: Vec<_> = declared.iter().zip(&expected).filter(|(_, generated)| **generated).map(|(path, _)| path.clone()).collect();
        let locks = if authored.is_empty() { declared.clone() } else { authored.clone() };
        proptest::prop_assert_eq!(partition.authored, authored);
        proptest::prop_assert_eq!(partition.generated, generated);
        proptest::prop_assert_eq!(partition.locks, locks);
    }
}

/// #845: a rejected empty plan must not persist an authorization scope.
#[test]
fn empty_plan_does_not_persist_scope() {
    let mut f = Fixture::new(1, &["gen/**/*.json"], true);
    f.plan(0, &[]);
    assert!(f.scope(0).is_none());
}

/// #845: an unaffordable extension awaiting a new plan is not authorized growth.
#[test]
fn unaffordable_growth_keeps_the_previous_scope() {
    let mut f = Fixture::new(1, &["gen/**/*.json"], true);
    let declared = files(&["src/input.rs"], &["gen/manifest.json"]);
    f.plan(0, &declared);
    f.arm(0, Phase::Making);
    let mut node = f.c().world().unwrap().nodes[0];
    node.credit.spent = node.credit.start;
    let mut store = Store::open(&f.settings).unwrap();
    let mut commit = Commit::new("exhausted-credit", json!(null), &store.load().unwrap());
    commit.rows.push(RowChange::Put {
        table: "nodes".into(),
        id: "0".into(),
        value: json!(node),
    });
    store.commit(&commit).unwrap();
    f.observe(Observation::WorkerReply {node:NodeId(0),generation:node.generation,role:"maker".into(),model:None,plan_check:None,reply:json!({"kind":"needs_files","files":[{"path":"src/extra.rs","why":"input"}],"fits":true})});
    assert_eq!(f.scope(0), Some(json!(declared)));
    assert_eq!(f.locks(0), ["src/input.rs"]);
}

/// #845: trim uses controller scope, retains declared outputs, and still removes protected paths.
#[test]
fn generated_scope_trim_retains_only_declared_unprotected_paths() {
    let mut f = Fixture::new(1, &["gen/**/*.json"], true);
    f.plan(
        0,
        &files(&["shared.txt", "pyproject.toml"], &["gen/manifest.json"]),
    );
    let paths = f.delivery(0).paths;
    let root = tempfile::tempdir().unwrap();
    let worktree = worktree_fixture::Fixture::create(root.path());
    let epoch = f.c().snapshot().unwrap().epoch.get();
    let mut adapter = worktree.adapter_at(epoch);
    let mut delivery = worktree.delivery(tl3_kernel::EffectKind::PrepareWorktree, 0);
    delivery.epoch = epoch;
    delivery.paths = paths;
    adapter.execute(&delivery).unwrap();
    let checkout = adapter.checkout(&delivery);
    std::fs::create_dir_all(checkout.join("gen")).unwrap();
    for path in ["gen/manifest.json", "gen/undeclared.json", "pyproject.toml"] {
        std::fs::write(checkout.join(path), "generated or unrelated\n").unwrap();
    }
    worktree_fixture::commit(&checkout, "shared.txt", "authored\n");
    let trim = f
        .c()
        .trim_scope(
            &adapter,
            &delivery,
            &tl3_config::Planning::default().protected_paths,
        )
        .unwrap();
    assert_eq!(
        trim.removed_paths,
        ["gen/undeclared.json", "pyproject.toml"].map(std::path::PathBuf::from)
    );
    assert!(checkout.join("gen/manifest.json").exists());
    assert!(checkout.join("shared.txt").exists());
    assert!(!checkout.join("gen/undeclared.json").exists());
    assert!(!checkout.join("pyproject.toml").exists());
}

/// #845: a real replan resets persisted scope and replay accepts only the replacement plan.
#[test]
fn replan_resets_scope_before_restart_and_new_plan() {
    let mut f = Fixture::new(1, &["gen/**/*.json"], true);
    f.plan(0, &files(&["src/old.rs"], &["gen/old.json"]));
    f.arm(0, Phase::Making);
    let generation = f.c().world().unwrap().nodes[0].generation;
    f.observe(Observation::WorkerReply {node:NodeId(0),generation,role:"maker".into(),model:None,plan_check:None,reply:json!({"kind":"needs_files","files":[{"path":"src/extra.rs","why":"input"}],"fits":false})});
    assert!(matches!(
        f.c().world().unwrap().nodes[0].phase,
        Phase::Planning { .. }
    ));
    assert!(f.scope(0).is_none());
    f.restart();
    assert!(f.scope(0).is_none());
    let declared = files(&["src/new.rs"], &["gen/new.json"]);
    f.arm(0, Phase::Planning { must_split: false });
    f.plan(0, &declared);
    assert_eq!(f.scope(0), Some(json!(declared)));
    assert_eq!(f.locks(0), ["src/new.rs"]);
}
