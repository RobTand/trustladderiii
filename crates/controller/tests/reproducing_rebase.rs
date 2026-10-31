#[allow(dead_code)]
mod worktree_fixture;

use std::fs;
use tl3_controller::adapters::worktrees::Real;
use tl3_controller::{
    adapters::prismabuild::{Check, CheckRun, Kind},
    executor::{Executor, Observation},
};
use tl3_kernel::EffectKind;
use worktree_fixture::{Fixture, commit, git};

#[test]
fn fix_round_rebases_reviewed_tree_with_upstream_configuration() {
    let root = tempfile::tempdir().unwrap();
    let f = Fixture::create(root.path());
    let config =
        "[tool.tl3.harness]\ncommand = ['python', '-m', 'pytest']\ntest-files = ['test_*.py']\n";
    commit(&f.source, "pyproject.toml", config);
    git(&f.source, &["push", f.remote.to_str().unwrap(), "main"]);
    let prepare = f.delivery(EffectKind::PrepareWorktree, 1);
    let mut adapter = f.adapter();
    adapter.execute(&prepare).unwrap();
    let checkout = adapter.checkout(&prepare);
    let base = git(&checkout, &["rev-parse", "HEAD"]);
    let reviewed = commit(&checkout, "product.py", "def value(): return 1\n");
    commit(
        &checkout,
        "test_value.py",
        "def test_value():\n    from product import value\n    assert value() == 2\n",
    );
    let fixed = commit(&checkout, "product.py", "def value(): return 2\n");
    let updated_config = format!("{config}\n# upstream settings\n");
    commit(&f.source, "pyproject.toml", &updated_config);
    commit(&f.source, "conftest.py", "# upstream fixtures\n");
    git(&f.source, &["push", f.remote.to_str().unwrap(), "main"]);
    let mut rebase = f.delivery(EffectKind::StartRebase, 2);
    rebase.head_sha = Some(fixed);
    let mut run = serde_json::json!({"repository": f.repo, "base_sha": base,
        "reviewed_sha": reviewed, "plan": [{"name": "reproduces", "kind": "reproducing",
        "tests": ["test_value.py::test_value"]}]});
    rebase.checks = Some(serde_json::from_value::<CheckRun>(run.take()).unwrap());
    let result = adapter.execute(&rebase).unwrap();
    let (head, run) = result
        .observations
        .iter()
        .find_map(|o| match o {
            Observation::CheckPlan { sha, run, .. } => Some((sha, run)),
            _ => None,
        })
        .expect("a clean rebase retains its check plan");
    let rebased_reviewed = run.reviewed_sha.as_ref().expect("keep reproducing checks");
    assert_ne!(
        rebased_reviewed.as_str(),
        &reviewed,
        "the before tree must move with main"
    );
    assert_eq!(
        run.plan,
        vec![Check {
            name: "reproduces".into(),
            kind: Kind::Reproducing,
            tests: vec!["test_value.py::test_value".into()]
        }]
    );
    assert_eq!(
        git(
            &f.repo,
            &["show", &format!("{rebased_reviewed}:pyproject.toml")]
        ),
        updated_config.trim()
    );
    assert_eq!(
        git(
            &f.repo,
            &["show", &format!("{rebased_reviewed}:conftest.py")]
        ),
        "# upstream fixtures"
    );
    assert_eq!(
        git(
            &f.repo,
            &["show", &format!("{rebased_reviewed}:product.py")]
        ),
        "def value(): return 1"
    );
    assert_eq!(
        git(&f.repo, &["show", &format!("{head}:product.py")]),
        "def value(): return 2"
    );
    let cloned = root.path().join("clone");
    git(
        root.path(),
        &[
            "clone",
            "--no-local",
            f.repo.to_str().unwrap(),
            cloned.to_str().unwrap(),
        ],
    );
    assert_eq!(
        git(
            &cloned,
            &["show", &format!("{rebased_reviewed}:product.py")]
        ),
        "def value(): return 1"
    );
    let replay = adapter.execute(&rebase).unwrap();
    assert_eq!(
        serde_json::to_value(result).unwrap(),
        serde_json::to_value(replay).unwrap()
    );
    assert!(
        fs::read_to_string(checkout.join("pyproject.toml"))
            .unwrap()
            .contains("upstream settings")
    );
}

#[test]
fn unreplayable_reviewed_tree_drops_reproducers_with_a_reason() {
    let root = tempfile::tempdir().unwrap();
    let f = Fixture::create(root.path());
    commit(&f.source, "product.py", "def value(): return 0\n");
    git(&f.source, &["push", f.remote.to_str().unwrap(), "main"]);
    let prepare = f.delivery(EffectKind::PrepareWorktree, 1);
    let mut adapter = f.adapter();
    adapter.execute(&prepare).unwrap();
    let checkout = adapter.checkout(&prepare);
    let base = git(&checkout, &["rev-parse", "HEAD"]);
    let reviewed = commit(&checkout, "product.py", "def value(): return 1\n");
    commit(&checkout, "product.py", "def value(): return 2\n");
    commit(
        &checkout,
        "test_value.py",
        "def test_value(): assert True\n",
    );
    // An outside rewritten head can rebase cleanly even when the old review cannot.
    git(&checkout, &["reset", "--soft", &base]);
    git(&checkout, &["commit", "-m", "squashed fix"]);
    commit(&f.source, "product.py", "def value(): return 2\n");
    git(&f.source, &["push", f.remote.to_str().unwrap(), "main"]);
    let mut rebase = f.delivery(EffectKind::StartRebase, 2);
    rebase.head_sha = Some(git(&checkout, &["rev-parse", "HEAD"]));
    rebase.checks = Some(serde_json::from_value(serde_json::json!({
        "repository": f.repo, "base_sha": base, "reviewed_sha": reviewed,
        "plan": [{"name":"ordinary", "kind":"regression", "tests":["test_value.py::test_value"]},
                 {"name":"reproducer", "kind":"reproducing", "tests":["test_value.py::test_value"]}]
    })).unwrap());
    let result = adapter.execute(&rebase).unwrap();
    let run = result
        .observations
        .iter()
        .find_map(|o| match o {
            Observation::CheckPlan { run, .. } => Some(run),
            _ => None,
        })
        .unwrap();
    assert!(run.reviewed_sha.is_none());
    assert_eq!(run.plan.len(), 1);
    assert_eq!(run.plan[0].kind, Kind::Regression);
    assert!(
        run.reproducing_unavailable
            .as_ref()
            .unwrap()
            .contains("could not be replayed")
    );
    assert_eq!(
        serde_json::to_value(&result).unwrap(),
        serde_json::to_value(adapter.execute(&rebase).unwrap()).unwrap()
    );
    adapter
        .execute(&f.delivery(EffectKind::RemoveWorktree, 3))
        .unwrap();
    assert!(git(&f.repo, &["for-each-ref", "refs/heads/tl3-reviewed/"]).is_empty());
}

#[test]
fn failed_review_pin_reaches_the_next_review_reason() {
    let root = tempfile::tempdir().unwrap();
    let f = Fixture::create(root.path());
    let mut adapter = f.adapter();
    let prepare = f.delivery(EffectKind::PrepareWorktree, 1);
    adapter.execute(&prepare).unwrap();
    let checkout = adapter.checkout(&prepare);
    let base = git(&checkout, &["rev-parse", "HEAD"]);
    let reviewed = commit(&checkout, "product.py", "def value(): return 1\n");
    let fixed = commit(&checkout, "product.py", "def value(): return 2\n");
    // A ref at the namespace's parent prevents pinning any reviewed commit.
    git(&f.repo, &["update-ref", "refs/heads/tl3-reviewed", &base]);
    commit(&f.source, "upstream.txt", "upstream\n");
    git(&f.source, &["push", f.remote.to_str().unwrap(), "main"]);
    let mut rebase = f.delivery(EffectKind::StartRebase, 2);
    rebase.head_sha = Some(fixed);
    rebase.checks = Some(serde_json::from_value(serde_json::json!({
        "repository": f.repo, "base_sha": base, "reviewed_sha": reviewed,
        "plan": [{"name":"ordinary", "kind":"regression", "tests":["test_value.py::test_value"]},
                 {"name":"reproducer", "kind":"reproducing", "tests":["test_value.py::test_value"]}]
    })).unwrap());
    let result = adapter.execute(&rebase).unwrap();
    let run = result
        .observations
        .iter()
        .find_map(|o| match o {
            Observation::CheckPlan { run, .. } => Some(run),
            _ => None,
        })
        .unwrap();
    assert!(run.reviewed_sha.is_none());
    assert_eq!(run.plan.len(), 1);
    assert_eq!(run.plan[0].kind, Kind::Regression);
    assert!(
        run.reproducing_unavailable
            .as_ref()
            .unwrap()
            .contains("pin failed")
    );
    assert_eq!(
        serde_json::to_value(&result).unwrap(),
        serde_json::to_value(adapter.execute(&rebase).unwrap()).unwrap()
    );
}

#[test]
fn publication_conflict_reports_a_fact_without_pushing() {
    let root = tempfile::tempdir().unwrap();
    let f = Fixture::create(root.path());
    let prepare = f.delivery(EffectKind::PrepareWorktree, 1);
    let mut adapter = f.adapter();
    adapter.execute(&prepare).unwrap();
    let checkout = adapter.checkout(&prepare);
    let head = commit(&checkout, "shared.txt", "leaf side\n");
    let target = f.advance_main(true);
    let mut push = f.delivery(EffectKind::Push, 2);
    push.head_sha = Some(head.clone());
    let result = adapter.execute(&push).unwrap();
    assert!(result.observations.iter().any(|o| matches!(o,
        Observation::Kernel(tl3_kernel::Event::Fact { fact: tl3_kernel::Fact::Conflict, .. })
    )), "publication conflict with {target} must report a Conflict fact: {result:?}");
    assert!(
        git(
            &f.remote,
            &[
                "for-each-ref",
                &format!("refs/heads/{}", Real::branch(&push).unwrap().as_str())
            ]
        )
        .is_empty()
    );
}

#[test]
fn publication_allows_a_nonconflicting_main_advance_without_rewriting_the_checked_head() {
    let root = tempfile::tempdir().unwrap();
    let f = Fixture::create(root.path());
    let prepare = f.delivery(EffectKind::PrepareWorktree, 1);
    let mut adapter = f.adapter();
    adapter.execute(&prepare).unwrap();
    let checkout = adapter.checkout(&prepare);
    let head = commit(&checkout, "shared.txt", "checked leaf\n");
    let main = f.advance_main(false);
    let mut push = f.delivery(EffectKind::Push, 2);
    push.head_sha = Some(head.clone());
    assert!(
        adapter.execute(&push).is_ok(),
        "a clean main advance must not require Rob"
    );
    assert_eq!(
        git(
            &f.remote,
            &["rev-parse", Real::branch(&push).unwrap().as_str()]
        ),
        head
    );
    assert_ne!(git(&checkout, &["merge-base", &head, &main]), main);
}

#[test]
fn newer_rebase_replaces_an_unfinished_merge_onto_old_main() {
    let root = tempfile::tempdir().unwrap();
    let f = Fixture::create(root.path());
    let mut adapter = f.adapter();
    let prepare = f.delivery(EffectKind::PrepareWorktree, 1);
    adapter.execute(&prepare).unwrap();
    let checkout = adapter.checkout(&prepare);
    let leaf = commit(&checkout, "shared.txt", "leaf\n");
    let old_main = f.advance_main(true);
    adapter
        .execute(&f.delivery(EffectKind::StartRebase, 2))
        .unwrap();
    assert_eq!(git(&checkout, &["rev-parse", "MERGE_HEAD"]), old_main);
    // A maker cant_meet leaves this merge unfinished. A later rebase gets newer main.
    let main = f.advance_main(true);
    let result = adapter.execute(&f.delivery(EffectKind::StartRebase, 3));
    assert!(
        result.is_ok(),
        "stale merge must restart, not become a permanent incident: {result:?}"
    );
    assert_eq!(git(&checkout, &["rev-parse", "HEAD"]), leaf);
    assert_eq!(git(&checkout, &["rev-parse", "MERGE_HEAD"]), main);
    let markers = fs::read_to_string(checkout.join("shared.txt")).unwrap();
    assert!(
        markers.contains(&old_main),
        "resolution must include the newer upstream edit"
    );
    commit(&checkout, "shared.txt", "resolved newest main\n");
    assert_eq!(git(&checkout, &["merge-base", "HEAD", &main]), main);
}
