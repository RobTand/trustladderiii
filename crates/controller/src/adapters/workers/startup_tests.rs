use super::*;
use std::os::unix::fs::PermissionsExt;

#[test]
fn planner_protected_paths_follow_the_request_repository() {
    use crate::git_tests::{Repo, documents, support::Database};
    let f = Repo::new();
    let mut db = Database::start();
    let mut config =
        tl3_config::Config::parse(include_str!("../../../../config/example.toml")).unwrap();
    config.store = db.create("repo_protection");
    config.workers.runs = f.root.path().join("runs");
    config.planning.protected_paths.push("global/**".into());
    config.github.repos[0].protected_paths = vec!["CHANGELOG.md".into()];
    let mut other = config.github.repos[0].clone();
    other.name = "other-repo".into();
    other.protected_paths = vec!["other-only/**".into()];
    config.github.repos.push(other);
    let registry = crate::adapters::simulation(&f.root.path().join("fakes")).unwrap();
    let mut controller = crate::Controller::open(
        Store::open(&config.store).unwrap(),
        registry,
        crate::Bootstrap::from_config(&config).unwrap(),
    )
    .unwrap();
    controller
        .intake(crate::LabelledIssue {
            delivery: "repo-protection".into(),
            owner: "RobTand".into(),
            repo: "tl3-sandbox".into(),
            installation: 1,
            number: 42,
            title: "Protect only this repo".into(),
            actor: config.github.rob.id,
            label: "tl3".into(),
        })
        .unwrap();
    let mut store = Store::open(&config.store).unwrap();
    store.put_worker_materials("0", &json!({})).unwrap();
    let snapshot = documents(
        &mut store,
        &[(
            "briefs",
            json!({
                "kind":"brief", "memory_ids":[], "brief":"Keep repository policies separate",
                "goals":[{"description":"Use this repo's files","verification":"review"}],
            }),
        )],
    );
    let supervisor = Supervisor::new(&config, &f.root.path().join("records")).unwrap();
    for (name, own, foreign) in [
        ("TL3-SANDBOX", "CHANGELOG.md", "other-only/**"),
        ("OTHER-REPO", "other-only/**", "CHANGELOG.md"),
    ] {
        let mut delivery = f.delivery();
        delivery.repository.owner = "ROBTAND".into();
        delivery.repository.name = name.into();
        let request = supervisor
            .request(&delivery, "planner", &mut store, &snapshot)
            .unwrap();
        let paths = request["inputs"]["protected_paths"].as_array().unwrap();
        assert!(paths.contains(&json!(own)), "missing {own}: {paths:?}");
        assert!(
            !paths.contains(&json!(foreign)),
            "leaked {foreign}: {paths:?}"
        );
        assert!(paths.contains(&json!("global/**")));
        for required in tl3_config::Planning::default().protected_paths {
            assert!(paths.contains(&json!(required)));
        }
    }
    drop(controller);
    drop(supervisor);
    db.stop();
}

#[test]
fn maker_input_stays_bounded_after_five_review_rounds() {
    use crate::git_tests::{Repo, documents, support::Database};
    let f = Repo::new();
    let mut db = Database::start();
    let mut config =
        tl3_config::Config::parse(include_str!("../../../../config/example.toml")).unwrap();
    config.store = db.create("bounded_feedback");
    config.workers.runs = f.root.path().join("runs");
    let supervisor = Supervisor::new(&config, &f.root.path().join("records")).unwrap();
    let mut store = Store::open(&config.store).unwrap();
    store
        .put_worker_materials(
            "0",
            &json!({"maker": {
                "brief": "Return hello.",
                "goals": [{"id": "greeting", "description": "Return hello.", "verification": "test"}],
                "plan": {"files": ["README.md"], "checks": [{"name": "hello", "kind": "demonstrating",
                    "tests": ["test_greeting.py::test_hello"], "goal_ids": ["greeting"]}]},
            }}),
        )
        .unwrap();
    // Five review rounds at the controller's own caps: 20 findings and 16,000 characters
    // each. A check failure at the cap follows every round, as it does in a long fix loop.
    let shas: Vec<String> = (1..=5).map(|n| format!("{n:040x}")).collect();
    let mut stored = Vec::new();
    for (round, sha) in shas.iter().enumerate() {
        for index in 0..20 {
            stored.push(json!({
                "kind": "review_finding", "id": format!("review-{round}-{index}"), "reviewed_sha": sha,
                "report": format!("Round {round} finding {index}: {}", "x".repeat(770)),
                "goal_ids": ["greeting"],
            }));
        }
        stored.push(json!({
            "kind": "failed_check", "report": format!("Check failure {round}: {}", "y".repeat(15900)),
            "goal_ids": [],
            "failed_tests": (0..20).map(|n| json!({"test": format!("test_{round}_{n}"), "message": "m".repeat(990)})).collect::<Vec<_>>(),
            "outside_paths_total": 0,
        }));
    }
    let snapshot = documents(&mut store, &[("maker_findings", json!(stored))]);
    let mut delivery = f.delivery();
    delivery.round = 1;
    delivery.attempt = 1;
    delivery.head_sha = Some(shas[4].clone());
    let request = supervisor
        .request(&delivery, "maker", &mut store, &snapshot)
        .unwrap();
    let inputs = &request["inputs"];
    let sent = serde_json::to_string(inputs).unwrap().len();
    let limit = config.limits.context_tokens as usize;
    assert!(
        sent / 4 * 100 < limit * 30,
        "the maker input is {sent} characters, about {} of {limit} context tokens",
        sent / 4
    );
    // No finding is lost: every stored finding is still named, and the reviewed head's
    // findings keep their whole text, sent once.
    let feedback = inputs["feedback"].as_array().unwrap();
    assert_eq!(feedback.len(), stored.len());
    for (sent, original) in feedback.iter().zip(&stored) {
        assert_eq!(sent["kind"], original["kind"]);
        assert_eq!(sent["id"], original["id"]);
    }
    let current = crate::findings::current_review(
        &inputs["feedback"],
        delivery.round,
        inputs["reviewed_sha"].as_str(),
    );
    let ids: Vec<_> = current.findings.iter().map(|f| f["id"].clone()).collect();
    assert_eq!(ids.len(), 20);
    for finding in &current.findings {
        assert!(
            stored.contains(finding),
            "an open finding keeps its full text"
        );
    }
    let refs = inputs["current_review_findings"].as_array().unwrap();
    assert_eq!(
        refs.iter().map(|r| r["id"].clone()).collect::<Vec<_>>(),
        ids
    );
    assert!(
        refs.iter().all(|r| r.get("report").is_none()),
        "the text of a current finding is sent once, in feedback"
    );
    // The reply check still maps every open finding id to its full report.
    let reply = json!({"reproductions": ids.iter().map(|id| json!({"finding_id": id, "reason": "Review by hand."})).collect::<Vec<_>>()});
    let fixes = super::reproducing::check(&reply, current).unwrap();
    assert_eq!(fixes.evidence["findings"].as_array().unwrap().len(), 20);
    // The store, and so the reviewer, keeps the whole history.
    assert_eq!(
        store
            .worker_document("0", "maker_findings")
            .unwrap()
            .unwrap(),
        json!(stored)
    );
    drop(supervisor);
    db.stop();
}

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
                    (
                        "maker_findings",
                        json!([{"kind":"failed_check", "report":"Earlier round failed", "goal_ids":[]}]),
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

#[test]
fn restarted_maker_checks_against_its_own_worktree_base() {
    crate::git_tests::hostile(
        "adapters::workers::startup_tests::restarted_maker_checks_against_its_own_worktree_base",
        // #326: a restart (Q49) links node 1 to node 0. Node 0 pinned its checks to the
        // older main f.base; main has moved to f.head, where node 1's worktree starts.
        || maker_checks_against_the_worktree_base("restarted_base", "0", true),
    );
}

#[test]
fn maker_heals_its_own_stale_check_base() {
    crate::git_tests::hostile(
        "adapters::workers::startup_tests::maker_heals_its_own_stale_check_base",
        // #326: node 1 already stored checks pinned to f.base, before main moved to
        // f.head and the worktree's recorded base followed it.
        || maker_checks_against_the_worktree_base("stale_own_base", "1", false),
    );
}

fn maker_checks_against_the_worktree_base(database: &str, stored_on: &str, restarted: bool) {
    use crate::git_tests::{Repo, git, support::Database};
    let f = Repo::new();
    let mut db = Database::start();
    let settings = db.create(database);
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
    let mut value = serde_json::to_value(f.delivery()).unwrap();
    value["output"]["Effect"]["node"] = json!(1);
    value["checks"] = json!(null);
    let delivery: Delivery = serde_json::from_value(value).unwrap();
    let checkout = super::super::worktrees::checkout_path(&config.worktrees_root, &delivery);
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
    assert_eq!(branch.as_str(), "prismabuild-42-tl3-1");
    let main = CommitId::new(&f.head).unwrap();
    Worktrees::open(&checkout, &checkout)
        .unwrap()
        .create_branch_from(&branch, &main, &main)
        .unwrap();
    git(&checkout, &["checkout", "--quiet", branch.as_str()]);
    let mut store = Store::open(&settings).unwrap();
    store
        .put_worker_materials(
            stored_on,
            &json!({"check_run": {"repository": f.path, "base_sha": f.base, "plan": [
                    {"name":"value","kind":"regression","tests":["test_value.py::test_value"]}]}}),
        )
        .unwrap();
    let before = store.load().unwrap();
    let mut commit = tl3_store::Commit::new("restart-fixture", json!(null), &before);
    let rows = [
        (
            "briefs",
            json!({"brief":"Read the intended repo","goals":[{"description":"Read it","verification":"test"}]}),
        ),
        (
            "plans",
            json!({"kind":"leaf","files":["README.md"],"checks":[{"kind":"regression","node_ids":["test_value.py::test_value"],"goal_ids":[0]}]}),
        ),
        (
            "request_links",
            json!({"original": 0, "authorized_by": "fixture", "issue": {
                "number": 42, "title": "Restarted", "body": "Restarted request", "state": "open",
                "updated_at": "2026-11-01T23:27:00Z", "labels": [], "pull_request": null}}),
        ),
    ];
    commit.rows = rows
        .into_iter()
        .filter(|(table, _)| restarted || *table != "request_links")
        .map(|(table, value)| tl3_store::RowChange::Put {
            table: table.into(),
            id: "1".into(),
            value,
        })
        .collect();
    store.commit(&commit).unwrap();
    let snapshot = store.load().unwrap();
    let request = supervisor
        .request(&delivery, "maker", &mut store, &snapshot)
        .unwrap();
    assert_eq!(
        request["inputs"]["check_base_sha"], f.head,
        "checks compare against the base the worktree sits on"
    );
    assert_eq!(
        store.worker_materials("1").unwrap().unwrap()["check_run"]["base_sha"],
        f.head
    );
}
