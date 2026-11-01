mod support;
#[allow(dead_code)]
mod worktree_fixture;

use serde_json::json;
use std::{
    fs,
    sync::{Arc, Mutex},
};
use tl3_controller::{
    Bootstrap, Controller, LabelledIssue, adapters,
    executor::{Completion, Delivery, Executor, Observation, Output, Reconciliation, Upstream},
};
use tl3_kernel::{EffectKind, Notice, Outcome, Phase, Role};
use tl3_store::Store;
use worktree_fixture::{Fixture, commit, git, identity};

#[test]
fn repository_protected_paths_trim_only_that_repository() {
    for (name, changelog_protected) in [("tl3-sandbox", true), ("other-repo", false)] {
        let root = tempfile::tempdir().unwrap();
        let f = Fixture::create(root.path());
        let mut repositories = f.repositories.clone();
        repositories[0].protected_paths = vec!["CHANGELOG.md".into()];
        let mut other = repositories[0].clone();
        other.name = "other-repo".into();
        other.protected_paths = vec![];
        repositories.push(other);
        let mut adapter = adapters::worktrees::Real::new(f.root.join("worktrees"), 1)
            .unwrap()
            .with_repositories(&repositories)
            .unwrap();
        let mut repo = identity();
        repo.name = name.to_ascii_uppercase();
        adapter
            .serve_local(repo.clone(), &f.repo, &f.remote)
            .unwrap();
        let mut delivery = f.delivery(EffectKind::PrepareWorktree, 0);
        delivery.repository = repo;
        adapter.execute(&delivery).unwrap();
        let checkout = adapter.checkout(&delivery);
        for file in ["CHANGELOG.md", "global.txt", "pyproject.toml"] {
            fs::write(checkout.join(file), "requested\n").unwrap();
        }
        commit(&checkout, "shared.txt", "requested\n");
        delivery.paths = ["shared.txt", "CHANGELOG.md", "global.txt", "pyproject.toml"]
            .map(String::from)
            .to_vec();
        let trim = adapter
            .trim_scope(&delivery, &["global.txt".into()])
            .unwrap();
        assert_eq!(
            trim.removed_paths.contains(&"CHANGELOG.md".into()),
            changelog_protected,
            "{name}"
        );
        assert!(trim.removed_paths.contains(&"global.txt".into()));
        assert!(
            trim.removed_paths.contains(&"pyproject.toml".into()),
            "defaults remain mandatory"
        );
        assert!(checkout.join("shared.txt").exists());
        assert_eq!(checkout.join("CHANGELOG.md").exists(), !changelog_protected);
    }
}

struct Capture {
    inner: Box<dyn Executor>,
    making: Arc<Mutex<Option<Delivery>>>,
    summary: Arc<Mutex<Option<Delivery>>>,
}
impl Executor for Capture {
    fn execute(&mut self, d: &Delivery) -> tl3_controller::Result<Completion> {
        if matches!(
            d.output,
            Output::Effect {
                kind: EffectKind::StartRun(Role::Make),
                ..
            }
        ) {
            *self.making.lock().unwrap() = Some(d.clone());
            return Ok(Completion::confirmed(vec![]));
        }
        if matches!(
            d.output,
            Output::Notice {
                notice: Notice::Summary(_),
                ..
            }
        ) {
            *self.summary.lock().unwrap() = Some(d.clone());
        }
        let mut result = self.inner.execute(d)?;
        for o in &mut result.observations {
            if let Observation::Plan { paths, .. } = o {
                *paths = vec!["shared.txt".into(), "pyproject.toml".into()];
            }
        }
        Ok(result)
    }
    fn reconcile(&mut self, d: &Delivery) -> tl3_controller::Result<Reconciliation> {
        if self
            .making
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|m| m.key == d.key)
        {
            return Ok(Reconciliation::Pending);
        }
        self.inner.reconcile(d)
    }
}

#[test]
fn scope_leaf_trims_before_checks_and_survives_restart_into_closing_summary() {
    let mut database = support::Database::start();
    let settings = database.create("scope");
    let root = tempfile::tempdir().unwrap();
    let f = Fixture::create(root.path());
    commit(&f.source, "pyproject.toml", "base config\n");
    git(&f.source, &["push", f.remote.to_str().unwrap(), "main"]);
    let making = Arc::new(Mutex::new(None));
    let summary = Arc::new(Mutex::new(None));
    let registry = || {
        let mut registry = adapters::simulation(&root.path().join("fakes")).unwrap();
        registry.register(Upstream::Worktrees, f.adapter());
        registry.register(
            Upstream::Workers,
            Capture {
                inner: Box::new(adapters::workers::Fake::new(&root.path().join("fakes"))),
                making: making.clone(),
                summary: summary.clone(),
            },
        );
        registry.register(
            Upstream::GitHub,
            Capture {
                inner: Box::new(adapters::github::Fake::new(&root.path().join("fakes"))),
                making: making.clone(),
                summary: summary.clone(),
            },
        );
        registry
    };
    let bootstrap = || Bootstrap {
        repos: vec![identity()],
        rob: 1,
        slots: 1,
        pool: 1,
        operations: None,
    };
    let mut c = Controller::open(Store::open(&settings).unwrap(), registry(), bootstrap()).unwrap();
    c.intake(LabelledIssue {
        delivery: "scope".into(),
        owner: identity().owner,
        repo: identity().name,
        installation: 1,
        number: 42,
        title: "Fix the value".into(),
        actor: 1,
        label: "tl3".into(),
    })
    .unwrap();
    while c.drive_once().unwrap() {}
    let d = making.lock().unwrap().clone().unwrap();
    let checkout = f.adapter().checkout(&d);
    let original = git(&checkout, &["rev-parse", "HEAD"]);
    fs::write(checkout.join("shared.txt"), "requested\n").unwrap();
    for file in ["extra.txt", "__init__.py", "example.com"] {
        fs::write(checkout.join(file), "unrelated\n").unwrap();
    }
    let proposed = commit(&checkout, "pyproject.toml", "unrelated\n");
    assert_eq!(
        git(
            &checkout,
            &["rev-list", "--count", &format!("{original}..HEAD")]
        ),
        "1",
        "the scope fixture needs one proposed commit, not one per file"
    );
    let trim = c
        .trim_scope(
            &f.adapter(),
            &d,
            &tl3_config::Planning::default().protected_paths,
        )
        .unwrap();
    assert_eq!(
        trim.removed_paths,
        vec![
            std::path::PathBuf::from("__init__.py"),
            "example.com".into(),
            std::path::PathBuf::from("extra.txt"),
            "pyproject.toml".into()
        ]
    );
    assert!(!checkout.join("extra.txt").exists());
    assert_eq!(
        fs::read(checkout.join("pyproject.toml")).unwrap(),
        b"base config\n"
    );
    assert_eq!(
        fs::read(checkout.join("shared.txt")).unwrap(),
        b"requested\n"
    );
    assert_eq!(git(&checkout, &["rev-parse", "HEAD^"]), proposed);
    let snapshot = c.snapshot().unwrap();
    let row = snapshot
        .rows
        .iter()
        .find(|r| r.table == "scope_trims")
        .expect("durable summary paths");
    assert_eq!(
        row.value["removed_paths"],
        json!(["__init__.py", "example.com", "extra.txt", "pyproject.toml"])
    );
    drop(c);
    let mut c = Controller::open(Store::open(&settings).unwrap(), registry(), bootstrap()).unwrap();
    assert_eq!(
        c.trim_scope(
            &f.adapter(),
            &d,
            &tl3_config::Planning::default().protected_paths
        )
        .unwrap(),
        trim
    );
    let mut store = Store::open(&settings).unwrap();
    let mut saved = tl3_store::Commit::new("saved-scratch", json!({}), &store.load().unwrap());
    saved.rows.push(tl3_store::RowChange::Put {
        table: "saved_scratch".into(),
        id: "scope-scratch".into(),
        value: json!({"node":d.node(), "reference":"refs/tl3/scratch/issue-42-node-0"}),
    });
    store.commit(&saved).unwrap();
    c.observe(
        "scope-head",
        Observation::Made {
            node: d.node(),
            generation: d.generation,
            sha: trim.head.as_str().into(),
        },
    )
    .unwrap();
    while c.drive_once().unwrap() {}
    assert_eq!(
        c.world().unwrap().nodes[0].phase,
        Phase::Closed(Outcome::Merged)
    );
    let summary = summary.lock().unwrap().clone().expect("closing summary");
    assert_eq!(
        serde_json::to_value(&summary).unwrap()["scope_removals"],
        "Changes not part of this request were removed: `__init__.py`, `example.com`, `extra.txt`, `pyproject.toml`."
    );
    assert_eq!(
        summary.note.as_deref(),
        Some("Saved unfinished work, kept for 30 days:\nrefs/tl3/scratch/issue-42-node-0")
    );
    drop(c);
    database.stop();
}
