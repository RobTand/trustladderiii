#[path = "support/ownership.rs"]
mod startup_ownership;
#[allow(dead_code)]
#[path = "../../store/tests/support/mod.rs"]
mod support;
#[allow(dead_code)]
mod worktree_fixture;

use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};
use tl3_controller::{adapters::worktrees::Real, executor::{Delivery, Executor, Observation}};
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
}
impl Generated {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let mut f = Fixture::create(root.path());
        for dir in ["src", "gen", "tools"] { fs::create_dir(f.source.join(dir)).unwrap(); }
        fs::write(f.source.join("src/a.txt"), "0\n").unwrap();
        fs::write(f.source.join("src/b.txt"), "0\n").unwrap();
        fs::write(f.source.join("gen/manifest.json"), "{\"count\": 0}\n").unwrap();
        commit(&f.source, "tools/gen_manifest.py", GENERATOR);
        git(&f.source, &["push", f.remote.to_str().unwrap(), "main"]);
        let repo = &mut f.repositories[0];
        repo.generated_files = vec!["gen/**/*.json".into()];
        repo.generator = Some("python3 tools/gen_manifest.py".into());
        let mut config = tl3_config::Config::parse(include_str!("../../config/example.toml")).unwrap();
        config.github.repos = f.repositories.clone();
        config.prismabuild.container_image = format!("sha256:{}", "1".repeat(64));
        config.planning.protected_paths.clear();
        let python = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../workers/.venv/bin/python");
        let script = root.path().join("transport.py");
        fs::write(&script, TRANSPORT).unwrap();
        let launcher = root.path().join("python");
        fs::write(&launcher, format!("#!/bin/sh\nexec '{}' '{}' \"$@\"\n", python.display(), script.display())).unwrap();
        fs::set_permissions(&launcher, fs::Permissions::from_mode(0o755)).unwrap();
        config.workers.python = launcher;
        let mut adapter = f.adapter().with_generation(&config).unwrap();
        let mut prepare = f.delivery(EffectKind::PrepareWorktree, 1);
        prepare.paths = vec!["src/a.txt".into(), "gen/manifest.json".into()];
        adapter.execute(&prepare).unwrap();
        let checkout = adapter.checkout(&prepare);
        Self { _root: root, f, adapter, prepare, checkout }
    }
    fn rebase(&self, seq: u64) -> Delivery {
        let mut delivery = self.f.delivery(EffectKind::StartRebase, seq);
        delivery.paths = self.prepare.paths.clone();
        delivery.head_sha = Some(self.f.head(&delivery));
        delivery
    }
    fn upstream(&self) {
        commit(&self.f.source, "src/b.txt", "1\n");
        // The upstream independently generated the same count as our leaf.
        commit(&self.f.source, "gen/manifest.json", "{\"count\": 1}\n");
        git(&self.f.source, &["push", self.f.remote.to_str().unwrap(), "main"]);
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
    let completion = g.adapter.execute(&delivery).unwrap();
    let head = completion.observations.iter().find_map(|o| match o { Observation::Rebased { sha, .. } => Some(sha), _ => None }).unwrap();
    assert_eq!(git(&g.f.repo, &["show", &format!("{head}:gen/manifest.json")]), "{\"count\": 2}", "a clean rebase must regenerate before Rebased");
    assert_eq!(git(&g.checkout, &["status", "--porcelain"]), "");
    assert_eq!(serde_json::to_value(g.adapter.execute(&delivery).unwrap()).unwrap(), serde_json::to_value(completion).unwrap());
}

#[test]
fn independent_freshness_check_refuses_stale_output_without_mutating_leaf() {
    let mut g = Generated::new();
    commit(&g.checkout, "src/a.txt", "1\n");
    let delivery = g.rebase(2);
    let before = g.f.head(&delivery);
    let report = g.adapter.check_generation(&delivery).unwrap().expect("configured generation must be independently checked");
    assert!(report.completed);
    assert_eq!(report.changed, vec!["gen/manifest.json"]);
    assert_eq!(g.f.head(&delivery), before);
    assert_eq!(fs::read_to_string(g.checkout.join("gen/manifest.json")).unwrap(), "{\"count\": 0}\n");
}

#[test]
fn failing_generator_cannot_report_a_clean_rebase() {
    let mut g = Generated::new();
    commit(&g.checkout, "tools/gen_manifest.py", "raise SystemExit(7)\n");
    g.upstream();
    let delivery = g.rebase(2);
    let completion = g.adapter.execute(&delivery).unwrap();
    assert!(!completion.observations.iter().any(|o| matches!(o, Observation::Rebased { .. })), "generator failure must become RebaseNeedsChanges, never RebaseClean");
    assert!(!completion.observations.is_empty(), "failure must reach the leaf's repair path");
}

#[test]
fn queue_failure_updates_and_regenerates_before_maker_repair() {
    let mut g = Generated::new();
    g.leaf();
    g.upstream();
    let delivery = g.rebase(2);
    let report = g.adapter.prepare_generated_repair(&delivery).unwrap().expect("queue repair must update configured generator branches");
    assert!(report.completed, "{report:?}");
    assert_eq!(git(&g.f.repo, &["show", &format!("{}:gen/manifest.json", report.head)]), "{\"count\": 2}");
    let mut check = delivery.clone();
    check.head_sha = Some(report.head);
    assert!(g.adapter.check_generation(&check).unwrap().unwrap().changed.is_empty());
}
