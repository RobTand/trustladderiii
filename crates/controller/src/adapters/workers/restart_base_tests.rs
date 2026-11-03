use super::*;
use crate::git_tests::{Repo, git, support::Database};
use tl3_kernel::NodeId;

struct Restart {
    repo: Repo,
    config: tl3_config::Config,
    delivery: Delivery,
    store: Store,
    parent: Value,
    selected: String,
}

impl Restart {
    fn new(db: &mut Database, name: &str, branch: &str, hops: u64, merged: bool) -> Self {
        let repo = Repo::new();
        git(&repo.path, &["reset", "--hard", &repo.base]);
        git(&repo.path, &["branch", "-m", branch]);
        let mut config =
            tl3_config::Config::parse(include_str!("../../../../config/example.toml")).unwrap();
        config.store = db.create(name);
        config.github.repos[0].base_branch = branch.into();
        config.workers.runs = repo.root.path().join("runs");
        config.worktrees_root = repo.root.path().join("worktrees");
        let mut delivery = repo.delivery();
        delivery.output = Out::Effect {
            node: NodeId(0),
            seq: 1,
            kind: EffectKind::StartRun(tl3_kernel::Role::Ingest),
            head: None,
        };
        delivery.checks = None;
        delivery.head_sha = None;
        let mut store = Store::open(&config.store).unwrap();
        let supervisor = Self::supervisor_for(&config, &repo);
        let snapshot = store.load().unwrap();
        let mut parent = supervisor
            .materials(&delivery, &mut store, &snapshot)
            .unwrap();
        parent["check_run"] = json!({"base_sha":repo.base,"repository":repo.path,"plan":[]});
        parent["maker"] = json!({"obsolete_implementation":"must not survive"});
        store.put_worker_materials("0", &parent).unwrap();
        let scratch = "refs/tl3/scratch/issue-42-node-0";
        git(&repo.path, &["update-ref", scratch, &repo.base]);
        if !merged {
            git(&repo.path, &["checkout", "-qb", "dependency"]);
        }
        fs::create_dir_all(repo.path.join("src/prismabuild")).unwrap();
        fs::create_dir_all(repo.path.join("docs")).unwrap();
        fs::write(
            repo.path.join("src/prismabuild/operator_view.py"),
            "def operator_ending_view(): return 'ended'\n",
        )
        .unwrap();
        fs::write(
            repo.path.join("docs/public_evidence.md"),
            "The operator view includes terminal evidence.\n",
        )
        .unwrap();
        git(&repo.path, &["add", "."]);
        git(&repo.path, &["commit", "-qm", "dependency B"]);
        let dependency = git(&repo.path, &["rev-parse", "HEAD"]);
        if !merged {
            git(&repo.path, &["checkout", "-q", branch]);
        }
        let snapshot = store.load().unwrap();
        let mut commit = tl3_store::Commit::new("restart-chain", json!(null), &snapshot);
        let context: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/decisions/pq2662.json"
        ))
        .unwrap();
        commit
            .rows
            .push(crate::put("request_decisions", "0".into(), context));
        commit.rows.push(crate::put(
            "saved_scratch",
            scratch.into(),
            json!({"node":0,"reference":scratch}),
        ));
        for node in 1..=hops {
            commit.rows.push(crate::put("request_links", node.to_string(), json!({"original":node-1,"issue":{
                "number":42,"title":"Use operator evidence","body":"Depends on dependency B merging first; do not implement the operator view here.",
                "state":"open","updated_at":"2026-11-03T15:01:00Z","pull_request":null
            }})));
            crate::decisions::inherit(&snapshot, &mut commit, NodeId(node - 1), NodeId(node))
                .unwrap();
        }
        store.commit(&commit).unwrap();
        delivery.output = Out::Effect {
            node: NodeId(hops),
            seq: 1,
            kind: EffectKind::StartRun(tl3_kernel::Role::Ingest),
            head: None,
        };
        let selected = if merged {
            dependency
        } else {
            repo.base.clone()
        };
        Self {
            repo,
            config,
            delivery,
            store,
            parent,
            selected,
        }
    }

    fn supervisor_for(config: &tl3_config::Config, repo: &Repo) -> Supervisor {
        Supervisor::new(config, &repo.root.path().join("records"))
            .unwrap()
            .with_materials(crate::production::Materials::repository_fixture(
                config.worktrees_root.clone(),
                repo.path.clone(),
                &config.github.repos,
            ))
    }

    fn materials(&mut self) -> Value {
        let snapshot = self.store.load().unwrap();
        Self::supervisor_for(&self.config, &self.repo)
            .materials(&self.delivery, &mut self.store, &snapshot)
            .unwrap()
    }

    fn request(&mut self, role: &str) -> Value {
        let snapshot = self.store.load().unwrap();
        let supervisor = Self::supervisor_for(&self.config, &self.repo);
        let request = supervisor
            .request(&self.delivery, role, &mut self.store, &snapshot)
            .unwrap();
        let materials = supervisor
            .materials(&self.delivery, &mut self.store, &snapshot)
            .unwrap();
        let checkout = Path::new(request["worktree"].as_str().unwrap());
        fs::create_dir_all(checkout.parent().unwrap()).unwrap();
        materialize(checkout, &materials).unwrap();
        request
    }
}

#[test]
fn restart_base_contains_merged_dependency_for_ingest_and_audit() {
    let mut db = Database::start();
    let mut f = Restart::new(&mut db, "restart_fresh", "trunk", 2, true);
    let materials = f.materials();
    assert_eq!(
        materials["base_checkout"]["commit"], f.selected,
        "restart must select dependency B on the current configured default branch"
    );
    assert!(materials.get("check_run").is_none());
    assert!(
        materials.get("maker").is_none(),
        "prior implementation overlays are not inputs"
    );
    let ingest = f.request("ingest");
    let snapshot = f.store.load().unwrap();
    let mut commit = tl3_store::Commit::new("accepted-ingest", json!(null), &snapshot);
    commit.rows.push(crate::put("briefs", "2".into(), json!({"kind":"brief","brief":"Use the merged operator view","goals":[{"description":"Use the operator view","verification":"review"}],"memory_ids":[]})));
    f.store.commit(&commit).unwrap();
    let audit = f.request("audit");
    for request in [ingest, audit] {
        assert_eq!(request["inputs"]["issue"]["title"], "Use operator evidence");
        assert!(
            request["inputs"]["issue"]["body"]
                .as_str()
                .unwrap()
                .contains("Depends on dependency B")
        );
        let files = request["inputs"]["repo"]["file_list"].as_array().unwrap();
        assert!(files.contains(&json!("src/prismabuild/operator_view.py")));
        assert!(files.contains(&json!("docs/public_evidence.md")));
        let checkout = Path::new(request["worktree"].as_str().unwrap());
        assert!(
            fs::read_to_string(checkout.join("src/prismabuild/operator_view.py"))
                .unwrap()
                .contains("def operator_ending_view")
        );
        assert!(
            fs::read_to_string(checkout.join("docs/public_evidence.md"))
                .unwrap()
                .contains("terminal evidence")
        );
        assert_eq!(git(checkout, &["rev-parse", "HEAD"]), f.selected);
    }
    assert_eq!(f.store.worker_materials("0").unwrap().unwrap(), f.parent);
}

#[test]
fn restart_base_is_persisted_before_work_and_reused_after_reattach() {
    let mut db = Database::start();
    let mut f = Restart::new(&mut db, "restart_replay", "main", 1, true);
    let first = f.materials();
    let saved = f.store.worker_materials("1").unwrap();
    assert_eq!(
        saved,
        Some(first.clone()),
        "initialization must persist one complete material set before work starts"
    );
    fs::write(f.repo.path.join("README.md"), "new upstream C\n").unwrap();
    git(
        &f.repo.path,
        &["commit", "-qam", "advance default branch to C"],
    );
    // Every call creates a new supervisor, as after a crash before request.json is written.
    assert_eq!(
        f.materials(),
        first,
        "reattach must not re-resolve the default branch"
    );
    // Reattach still works when upstream reads cannot succeed.
    fs::rename(&f.repo.path, f.repo.path.with_extension("unavailable")).unwrap();
    assert_eq!(f.materials(), first);
    let checkout = Restart::supervisor_for(&f.config, &f.repo)
        .root(&f.delivery.key)
        .join("worktree");
    fs::create_dir_all(&checkout).unwrap();
    fs::write(
        checkout.join("partial-initialization"),
        "old checkout fragment",
    )
    .unwrap();
    let request = f.request("ingest");
    assert!(!checkout.join("partial-initialization").exists());
    assert_eq!(
        git(
            Path::new(request["worktree"].as_str().unwrap()),
            &["rev-parse", "HEAD"]
        ),
        f.selected
    );
}

#[test]
fn restart_base_preserves_approved_decisions_pins_and_scratch_as_reference() {
    let mut db = Database::start();
    let mut f = Restart::new(&mut db, "restart_context", "main", 1, true);
    let request = f.request("ingest");
    let decision = &request["inputs"]["decision_context"]["decisions"][0];
    assert_eq!(decision["id"], "pq2662-q10");
    assert!(
        request["inputs"]["issue"]["comments"]
            .to_string()
            .contains("refs/tl3/scratch/issue-42-node-0"),
        "saved scratch is offered as prior work"
    );
    assert_eq!(
        decision["restrictions"],
        json!([
            "Change CURRENT_SOURCE_BLOBS only.",
            "Preserve historical manifest material."
        ])
    );
    assert_eq!(
        git(
            &f.repo.path,
            &["rev-parse", "refs/tl3/scratch/issue-42-node-0"]
        ),
        f.repo.base
    );
    assert_ne!(
        git(
            Path::new(request["worktree"].as_str().unwrap()),
            &["rev-parse", "HEAD"]
        ),
        f.repo.base,
        "saved scratch is reference, never the new base"
    );
    assert!(
        f.store
            .load()
            .unwrap()
            .rows
            .iter()
            .any(|row| row.table == "saved_scratch")
    );
}

#[test]
fn restart_base_provenance_identifies_selected_commit_and_link_origin() {
    let mut db = Database::start();
    let mut f = Restart::new(&mut db, "restart_provenance", "main", 2, true);
    let request = f.request("ingest");
    assert_eq!(
        request["trace"]["base_sha"], f.selected,
        "ingest trace must expose selected base distinct from check base"
    );
    assert_eq!(request["trace"]["request_origin"], "1");
}

#[test]
fn restart_base_does_not_assume_an_unmerged_dependency_is_present() {
    let mut db = Database::start();
    let mut f = Restart::new(&mut db, "restart_unmerged", "main", 1, false);
    let materials = f.materials();
    assert_eq!(materials["base_checkout"]["commit"], f.repo.base);
    let checkout = f.repo.root.path().join("unmerged-explore");
    materialize(&checkout, &materials).unwrap();
    assert!(!checkout.join("src/prismabuild/operator_view.py").exists());
    assert!(
        materials["issue"]["body"]
            .as_str()
            .unwrap()
            .contains("Depends on dependency B")
    );
}
