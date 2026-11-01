use super::*;
use std::os::unix::fs::PermissionsExt;

#[test]
fn worker_credentials_survive_controller_credential_removal_and_expose_only_model_key() {
    let root = tempfile::tempdir().unwrap();
    let inherited = root.path().join("systemd-credentials");
    fs::create_dir(&inherited).unwrap();
    for name in ["model-key", "github-app.json", "postgres-password"] {
        fs::write(inherited.join(name), b"disposable fixture").unwrap();
        fs::set_permissions(inherited.join(name), fs::Permissions::from_mode(0o600)).unwrap();
    }
    let attempt = root.path().join("attempt");
    fs::create_dir(&attempt).unwrap();
    let mut request = json!({"key_file": inherited.join("model-key")});
    let mut command = Command::new("python3");
    command.args(["-c", "import json,os,sys; from pathlib import Path; r=json.load(sys.stdin); p=Path(os.environ['CREDENTIALS_DIRECTORY']); assert sorted(x.name for x in p.iterdir())==['model-key']; assert Path(r['key_file']).parent==p; assert Path(r['key_file']).read_bytes()==b'disposable fixture'"]);
    isolate_credentials(&attempt, &mut request, &mut command).unwrap();
    fs::remove_dir_all(inherited).unwrap();
    let path = Path::new(request["key_file"].as_str().unwrap());
    assert_eq!(path.metadata().unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(
        path.parent()
            .unwrap()
            .metadata()
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    let mut child = command.stdin(Stdio::piped()).spawn().unwrap();
    serde_json::to_writer(child.stdin.take().unwrap(), &request).unwrap();
    assert!(child.wait().unwrap().success());
}

#[test]
fn hostile_maker_head() {
    crate::git_tests::hostile(
        "adapters::workers::startup_tests::hostile_maker_head",
        || {
            use crate::git_tests::{Repo, documents, git, support::Database};
            let f = Repo::new();
            let mut db = Database::start();
            let settings = db.create("maker_head");
            let mut config =
                tl3_config::Config::parse(include_str!("../../../../config/example.toml")).unwrap();
            config.store = settings.clone();
            config.github.repos[0].branch_template = "prismabuild-{issue}-tl3-{node}".into();
            config.workers.runs = f.root.path().join("runs");
            config.worktrees_root = f.root.path().join("worktrees");
            let supervisor = Supervisor::new(&config, &f.root.path().join("records"))
                .unwrap()
                .with_materials(crate::production::Materials::fixture(
                    config.worktrees_root.clone(),
                ));
            let mut delivery = f.delivery();
            delivery.checks = None;
            let checkout =
                super::super::worktrees::checkout_path(&config.worktrees_root, &delivery);
            fs::create_dir_all(checkout.parent().unwrap()).unwrap();
            git(
                f.root.path(),
                &[
                    "clone",
                    "--quiet",
                    f.path.to_str().unwrap(),
                    checkout.to_str().unwrap(),
                ],
            );
            let branch = super::super::worktrees::Real::new(&config.worktrees_root, 0)
                .unwrap()
                .with_repositories(&config.github.repos)
                .unwrap()
                .branch_for(&delivery)
                .unwrap();
            assert_eq!(branch.as_str(), "prismabuild-42-tl3-0");
            Worktrees::open(&checkout, &checkout)
                .unwrap()
                .create_branch_from(
                    &branch,
                    &CommitId::new(&f.base).unwrap(),
                    &CommitId::new(&f.head).unwrap(),
                )
                .unwrap();
            git(&checkout, &["checkout", "--quiet", branch.as_str()]);
            let mut store = Store::open(&settings).unwrap();
            store.put_worker_materials("0", &json!({})).unwrap();
            let snapshot = documents(
                &mut store,
                &[
                    (
                        "briefs",
                        json!({"brief":"Read the intended repo","goals":[{"description":"Read it","verification":"test"}]}),
                    ),
                    (
                        "plans",
                        json!({"kind":"leaf","files":["README.md"],"checks":[{"kind":"regression","node_ids":["test_value.py::test_value"],"goal_ids":[0]}]}),
                    ),
                ],
            );
            let request = supervisor
                .request(&delivery, "maker", &mut store, &snapshot)
                .unwrap();
            assert_eq!(request["worktree"], json!(checkout));
            assert_eq!(request["inputs"]["check_base_sha"], f.base);
            assert_eq!(
                store.worker_materials("0").unwrap().unwrap()["check_run"]["base_sha"],
                f.base
            );
            // A fix round may start after commits and a rebase changed the check base.
            fs::write(checkout.join("README.md"), "earlier maker round\n").unwrap();
            git(&checkout, &["commit", "-qam", "Earlier maker round"]);
            let snapshot = documents(
                &mut store,
                &[(
                    "maker_findings",
                    json!([{"kind":"failed_check", "report":"Earlier round failed", "goal_ids":[]}]),
                )],
            );
            delivery.round = 1;
            delivery.checks = f.delivery().checks;
            delivery.checks.as_mut().unwrap().base_sha = CommitId::new(&f.head).unwrap();
            let request = supervisor
                .request(&delivery, "maker", &mut store, &snapshot)
                .unwrap();
            assert_eq!(request["inputs"]["check_base_sha"], f.head);
            assert_ne!(
                request["inputs"]["check_base_sha"],
                git(&checkout, &["rev-parse", "HEAD"])
            );
            delivery.checks = None;
            let request = supervisor
                .request(&delivery, "maker", &mut store, &snapshot)
                .unwrap();
            assert_eq!(request["inputs"]["check_base_sha"], f.head);
        },
    );
}
