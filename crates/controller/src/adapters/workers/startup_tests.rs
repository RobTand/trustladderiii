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
    // The maker's retro deadline derives from this allowance (#648).
    config.allowances.agent_worker.silence_secs = 90;
    config.allowances.agent_worker.no_progress_secs = 20;
    config.github.repos[0].test_demand = Some(tl3_config::TestDemand {
        cpus: 4,
        mem_gb: 12,
    });
    config.github.repos[0].protected_paths = vec!["CHANGELOG.md".into()];
    config.github.repos[0].generated_files = vec!["**/BUILD.bazel".into()];
    config.github.repos[0].generator = Some("bash tools/bazel/generate.sh".into());
    config.github.repos[0].generator_timeout_s = Some(300);
    let mut other = config.github.repos[0].clone();
    other.name = "other-repo".into();
    other.test_demand = None;
    other.generator_timeout_s = None;
    other.protected_paths = vec!["other-only/**".into()];
    other.generator = None;
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
            labels: Vec::new(),
        })
        .unwrap();
    let mut store = Store::open(&config.store).unwrap();
    store
        .put_worker_materials(
            "0",
            &json!({"base_checkout": {
                "repository": f.root.path(), "commit": f.base
            }}),
        )
        .unwrap();
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
        let cpus = if name == "TL3-SANDBOX" { 4 } else { 1 };
        assert!(
            request["session_file"]
                .as_str()
                .is_some_and(|path| path.ends_with("planner-rehearsals.json")),
            "planner recovery needs a controller-owned rehearsal checkpoint"
        );
        assert_eq!(request["test_cpus"], cpus);
        assert_eq!(request["silence_secs"], 90);
        assert_eq!(request["no_progress_secs"], 20);
        assert_eq!(request["inputs"]["repair_plan_consistency"], true);
        assert_eq!(request["inputs"]["base_sha"], f.base);
        assert_eq!(request["inputs"]["tests"]["cpus"], cpus);
        assert_eq!(
            request["inputs"]["tests"]["mem_gb"],
            if cpus == 4 { 12 } else { 2 }
        );
        assert_eq!(
            request["inputs"]["tests"]["tools_path"],
            json!(config.prismabuild.tools_dir)
        );
        let features = request["inputs"]["tool_capabilities"][0]["runner_features"]
            .as_array()
            .unwrap();
        assert!(features.contains(&json!(format!("native_threads_{cpus}"))));
        if cpus != 1 {
            assert!(!features.contains(&json!("native_threads_1")));
        }
        let forecast = &request["inputs"]["forecast"];
        assert_eq!(forecast["generated_files"], json!(["**/BUILD.bazel"]));
        assert_eq!(forecast["generator_available"], name == "TL3-SANDBOX");
        assert_eq!(
            forecast["generator_timeout_s"],
            if name == "TL3-SANDBOX" { 300 } else { 120 }
        );
        if name == "TL3-SANDBOX" {
            assert_eq!(forecast["generator"], "bash tools/bazel/generate.sh");
        } else {
            assert!(forecast.get("generator").is_none());
        }
        let mut reply = json!({"kind":"leaf", "evidence_contract":[{
            "requirement":"Regenerate indexed Bazel outputs", "tool":"run_generator",
            "executor":"prismabuild_generator", "runner_features":["git_index", "scoped_outputs"],
            "artifacts":["cas_receipt", "output_tree"], "methods":["configured_generation"], "protected_files":[],
        }]});
        super::evidence::preflight("planner", &request["inputs"], &mut reply).unwrap();
        assert_eq!(reply["evidence_errors"] == json!([]), name == "TL3-SANDBOX");
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
    config.github.repos[0].generated_files = vec!["**/BUILD.bazel".into()];
    config.github.repos[0].generator = Some("bash tools/bazel/generate.sh".into());
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
    // Five review rounds at the controller's own caps, and a check failure at its caps
    // after every round, as in a long fix loop.
    use crate::findings::{
        MAX_FAILED_TESTS, MAX_MESSAGE_CHARS, MAX_REPORT_CHARS, MAX_REVIEW_FINDINGS, clipped,
    };
    let finding_chars = MAX_REPORT_CHARS / MAX_REVIEW_FINDINGS;
    let shas: Vec<String> = (1..=5).map(|n| format!("{n:040x}")).collect();
    let mut stored = Vec::new();
    for (round, sha) in shas.iter().enumerate() {
        for index in 0..MAX_REVIEW_FINDINGS {
            let report = format!(
                "Round {round} finding {index}: {}",
                "x".repeat(finding_chars)
            );
            stored.push(json!({
                "kind": "review_finding", "id": format!("review-{round}-{index}"), "reviewed_sha": sha,
                "report": clipped(&report, finding_chars),
                "goal_ids": ["greeting"],
            }));
        }
        let report = format!("Check failure {round}: {}", "y".repeat(MAX_REPORT_CHARS));
        stored.push(json!({
            "kind": "failed_check", "report": clipped(&report, MAX_REPORT_CHARS),
            "goal_ids": ["greeting"],
            "failed_tests": (0..MAX_FAILED_TESTS).map(|n| json!({"test": format!("test_{round}_{n}"), "message": "m".repeat(MAX_MESSAGE_CHARS)})).collect::<Vec<_>>(),
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
    assert_eq!(inputs["forecast"]["fit_tokens"], config.limits.fit_tokens);
    assert_eq!(
        inputs["forecast"]["context_tokens"],
        config.limits.context_tokens
    );
    assert_eq!(
        inputs["forecast"]["overhead_tokens"],
        json!(config.planning.overhead_tokens)
    );
    assert_eq!(inputs["forecast"]["generator_available"], true);
    assert_eq!(
        inputs["forecast"]["generator"],
        "bash tools/bazel/generate.sh"
    );
    assert!(
        inputs["protected_paths"]
            .as_array()
            .unwrap()
            .contains(&json!(".github/**"))
    );
    for demand in [
        None,
        Some(tl3_config::TestDemand {
            cpus: 4,
            mem_gb: 12,
        }),
    ] {
        let mut config = config.clone();
        config.github.repos[0].test_demand = demand;
        let supervisor = Supervisor::new(&config, &f.root.path().join("demand-records")).unwrap();
        let request = supervisor
            .request(&delivery, "maker", &mut store, &snapshot)
            .unwrap();
        let expected = demand.unwrap_or(tl3_config::TestDemand { cpus: 1, mem_gb: 2 });
        assert_eq!(
            request["inputs"]["tests"]["cpus"], expected.cpus,
            "maker CPU reservation"
        );
        assert_eq!(
            request["inputs"]["tests"]["mem_gb"], expected.mem_gb,
            "maker memory reservation"
        );
    }
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
    let open = MAX_REVIEW_FINDINGS + 1;
    for (index, (sent, original)) in feedback.iter().zip(&stored).enumerate() {
        assert_eq!(sent["kind"], original["kind"]);
        assert_eq!(sent["id"], original["id"]);
        assert_eq!(sent["goal_ids"], original["goal_ids"]);
        if index >= stored.len() - open {
            assert_eq!(sent, original, "the latest review and check stay whole");
            continue;
        }
        // Each review and each check failure is one feedback event, and the review of
        // the next round supersedes both the findings and the check that followed them.
        let round = index / open;
        assert_eq!(sent["resolved_round"], 2 * round + 3, "{sent}");
        let title: String = original["report"]
            .as_str()
            .unwrap()
            .chars()
            .take(crate::findings::TITLE_CHARS)
            .collect();
        assert_eq!(sent["report"], title, "a collapsed item keeps its title");
        assert!(sent.get("failed_tests").is_none());
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
                        json!({"kind":"leaf","files":["README.md"],"checks":[{"kind":"demonstrating","node_ids":["test_value.py::test_value"],"goal_ids":[0],"forcing":{"symptom":"The value is late.","environment":{"VALUE_LATE":"1"},"patches":[]}}]}),
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
            // #393: the harness gets the plan's forcing through the stored check run.
            assert_eq!(
                store.worker_materials("0").unwrap().unwrap()["check_run"]["plan"][0]["forcing"]["environment"]
                    ["VALUE_LATE"],
                "1"
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
        .with_materials(crate::production::Materials::repository_fixture(
            config.worktrees_root.clone(),
            f.path.clone(),
            &config.github.repos,
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

/// A claimed postmortem's delivery, with its store and config.
fn claimed_postmortem(
    name: &str,
    configure: impl FnOnce(&mut tl3_config::Config),
) -> (
    crate::git_tests::Repo,
    crate::git_tests::support::Database,
    tl3_config::Config,
    Store,
    Delivery,
) {
    use crate::git_tests::{Repo, support::Database};
    let f = Repo::new();
    let mut db = Database::start();
    let mut config =
        tl3_config::Config::parse(include_str!("../../../../config/example.toml")).unwrap();
    config.store = db.create(name);
    config.workers.runs = f.root.path().join("runs");
    configure(&mut config);
    let registry = crate::adapters::simulation(&f.root.path().join("fakes")).unwrap();
    let mut controller = crate::Controller::open(
        Store::open(&config.store).unwrap(),
        registry,
        crate::Bootstrap::from_config(&config).unwrap(),
    )
    .unwrap();
    controller.run_postmortems(crate::PostmortemPace::from_config(&config));
    let mut store = Store::open(&config.store).unwrap();
    postgres::Config::new()
        .host(&config.store.host)
        .port(config.store.port)
        .user(&config.store.user)
        .dbname(&config.store.database)
        .password("tl3-disposable-test-password")
        .connect(postgres::NoTls)
        .unwrap()
        .execute(
            "UPDATE tl3_settings SET value = '1' WHERE key = 'postmortem_sample_share'",
            &[],
        )
        .unwrap();
    controller
        .intake(crate::LabelledIssue {
            delivery: "postmortem-case".into(),
            owner: "RobTand".into(),
            repo: "tl3-sandbox".into(),
            installation: 1,
            number: 42,
            title: "Fix the sandbox build".into(),
            actor: config.github.rob.id,
            label: "tl3".into(),
            labels: Vec::new(),
        })
        .unwrap();
    let mut delivery = None;
    for _ in 0..300 {
        assert!(
            controller.drive_once().unwrap(),
            "no postmortem was claimed"
        );
        if let Some(effect) = store
            .effects()
            .unwrap()
            .into_iter()
            .find(|effect| effect.key.starts_with("postmortem:"))
        {
            delivery = Some(serde_json::from_value::<Delivery>(effect.payload).unwrap());
            break;
        }
    }
    let delivery = delivery.unwrap();
    assert!(delivery.is_postmortem());
    (f, db, config, store, delivery)
}

/// #403: a claimed postmortem's request carries a case file its schema accepts,
/// with the postmortem ladder's models and the node's MLflow tags.
#[test]
fn a_postmortem_request_carries_a_valid_case_file() {
    let (f, _db, config, mut store, delivery) = claimed_postmortem("postmortem_case", |_| {});
    let snapshot = store.load().unwrap();
    let supervisor = Supervisor::new(&config, &f.root.path().join("records")).unwrap();
    let request = supervisor
        .request(&delivery, "postmortem", &mut store, &snapshot)
        .unwrap();
    assert_eq!(request["role"], "postmortem");
    assert_eq!(request["model"]["name"], config.models.postmortem[0].name);
    let inputs = &request["inputs"];
    assert_eq!(inputs["grade_model"], config.models.postmortem[1].name);
    assert_eq!(inputs["record"]["trigger"], "close");
    assert_eq!(
        inputs["trace_tags"],
        json!({"node": "0", "request": "0", "repo": "RobTand/tl3-sandbox", "issue": "42"})
    );
    assert_eq!(inputs["case"]["outcome"], "Merged");
    assert_eq!(inputs["case"]["request"]["title"], "Fix the sandbox build");
    assert_eq!(inputs["rubric"]["version"], 1);
    assert_eq!(inputs["release"], inputs["case"]["release"]);
    assert_eq!(
        inputs["deadline_secs"], config.limits.postmortem_deadline_secs,
        "the run's whole wall-clock allowance (#486)"
    );
    assert_eq!(config.limits.postmortem_deadline_secs, 1200);
    assert!(
        inputs["case"]["grades"]
            .as_array()
            .unwrap()
            .iter()
            .any(|grade| grade["dimension"] == "reliability")
    );
    assert!(inputs["case"]["phases"].as_array().unwrap().len() > 1);
}

/// #759 (Q53): a postmortem falls back like other roles. Its writer is the
/// first postmortem model not out, and its grade judges run on the first not
/// out from the second entry on.
#[test]
fn a_postmortem_request_falls_back_past_models_that_are_out() {
    let (f, _db, config, mut store, delivery) =
        claimed_postmortem("postmortem_fallback", |config| {
            config.models.postmortem = config.models.maker.clone();
        });
    let [writer, grader] = [0, 1].map(|index| config.models.postmortem[index].name.clone());
    assert_ne!(writer, grader);
    let supervisor = Supervisor::new(&config, &f.root.path().join("records")).unwrap();
    let mut models = |out: &[(&str, u64)]| {
        let snapshot = store.load().unwrap();
        let mut commit = tl3_store::Commit::new(
            format!("fixture-quotas:{}", snapshot.revision.get()),
            json!({"fixture": "quotas"}),
            &snapshot,
        );
        for row in snapshot
            .rows
            .iter()
            .filter(|row| row.table == "model_quotas")
        {
            commit.rows.push(tl3_store::RowChange::Delete {
                table: row.table.clone(),
                id: row.id.clone(),
            });
        }
        for (model, reset) in out {
            commit.rows.push(crate::put(
                "model_quotas",
                (*model).into(),
                json!({"reset_ms": reset}),
            ));
        }
        store.commit(&commit).unwrap();
        let snapshot = store.load().unwrap();
        let request = supervisor
            .request(&delivery, "postmortem", &mut store, &snapshot)
            .unwrap();
        (
            request["model"]["name"].as_str().unwrap().to_owned(),
            request["inputs"]["grade_model"]
                .as_str()
                .unwrap()
                .to_owned(),
        )
    };
    const LATER: u64 = 4_000_000_000_000;
    assert_eq!(models(&[]), (writer.clone(), grader.clone()));
    assert_eq!(
        models(&[(grader.as_str(), LATER)]),
        (writer.clone(), writer.clone()),
        "the grade model is out, so the writer's model grades"
    );
    assert_eq!(
        models(&[(writer.as_str(), LATER)]),
        (grader.clone(), grader.clone()),
        "the writer's model is out, so the grade model writes"
    );
    assert_eq!(
        models(&[(writer.as_str(), 1), (grader.as_str(), 1)]),
        (writer.clone(), grader.clone()),
        "expired records are no bar"
    );
}

#[test]
fn ingest_preflight_refuses_a_declared_protected_edit_without_launching_a_worker() {
    use crate::git_tests::{Repo, support::Database};
    let f = Repo::new();
    let mut db = Database::start();
    let mut config =
        tl3_config::Config::parse(include_str!("../../../../config/example.toml")).unwrap();
    config.store = db.create("protected_preflight");
    config.workers.runs = f.root.path().join("runs");
    config.workers.python = f.root.path().join("missing-runtime/python");
    config.github.repos[0].protected_paths = vec!["tools/ci/**".into()];
    let controller = crate::Controller::open(
        Store::open(&config.store).unwrap(),
        crate::adapters::simulation(&f.root.path().join("fakes")).unwrap(),
        crate::Bootstrap::from_config(&config).unwrap(),
    )
    .unwrap();
    let mut store = Store::open(&config.store).unwrap();
    store.put_worker_materials("0", &json!({
        "issue":{"number":2636,"title":"Retry gate budgets","body":"## Expected footprint\n- `tools/ci/bazel_gate.py`", "author":"owner","comments":[]},
        "repo":{"readme":"Gate repository", "file_list":["tools/ci/bazel_gate.py"]}
    })).unwrap();
    let records = f.root.path().join("records");
    let mut supervisor = Supervisor::new(&config, &records).unwrap();
    let mut delivery = f.delivery();
    delivery.epoch = store.load().unwrap().epoch.get();
    delivery.output = Out::Effect {
        node: tl3_kernel::NodeId(0),
        seq: 1,
        kind: EffectKind::StartRun(tl3_kernel::Role::Ingest),
        head: None,
    };
    let completion = supervisor
        .execute(&delivery)
        .expect("declared protected edits need no runtime or model");
    let encoded = serde_json::to_value(&completion).unwrap();
    assert_eq!(
        encoded["observations"][0]["WorkerReply"]["reply"]["protected_paths"],
        json!(["tools/ci/bazel_gate.py"])
    );
    assert!(
        !supervisor.root(&delivery.key).exists(),
        "preflight creates no worker run"
    );
    drop(supervisor);
    let mut restarted = Supervisor::new(&config, &records).unwrap();
    assert!(
        matches!(
            restarted.reconcile(&delivery).unwrap(),
            Reconciliation::Found(_)
        ),
        "preflight completion is durable"
    );
    assert_eq!(
        serde_json::to_value(restarted.execute(&delivery).unwrap()).unwrap(),
        encoded
    );
    drop(controller);
    db.stop();
}

#[test]
fn t604_ingest_and_audit_get_authoritative_per_repository_request_branch() {
    use crate::git_tests::{Repo, support::Database};
    let f = Repo::new();
    let mut db = Database::start();
    let mut config =
        tl3_config::Config::parse(include_str!("../../../../config/example.toml")).unwrap();
    config.store = db.create("branch_context");
    config.workers.runs = f.root.path().join("runs");
    config.github.repos[0].branch_template = "prismabuild-{issue}-tl3-{node}".into();
    let mut other = config.github.repos[0].clone();
    other.name = "other-repo".into();
    other.branch_template = "request-{node}-issue-{issue}".into();
    config.github.repos.push(other);
    let mut store = Store::open(&config.store).unwrap();
    store.put_worker_materials("200", &json!({"issue":{"number":2646,"title":"Branch", "body":"Publish", "author":"rob","comments":[]},
        "repo":{"readme":"Repo","file_list":[]}})).unwrap();
    let snapshot = store.load().unwrap();
    let mut commit = tl3_store::Commit::new("brief", json!(null), &snapshot);
    commit.rows.push(crate::put(
        "briefs",
        "200".into(),
        json!({"kind":"brief","brief":"Publish", "goals":[{"description":"Publish","verification":"review"}],"memory_ids":[]}),
    ));
    store.commit(&commit).unwrap();
    let snapshot = store.load().unwrap();
    let supervisor = Supervisor::new(&config, &f.root.path().join("records")).unwrap();
    for (name, branch) in [
        ("tl3-sandbox", "prismabuild-2646-tl3-200"),
        ("other-repo", "request-200-issue-2646"),
    ] {
        let mut delivery = f.delivery();
        delivery.output = crate::executor::Output::Effect {
            node: tl3_kernel::NodeId(200),
            seq: 1,
            kind: tl3_kernel::EffectKind::StartRun(tl3_kernel::Role::Ingest),
            head: None,
        };
        delivery.issue = 2646;
        delivery.proposed = Some(crate::adoption::ProposedPullRequest {
            number: 99,
            head: f.head.clone(),
            head_repository: format!("RobTand/{name}"),
        });
        delivery.repository.name = name.into();
        for role in ["ingest", "audit"] {
            let request = supervisor
                .request(&delivery, role, &mut store, &snapshot)
                .unwrap();
            assert_eq!(request["inputs"]["publication_branch"]["branch"], branch);
            assert_eq!(request["inputs"]["publication_branch"]["node"], 200);
        }
    }
}

#[test]
fn t653_input_schema_defect_reaches_the_incident_without_retrying() {
    use crate::executor::{Registry, Upstream};
    use crate::git_tests::{Repo, support::Database};
    struct BrokenInputs;
    impl Executor for BrokenInputs {
        fn execute(&mut self, _: &Delivery) -> Result<Completion> {
            protocol::validate_inputs(
                "classifier",
                &json!({"comment":{"author":"owner", "body":"private-input-value"},
                "state":{"phase":123, "closed":false}}),
            )?;
            unreachable!()
        }
        fn reconcile(&mut self, _: &Delivery) -> Result<Reconciliation> {
            Ok(Reconciliation::Absent)
        }
    }
    let f = Repo::new();
    let mut db = Database::start();
    let mut config =
        tl3_config::Config::parse(include_str!("../../../../config/example.toml")).unwrap();
    config.store = db.create("schema_incident");
    let mut registry: Registry = crate::adapters::simulation(&f.root.path().join("fakes")).unwrap();
    registry.register(Upstream::Workers, BrokenInputs);
    let mut controller = crate::Controller::open(
        Store::open(&config.store).unwrap(),
        registry,
        crate::Bootstrap::from_config(&config).unwrap(),
    )
    .unwrap();
    controller
        .intake(crate::LabelledIssue {
            delivery: "schema-incident".into(),
            owner: "RobTand".into(),
            repo: "tl3-sandbox".into(),
            installation: 1,
            number: 42,
            title: "Contract failure".into(),
            actor: config.github.rob.id,
            label: "tl3".into(),
            labels: vec![],
        })
        .unwrap();
    for _ in 0..15 {
        if !controller.drive_once().unwrap() {
            break;
        }
    }
    let mut store = Store::open(&config.store).unwrap();
    let snapshot = store.load().unwrap();
    let incidents: Vec<_> = snapshot
        .rows
        .iter()
        .filter(|row| row.table == "incidents")
        .collect();
    assert_eq!(
        incidents.len(),
        1,
        "a local schema defect must be attributed without an infrastructure retry"
    );
    let incident = &incidents[0].value;
    assert_eq!(incident["cause"], "tl3");
    assert_eq!(incident["signature"]["class"], "worker_input_schema");
    let summary = incident["summary"].as_str().unwrap();
    assert!(summary.contains("role=classifier"), "{summary}");
    assert!(summary.contains("pointer=/state/phase"), "{summary}");
    assert!(summary.contains("rule=type"), "{summary}");
    assert!(
        !serde_json::to_string(incident)
            .unwrap()
            .contains("private-input-value")
    );
    assert_eq!(
        store
            .effects()
            .unwrap()
            .iter()
            .filter(
                |effect| effect.kind == "Workers" && effect.state == tl3_store::EffectState::Failed
            )
            .count(),
        1
    );
}

#[test]
fn t653_rejected_inputs_send_only_the_safe_diagnostic_to_tracing() {
    use crate::git_tests::{Repo, support::Database};
    let f = Repo::new();
    let mut db = Database::start();
    let mut config =
        tl3_config::Config::parse(include_str!("../../../../config/example.toml")).unwrap();
    config.store = db.create("schema_trace");
    config.workers.runs = f.root.path().join("runs");
    let capture = f.root.path().join("trace-input.json");
    let python = f.root.path().join("python");
    fs::write(
        &python,
        format!("#!/bin/sh\ncat > '{}'\n", capture.display()),
    )
    .unwrap();
    fs::set_permissions(&python, fs::Permissions::from_mode(0o755)).unwrap();
    config.workers.python = python;
    config.workers.launcher.clear();
    let supervisor = Supervisor::new(&config, &f.root.path().join("records")).unwrap();
    let mut store = Store::open(&config.store).unwrap();
    store.put_worker_materials("0", &json!({"ingest":{"issue":{"number":42, "title":123,
        "body":"private-input-value", "author":"owner", "comments":[]}, "repo":{"readme":"", "file_list":[]}}})).unwrap();
    let snapshot = store.load().unwrap();
    let error = supervisor
        .request(&f.delivery(), "ingest", &mut store, &snapshot)
        .unwrap_err();
    assert!(error.to_string().contains("role=ingest"));
    assert!(
        capture.exists(),
        "a schema failure before spawn still needs an MLflow trace"
    );
    let bytes = fs::read(capture).unwrap();
    let request: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(request["role"], "ingest");
    assert_eq!(request["inputs"]["pointer"], "/issue/title");
    assert_eq!(request["inputs"]["rule"], "type");
    assert_eq!(request["trace"]["issue"], 42);
    assert!(
        !String::from_utf8(bytes)
            .unwrap()
            .contains("private-input-value")
    );
    assert!(request.get("key_file").is_none());
}

#[test]
fn t653_review_setup_failure_preserves_completed_work_in_both_notices() {
    use crate::executor::{Registry, Upstream};
    use crate::git_tests::{Repo, support::Database};
    struct ReviewFails(Fake);
    impl Executor for ReviewFails {
        fn execute(&mut self, delivery: &Delivery) -> Result<Completion> {
            if protocol::delivery_role(delivery) == Some("reviewer") {
                protocol::validate_inputs("reviewer", &json!({}))?;
            }
            self.0.execute(delivery)
        }
        fn reconcile(&mut self, delivery: &Delivery) -> Result<Reconciliation> {
            self.0.reconcile(delivery)
        }
    }
    struct NeverDraft;
    impl writer::Writer for NeverDraft {
        fn draft(&mut self, _: &str, _: &writer::WriteRequest) -> Result<writer::WriterReply> {
            panic!("recorded setup failures must not depend on model prose")
        }
    }
    let f = Repo::new();
    let mut db = Database::start();
    let mut config =
        tl3_config::Config::parse(include_str!("../../../../config/example.toml")).unwrap();
    config.store = db.create("review_setup_progress");
    fs::create_dir(f.root.path().join("worker")).unwrap();
    let mut registry: Registry = crate::adapters::simulation(&f.root.path().join("fakes")).unwrap();
    registry.register(
        Upstream::Workers,
        ReviewFails(Fake::new(&f.root.path().join("worker"))),
    );
    let mut controller = crate::Controller::open(
        Store::open(&config.store).unwrap(),
        registry,
        crate::Bootstrap::from_config(&config).unwrap(),
    )
    .unwrap();
    controller
        .intake(crate::LabelledIssue {
            delivery: "review-progress".into(),
            owner: "RobTand".into(),
            repo: "tl3-sandbox".into(),
            installation: 1,
            number: 42,
            title: "Preserve the test change".into(),
            actor: config.github.rob.id,
            label: "tl3".into(),
            labels: vec![],
        })
        .unwrap();
    let mut store = Store::open(&config.store).unwrap();
    let mut recorded = false;
    for _ in 0..100 {
        let snapshot = store.load().unwrap();
        if let Some(effect) = snapshot.pending_effects.iter().find(|effect| {
            let delivery: Delivery = serde_json::from_value(effect.payload.clone()).unwrap();
            protocol::delivery_role(&delivery) == Some("reviewer")
        }) && !recorded
        {
            let delivery: Delivery = serde_json::from_value(effect.payload.clone()).unwrap();
            let head = delivery.head_sha.unwrap();
            let mut commit =
                tl3_store::Commit::new("record-maker-handover", json!(null), &snapshot);
            commit.rows.push(crate::put("maker_replies","0".into(),json!({"kind":"done","head":head,
                "self_check":[{"goal_id":"goal-1","status":"met","evidence":"Committed the test change"}]})));
            store.commit(&commit).unwrap();
            recorded = true;
        }
        if snapshot.rows.iter().any(|row| row.table == "incidents") {
            break;
        }
        assert!(controller.drive_once().unwrap());
    }
    assert!(
        recorded,
        "fixture must reach review after the maker completed"
    );
    let snapshot = store.load().unwrap();
    let incident = snapshot
        .rows
        .iter()
        .find(|row| row.table == "incidents")
        .unwrap();
    let summary = incident.value["summary"].as_str().unwrap();
    assert!(summary.contains("Review setup failed."), "{summary}");
    assert!(
        summary.contains("The maker's change is complete and preserved."),
        "{summary}"
    );
    let mut notices = 0;
    for effect in store.effects().unwrap() {
        let delivery: Delivery = serde_json::from_value(effect.payload).unwrap();
        let notice = match delivery.output {
            crate::executor::Output::Notice {
                notice:
                    notice @ (tl3_kernel::Notice::Incident { .. } | tl3_kernel::Notice::Ask { .. }),
                ..
            } => notice,
            _ => continue,
        };
        let input = crate::adapters::github::writer_input(
            &delivery,
            Some(notice),
            "Preserve the test change",
            "Preserve the test change",
        )
        .unwrap();
        protocol::validate_inputs("writer", &serde_json::to_value(&input).unwrap()).unwrap();
        let text = crate::adapters::github::question_draft(
            &mut NeverDraft,
            &delivery,
            input,
            &tl3_github::message::Policy::default(),
        )
        .unwrap();
        assert!(text.contains("Review setup failed."), "{text}");
        assert!(
            text.contains("The maker's change is complete and preserved."),
            "{text}"
        );
        assert!(!text.contains("has not started"), "{text}");
        notices += 1;
    }
    assert_eq!(
        notices, 2,
        "operations incident and request question both preserve progress"
    );
}
