use crate::{Error, Result, executor::Delivery};
use serde_json::{Value, json};
use std::{fs, path::Path};
use tl3_store::Store;
use tl3_worktrees::{CommitId, Worktrees};

pub const SCOPE_RULE: &str = "request-scope";

pub const REPORT_RULE: &str = "test-report-integrity";

pub(super) fn session(delivery: &Delivery, store: &mut Store) -> Result<Option<Value>> {
    Ok(store
        .worker_document(&delivery.node().0.to_string(), "reviewer_session")?
        .filter(|session| {
            session["attempt"] == delivery.attempt && session["replanned"] == delivery.replanned
        }))
}

pub(super) fn resumes(delivery: &Delivery, session: Option<&Value>, restarted: bool) -> bool {
    delivery.round == 1
        && session.is_some_and(|session| {
            session["review_restarted"].as_bool().unwrap_or(false) == restarted
                && session["out_of_room"] != true
        })
}

pub(super) fn inputs(
    delivery: &Delivery,
    store: &mut Store,
    snapshot: &tl3_store::Snapshot,
    session: Option<&Value>,
) -> Result<Value> {
    let run = delivery
        .checks
        .as_ref()
        .ok_or(Error::Invalid("review needs its check plan"))?;
    let head = delivery
        .head_sha
        .as_deref()
        .ok_or(Error::Invalid("review needs its exact head"))?;
    // A reproducing round keeps the reviewed commit, rebased after a clean rebase.
    let mut base = if delivery.round == 1 && session.is_some() {
        match &run.reviewed_sha {
            Some(reviewed) => reviewed.clone(),
            None => session
                .as_ref()
                .map(|s| {
                    CommitId::new(
                        s["head_sha"]
                            .as_str()
                            .ok_or(Error::Invalid("reviewer session has no reviewed head"))?,
                    )
                    .map_err(Error::from)
                })
                .transpose()?
                .unwrap_or_else(|| run.base_sha.clone()),
        }
    } else {
        run.base_sha.clone()
    };
    let head_id = CommitId::new(head)?;
    let repository = Worktrees::open(&run.repository, &run.repository)?;
    if base != run.base_sha && repository.merge_base(&base, &run.base_sha)? != run.base_sha {
        // Rebasing moves the check base beyond the previously reviewed history.
        base = run.base_sha.clone();
    }
    let base = if base == run.base_sha {
        repository.merge_base(&base, &head_id)?
    } else {
        base
    };
    let node = delivery.node().0.to_string();
    let report = snapshot
        .rows
        .iter()
        .find(|r| r.table == "check_reports" && r.id == format!("{node}:{head}"))
        .map(|r| &r.value)
        .ok_or(Error::Invalid("review needs exact-head check evidence"))?;
    if report["base_sha"].as_str() != Some(run.base_sha.as_str()) {
        return Err(Error::Invalid("review and checks disagree on base"));
    }
    let criteria = criteria(store, snapshot, delivery.node())?;
    let diff = repository.diff(&base, &head_id)?;
    if diff.len() > 512 * 1024 {
        return Err(Error::Invalid("review diff exceeds the input bound"));
    }
    let mut inputs = json!({"goals": criteria["goals"], "base_sha": base, "check_base_sha": run.base_sha, "head_sha": head, "diff": String::from_utf8(diff).map_err(|_| Error::Invalid("review diff must be UTF-8"))?, "checks": report["results"], "standing_rules": criteria["standing_rules"]});
    // A test that failed a selected check is no longer selected once edited (#359).
    if let Some(failures) = report["selected_failures"].as_array() {
        let touched = repository.changed_paths_since_merge_base(&base, &head_id)?;
        let edited: Vec<_> = failures
            .iter()
            .filter_map(|test| test.as_str())
            .filter(|test| {
                let file = test.split("::").next().unwrap_or_default();
                touched.iter().any(|path| path.as_os_str() == file)
            })
            .collect();
        if !edited.is_empty() {
            inputs["edited_selected_tests"] = json!(edited);
        }
        // Missing head evidence cannot resolve a historical failure, even when no
        // test file changed. This also covers edited, filtered and unverified units.
        let displaced: Vec<_> = failures
            .iter()
            .filter_map(|test| test.as_str())
            .filter(|test| {
                let file = test.split("::").next().unwrap_or_default();
                !report["results"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|result| result["name"] == "Existing tests that import the change")
                    .any(|result| {
                        result["tests"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .any(|current| {
                                current["test"].as_str() == Some(file)
                                    && matches!(
                                        current["head"].as_str(),
                                        Some("passed" | "assertion" | "error")
                                    )
                            })
                    })
            })
            .collect();
        if !displaced.is_empty() {
            inputs["unresolved_displaced_failures"] = json!(displaced);
        }
    }
    if base != run.base_sha {
        let scope_base = repository.merge_base(&run.base_sha, &head_id)?;
        let scope_diff = repository.diff(&scope_base, &head_id)?;
        if scope_diff.len() > 512 * 1024 {
            return Err(Error::Invalid("scope diff exceeds the input bound"));
        }
        let scope_diff = String::from_utf8(scope_diff)
            .map_err(|_| Error::Invalid("scope diff must be UTF-8"))?;
        if scope_diff != inputs["diff"].as_str().unwrap() {
            inputs["scope_diff"] = json!(scope_diff);
        }
    }
    // #393: review judges a forcing against the issue's own symptom, not only the
    // planner's description of it. The issue is the text ingest read, bounded.
    let forced = report["results"].as_array().is_some_and(|results| {
        results.iter().any(|r| {
            r.get("forcing").is_some()
                || r.get("proof").is_some()
                || r.get("declared_symptom").is_some()
        })
    });
    if forced
        && let Some(issue) = store
            .worker_materials(&node)?
            .and_then(|materials| materials.get("issue").cloned())
    {
        inputs["issue"] = json!({
            "title": crate::findings::clipped(issue["title"].as_str().unwrap_or_default(), 1_000),
            "body": crate::findings::clipped(issue["body"].as_str().unwrap_or_default(), 16_000),
        });
    }
    if let Some(mut fixes) = store.worker_document(&node, "review_fixes")?
        && fixes["head_sha"] == head
    {
        fixes.as_object_mut().unwrap().remove("head_sha");
        inputs["review_fixes"] = fixes;
    }
    // The run_tests receipts the maker cited for this head (#408). The worker attests each
    // run's tree from the snapshot it staged; keep only runs on this head's tree, or on the
    // check base's for base runs (#432).
    if let Some(reply) = store.worker_document(&node, "maker_replies")?
        && reply["head"] == head
        && let Some(evidence) = reply["test_evidence"].as_array()
    {
        let head_tree = repository.tree(&head_id)?;
        let base_tree = repository.tree(&run.base_sha)?;
        let tested: Vec<Value> = evidence
            .iter()
            .filter(|entry| {
                let expected = if entry["base"] == true {
                    &base_tree
                } else {
                    &head_tree
                };
                entry["tree"].as_str() == Some(expected.as_str())
            })
            .cloned()
            .collect();
        if !tested.is_empty() {
            inputs["test_evidence"] = json!(tested);
        }
    }
    if let Some(reply) = store.worker_document(&node, "maker_replies")?
        && reply["head"] == head
        && let Some(receipt) = reply.get("generation_evidence")
        && receipt["output_tree"].as_str() == Some(repository.tree(&head_id)?.as_str())
    {
        inputs["generation_evidence"] = receipt.clone();
    }
    super::protocol::validate_inputs("reviewer", &inputs)?;
    Ok(inputs)
}

pub(crate) fn criteria(
    store: &mut Store,
    snapshot: &tl3_store::Snapshot,
    node: tl3_kernel::NodeId,
) -> Result<Value> {
    let goals = crate::goals::load(snapshot, node)?.ok_or(Error::Invalid("review needs goals"))?;
    let node = node.0.to_string();
    let materials = store
        .worker_materials(&node)?
        .ok_or(Error::Invalid("review materials missing"))?;
    let mut rules = materials
        .get("standing_rules")
        .cloned()
        .unwrap_or(json!([]));
    let rules_list = rules
        .as_array_mut()
        .ok_or(Error::Invalid("standing rules must be a list"))?;
    rules_list.retain(|rule| rule["id"] != REPORT_RULE && rule["id"] != SCOPE_RULE);
    rules_list.push(json!({"id": REPORT_RULE, "description": "Protect test reports. Block code that writes directly to the test process's file descriptors, replaces its output streams, emits text shaped like test-report events, or reaches into pytest hooks, plugins, reporter or configuration internals. Ordinary output from the program under test is allowed. In-process report custody is accepted for slice one."}));
    rules_list.push(json!({"id": SCOPE_RULE, "description": "Keep edits within the supplied request goals. Block an unrelated edit, including inside a planned file, only when it serves none of the supplied goals. Necessary supporting code and tests are in scope. Imports, test fixtures, docstrings and type hints for new or changed code, formatting the change forces, and small helper refactors the change needs are in scope. When unsure whether an edit serves the goals, record a non-blocking finding instead of blocking. Cite this standing rule, identify the unrelated hunk and quote a whole changed diff line from that hunk on its own line in evidence, including its leading plus or minus, from scope_diff when supplied or otherwise diff. Explain why the hunk serves no supplied goal. The quoted changed content must have at least 8 non-whitespace characters; otherwise record a non-blocking finding. Do not invent goals or demand additional work."}));
    Ok(json!({"goals": goals, "standing_rules": rules}))
}

pub(super) fn checkout(root: &Path, delivery: &Delivery) -> Result<()> {
    let run = delivery
        .checks
        .as_ref()
        .ok_or(Error::Invalid("review check plan missing"))?;
    let head = delivery
        .head_sha
        .as_deref()
        .ok_or(Error::Invalid("review head missing"))?;
    // Clone committed objects. Dirty maker files never become review evidence.
    fs::remove_dir_all(root)?;
    let head = CommitId::new(head)?;
    Worktrees::open(&run.repository, &run.repository)?.clone_checkout(root, &head)?;
    Ok(())
}

#[derive(serde::Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CitationKind {
    Goal,
    StandingRule,
}

#[derive(serde::Deserialize)]
pub(crate) struct Citation {
    pub kind: CitationKind,
    pub id: String,
}

#[derive(serde::Deserialize)]
pub(crate) struct BlockingFinding {
    pub citation: Citation,
    pub problem: String,
    pub evidence: String,
}

pub(crate) fn validate_reply(reply: &Value, inputs: &Value) -> Result<Vec<BlockingFinding>> {
    crate::reviewer::validate_findings(inputs, reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exhaustion_has_no_blocking_findings() {
        assert!(
            validate_reply(&json!({"verdict":"out_of_room"}), &json!({}))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn approval_and_blockers_must_match_supplied_evidence() {
        let inputs = json!({"head_sha": "a".repeat(40), "goals": [{"id":"goal-1"}], "standing_rules":[{"id":REPORT_RULE}]});
        let mut reply = json!({"verdict":"approve", "head_sha":"a".repeat(40), "blocking_findings":[], "findings":[]});
        assert!(validate_reply(&reply, &inputs).is_ok());
        reply["head_sha"] = json!("b".repeat(40));
        assert!(validate_reply(&reply, &inputs).is_err());
        for (kind, id, valid) in [
            ("goal", "goal-1", true),
            ("standing_rule", REPORT_RULE, true),
            ("goal", "invented", false),
            ("standing_rule", "preference", false),
        ] {
            let reply = json!({"verdict":"changes", "blocking_findings":[{"citation":{"kind":kind,"id":id},"problem":"Problem","evidence":"Evidence"}],"findings":[]});
            assert_eq!(validate_reply(&reply, &inputs).is_ok(), valid);
        }
    }
}

#[cfg(test)]
mod hostile_tests {
    use super::*;
    use crate::git_tests::{Repo, documents, support::Database};
    #[test]
    fn hostile_review_inputs() {
        crate::git_tests::hostile(
            "adapters::workers::review::hostile_tests::hostile_review_inputs",
            || {
                let f = Repo::new();
                let mut db = Database::start();
                let mut store = Store::open(&db.create("review_inputs")).unwrap();
                store.put_worker_materials("0", &json!({})).unwrap();
                let snapshot = documents(
                    &mut store,
                    &[
                        (
                            "goals",
                            json!([{"description":"Read the intended repository","verification":"test"}]),
                        ),
                        ("check_reports", json!({})),
                    ],
                );
                let mut snapshot = snapshot;
                let report = snapshot
                    .rows
                    .iter_mut()
                    .find(|r| r.table == "check_reports")
                    .unwrap();
                report.id = format!("0:{}", f.head);
                report.value = json!({"base_sha":f.base,"results":[]});
                let value = inputs(&f.delivery(), &mut store, &snapshot, None).unwrap();
                assert!(
                    value["diff"]
                        .as_str()
                        .unwrap()
                        .contains(&format!("+intended {}", f.path.display()))
                );
            },
        );
    }
    #[test]
    fn review_names_edited_tests_that_failed_a_selected_check() {
        // #409: a test that failed the selected check and that the change then edits is
        // no longer selected, so review must judge the edit.
        let f = Repo::new();
        let mut db = Database::start();
        let mut store = Store::open(&db.create("edited_selected")).unwrap();
        store.put_worker_materials("0", &json!({})).unwrap();
        let mut snapshot = documents(
            &mut store,
            &[
                (
                    "goals",
                    json!([{"description":"Update the readme", "verification":"test"}]),
                ),
                ("check_reports", json!({})),
            ],
        );
        let report = snapshot
            .rows
            .iter_mut()
            .find(|r| r.table == "check_reports")
            .unwrap();
        report.id = format!("0:{}", f.head);
        report.value = json!({"base_sha":f.base, "results":[], "selected_failures":
            ["README.md::test_readme", "tests/test_other.py::test_other"]});
        let value = inputs(&f.delivery(), &mut store, &snapshot, None).unwrap();
        assert_eq!(
            value["edited_selected_tests"],
            json!(["README.md::test_readme"])
        );
    }

    #[test]
    fn selection_displacement_surfaces_unedited_unchecked_historical_failures() {
        let f = Repo::new();
        let mut db = Database::start();
        let mut store = Store::open(&db.create("displaced_selected")).unwrap();
        store.put_worker_materials("0", &json!({})).unwrap();
        let mut snapshot = documents(
            &mut store,
            &[
                (
                    "goals",
                    json!([{"description":"Update the readme", "verification":"test"}]),
                ),
                ("check_reports", json!({})),
            ],
        );
        let report = snapshot
            .rows
            .iter_mut()
            .find(|r| r.table == "check_reports")
            .unwrap();
        report.id = format!("0:{}", f.head);
        report.value = json!({"base_sha":f.base, "selected_failures":
            ["tests/test_old.py", "tests/test_rechecked.py", "tests/test_unverified.py"],
            "results":[{"name":"Existing tests that import the change", "kind":"regression",
                "custody":"in-process", "verdict":"pass",
                "tests":[{"test":"tests/test_rechecked.py", "passed":true, "base":"passed", "head":"passed", "pre_existing":false},
                         {"test":"tests/test_unverified.py", "passed":false, "base":"passed", "head":"no-report", "pre_existing":false}],
                "selection":{"limit":500,"charged_cases":480,"head_units":[["tests/test_rechecked.py"]],
                    "skipped_units":[["tests/test_old.py"]], "skipped_checked":false}}]});
        let value = inputs(&f.delivery(), &mut store, &snapshot, None).unwrap();
        assert!(value.get("edited_selected_tests").is_none());
        assert_eq!(
            value["unresolved_displaced_failures"],
            json!(["tests/test_old.py", "tests/test_unverified.py"])
        );
    }

    #[test]
    fn forced_check_review_sees_the_issue() {
        // #393 review: the reviewer judges a forcing against the issue's own symptom,
        // not only the planner's description of it.
        let f = Repo::new();
        let mut db = Database::start();
        let mut store = Store::open(&db.create("forced_issue")).unwrap();
        let body = format!("PID 4242 may be live. {}", "x".repeat(20_000));
        store
            .put_worker_materials(
                "0",
                &json!({"issue": {"number": 2625, "title": "Stop depending on PID 4242",
                    "body": body, "author": "unknown", "comments": []}}),
            )
            .unwrap();
        let identity = json!({"rule":"subject-observer-v1", "declaration":null, "mode":"head-observer",
            "subjects":{"tests/test_a.py::_execute":{"base":"a".repeat(64),"head":"b".repeat(64)}},
            "source_base_tree_sha256":"f".repeat(64), "base_tree_sha256":"c".repeat(64), "head_tree_sha256":"d".repeat(64),
            "observer_sha256":{"tests/test_a.py::test_a":"e".repeat(64)}, "support_sha256":{}});
        let review = |store: &mut Store, forcing: Option<Value>, helper: bool| {
            let mut snapshot = documents(
                store,
                &[
                    (
                        "goals",
                        json!([{"description":"Update the readme", "verification":"test"}]),
                    ),
                    ("check_reports", json!({})),
                ],
            );
            let report = snapshot
                .rows
                .iter_mut()
                .find(|r| r.table == "check_reports")
                .unwrap();
            report.id = format!("0:{}", f.head);
            let mut check = json!({"name": "check-1", "kind": "demonstrating", "verdict": "pass",
                "custody": "in-process", "tests": [{"test": "tests/test_a.py::test_a",
                "base": "assertion", "head": "passed", "pre_existing": false, "passed": true}]});
            if let Some(forcing) = forcing {
                check["forcing"] = forcing;
                check["tests"][0]["head_forced"] = json!("passed");
            }
            if helper {
                check["proof"] = identity.clone();
            }
            report.value = json!({"base_sha": f.base, "results": [check]});
            inputs(&f.delivery(), store, &snapshot, None).unwrap()
        };
        let forced = review(
            &mut store,
            Some(json!({"symptom": "PID 4242 is live.", "environment": {},
                "patches": [{"target": "sampler.process_in_scope", "raises": "ValueError"}]})),
            false,
        );
        assert_eq!(forced["issue"]["title"], "Stop depending on PID 4242");
        let shown = forced["issue"]["body"].as_str().unwrap();
        assert!(shown.starts_with("PID 4242 may be live."));
        assert!(shown.chars().count() <= 16_000, "the issue body is bounded");
        assert!(
            review(&mut store, None, false).get("issue").is_none(),
            "ordinary checks do not need the issue"
        );
        let helper = review(&mut store, None, true);
        let serialized: Value =
            serde_json::from_slice(&serde_json::to_vec(&helper).unwrap()).unwrap();
        assert_eq!(serialized["checks"][0]["proof"], identity);
        assert_eq!(serialized["checks"][0]["tests"][0]["base"], "assertion");
        assert_eq!(serialized["checks"][0]["tests"][0]["head"], "passed");
        assert_eq!(serialized["issue"], forced["issue"]);
    }

    #[test]
    fn adopted_scope_evidence_excludes_main_only_progress() {
        use crate::git_tests::git;
        let f = Repo::new();
        git(&f.path, &["checkout", "--detach", &f.base]);
        fs::write(f.path.join("main-only.txt"), "main progress\n").unwrap();
        git(&f.path, &["add", "."]);
        git(&f.path, &["commit", "-qm", "main progress"]);
        let check_base = git(&f.path, &["rev-parse", "HEAD"]);
        let mut delivery = f.delivery();
        delivery.checks.as_mut().unwrap().base_sha = CommitId::new(&check_base).unwrap();
        let mut db = Database::start();
        let mut store = Store::open(&db.create("adopted_scope")).unwrap();
        store.put_worker_materials("0", &json!({})).unwrap();
        let mut snapshot = documents(
            &mut store,
            &[
                (
                    "goals",
                    json!([{"description":"Update the readme", "verification":"test"}]),
                ),
                ("check_reports", json!({})),
            ],
        );
        let report = snapshot
            .rows
            .iter_mut()
            .find(|r| r.table == "check_reports")
            .unwrap();
        report.id = format!("0:{}", f.head);
        report.value = json!({"base_sha":check_base, "results":[]});
        let value = inputs(&delivery, &mut store, &snapshot, None).unwrap();
        let scope = value
            .get("scope_diff")
            .unwrap_or(&value["diff"])
            .as_str()
            .unwrap();
        assert!(scope.contains("README.md"));
        assert!(
            !scope.contains("main-only.txt"),
            "main progress must not become scope evidence: {scope}"
        );
    }

    #[test]
    fn review_inputs_carry_the_makers_test_evidence_for_this_head() {
        // #408: receipts the maker cited reach the reviewer, for the head it made.
        // #432: only runs whose tested tree is the head's tree, or the check base's for base runs.
        let f = Repo::new();
        let head_tree =
            crate::git_tests::git(&f.path, &["rev-parse", &format!("{}^{{tree}}", f.head)]);
        let base_tree =
            crate::git_tests::git(&f.path, &["rev-parse", &format!("{}^{{tree}}", f.base)]);
        let run = |base: bool, tree: &str| {
            json!({"tests": ["test_value.py::test_value"], "base": base, "repeat": 5,
                "parallel": 0, "status": "completed", "tree": tree, "fresh": true,
                "totals": {"passed": 1, "failed": 0, "error": 0, "skipped": 0},
                "receipts": [{"action_key": "a".repeat(64), "status": "executed", "exit_code": 0,
                    "receipt_sha256": "b".repeat(64), "log_sha256": "c".repeat(64)}]})
        };
        let all = json!([
            run(false, &head_tree),
            run(true, &base_tree),
            run(false, &base_tree),
            run(true, &head_tree)
        ]);
        let mut db = Database::start();
        for (index, (head, evidence, expected)) in [
            (
                f.head.clone(),
                all.clone(),
                json!([run(false, &head_tree), run(true, &base_tree)]),
            ),
            ("d".repeat(40), all.clone(), Value::Null),
            (f.head.clone(), json!([run(false, &base_tree)]), Value::Null),
        ]
        .into_iter()
        .enumerate()
        {
            // documents() commits under one event key, so each case needs its own store.
            let mut store = Store::open(&db.create(&format!("review_evidence_{index}"))).unwrap();
            store.put_worker_materials("0", &json!({})).unwrap();
            let mut snapshot = documents(
                &mut store,
                &[
                    (
                        "goals",
                        json!([{"description":"Run it five times", "verification":"review"}]),
                    ),
                    ("check_reports", json!({})),
                    (
                        "maker_replies",
                        json!({"kind": "done", "head": head,
                        "self_check": [{"goal_id": "g", "status": "met", "evidence": "five runs"}],
                        "test_evidence": evidence}),
                    ),
                ],
            );
            let report = snapshot
                .rows
                .iter_mut()
                .find(|r| r.table == "check_reports")
                .unwrap();
            report.id = format!("0:{}", f.head);
            report.value = json!({"base_sha":f.base,"results":[]});
            let value = inputs(&f.delivery(), &mut store, &snapshot, None).unwrap();
            assert_eq!(
                value.get("test_evidence").cloned().unwrap_or(Value::Null),
                expected,
                "case {index}"
            );
        }
    }

    #[test]
    fn hostile_review_checkout() {
        crate::git_tests::hostile(
            "adapters::workers::review::hostile_tests::hostile_review_checkout",
            || {
                let f = Repo::new();
                let root = f.root.path().join("review");
                fs::create_dir(&root).unwrap();
                checkout(&root, &f.delivery()).unwrap();
                assert_eq!(
                    fs::read(root.join("README.md")).unwrap(),
                    fs::read(f.path.join("README.md")).unwrap()
                );
                assert_eq!(crate::git_tests::git(&root, &["rev-parse", "HEAD"]), f.head);
            },
        );
    }
}

#[cfg(test)]
mod reply_defects {
    use super::*;

    #[test]
    fn malformed_blockers_return_invalid_before_feedback_is_built() {
        let inputs = json!({"goals":[{"id":"g"}], "standing_rules":[]});
        for finding in [
            json!({"citation":{"kind":"goal","id":"g"},"evidence":"e"}),
            json!({"citation":{"kind":"goal","id":"g"},"problem":42,"evidence":"e"}),
            json!({"citation":{"kind":"goal","id":"g"},"problem":"p","evidence":null}),
        ] {
            let reply = json!({"verdict":"changes", "blocking_findings":[finding], "findings":[]});
            assert!(
                matches!(validate_reply(&reply, &inputs), Err(Error::Invalid(_))),
                "{reply}"
            );
        }
    }
}

#[cfg(test)]
mod scope_rule_tests {
    use super::*;
    #[test]
    fn scope_blocker_requires_a_quoted_added_hunk() {
        let inputs = json!({"diff":"+def unrelated(): return 42\n", "goals":[{"id":"g1"}], "standing_rules":[{"id":"request-scope"}]});
        for (evidence, valid) in [
            ("+def unrelated(): return 42", true),
            ("file.py has extra work", false),
            ("+invented()", false),
        ] {
            let reply = json!({"verdict":"changes", "blocking_findings":[{"citation":{"kind":"standing_rule", "id":"request-scope"}, "problem":"Serves none of the supplied goals.", "evidence":evidence}], "findings":[]});
            assert_eq!(validate_reply(&reply, &inputs).is_ok(), valid);
        }
    }

    #[test]
    fn scope_blocker_rejects_short_or_partial_changed_lines() {
        let inputs = json!({"diff":"+}\n+       x\n+abcdefg\n+abcdefgh\n+unrelated_action()\n", "goals":[{"id":"g1"}], "standing_rules":[{"id":SCOPE_RULE}]});
        for (evidence, valid) in [
            ("+}", false),
            ("+       x", false),
            ("+abcdefg", false),
            ("+abcdefgh", true),
            ("+unrelated_action() suffix", false),
            ("The unrelated helper:\n+unrelated_action()", true),
        ] {
            let reply = json!({"verdict":"changes", "blocking_findings":[{"citation":{"kind":"standing_rule", "id":SCOPE_RULE}, "problem":"This helper serves no request goal.", "evidence":evidence}], "findings":[]});
            assert_eq!(validate_reply(&reply, &inputs).is_ok(), valid, "{evidence}");
        }
    }
}

#[cfg(test)]
mod scope_fix_tests {
    use super::*;
    #[test]
    fn scope_blocker_can_quote_full_change_after_an_unrelated_fix() {
        let inputs = json!({"diff":"+requested_fix()\n", "scope_diff":"+requested_fix()\n+unrelated()\n", "goals":[{"id":"g1"}], "standing_rules":[{"id":SCOPE_RULE}]});
        let reply = json!({"verdict":"changes", "blocking_findings":[{"citation":{"kind":"standing_rule","id":SCOPE_RULE},"problem":"Still unrelated to any supplied goal.","evidence":"+unrelated()"}],"findings":[]});
        assert!(validate_reply(&reply, &inputs).is_ok());
    }
}

#[cfg(test)]
mod input_contract_tests {
    use super::*;
    use crate::git_tests::{Repo, documents, support::Database};

    fn review_with(check: &Value, f: &Repo) -> Result<Value> {
        let mut db = Database::start();
        let mut store = Store::open(&db.create("review_input_contract")).unwrap();
        store
            .put_worker_materials(
                "0",
                &json!({"issue": {
                    "title":"Preserve failure evidence", "body":"Reproduce the original exception"
                }}),
            )
            .unwrap();
        let mut snapshot = documents(
            &mut store,
            &[
                (
                    "goals",
                    json!([{"description":"Preserve failure evidence", "verification":"test"}]),
                ),
                ("check_reports", json!({})),
            ],
        );
        let report = snapshot
            .rows
            .iter_mut()
            .find(|r| r.table == "check_reports")
            .unwrap();
        report.id = format!("0:{}", f.head);
        report.value = json!({"base_sha":f.base, "results":[check]});
        inputs(&f.delivery(), &mut store, &snapshot, None)
    }

    fn check() -> Value {
        json!({"name":"failure evidence", "kind":"demonstrating", "verdict":"pass", "custody":"in-process",
            "tests":[{"test":"test_value.py::test_value", "base":"error", "head":"passed",
                "pre_existing":false, "passed":true, "message":""}]})
    }

    #[test]
    fn t653_reviewer_preserves_phase_evidence_after_checking() {
        let f = Repo::new();
        let fact = json!({"outcome":"passed", "exception":"", "assertion":false});
        let run =
            json!({"test":"test_value.py::test_value", "setup":fact, "call":fact, "teardown":fact});
        let mut check = check();
        check["tests"][0]["phases"] = json!({"base":run, "head":run, "head_forced":run});
        check["tests"][0]["phases"]["base"]["call"] = json!({
            "outcome":"failed", "exception":"builtins.TimeoutError", "assertion":false});
        let value = review_with(&check, &f).expect("reviewer must accept #630 phase evidence");
        assert_eq!(value["checks"][0], check);
        for path in [
            "/tests/0/phases/head/setup/outcome",
            "/tests/0/phases/head/setup/assertion",
            "/tests/0/phases/head/unexpected",
        ] {
            let mut invalid = value.clone();
            let mut bad = check.clone();
            if path.ends_with("unexpected") {
                bad["tests"][0]["phases"]["head"]["unexpected"] = json!(true);
            } else {
                *bad.pointer_mut(path).unwrap() = json!("invalid");
            }
            invalid["checks"][0] = bad;
            assert!(
                super::super::protocol::validate_inputs("reviewer", &invalid).is_err(),
                "{path}"
            );
        }
    }

    #[test]
    fn t653_reviewer_preserves_declared_symptom_evidence_after_checking() {
        let f = Repo::new();
        let declaration = json!({"node":"test_value.py::test_value", "exception":"builtins.TimeoutError",
            "phase":"call", "repeat":1, "parallel":0});
        let records = json!([{"event":"start", "schema":1},
            {"event":"collection", "tests":["test_value.py::test_value"]},
            {"event":"result", "test":"test_value.py::test_value", "outcome":"passed",
                "message":"", "truncated":false, "failures":[]},
            {"event":"finish", "exit_code":0}]);
        let mut check = check();
        check["declared_symptom"] = declaration.clone();
        check["symptom_evidence"] = json!({"base":f.base, "head":f.head, "forcing":null,
        "declaration":declaration, "runs":[
            {"side":"base", "run":1, "outcome":"symptom", "sha256":"a".repeat(64), "records":records, "target_hits":{}},
            {"side":"head", "run":1, "outcome":"passed", "sha256":"b".repeat(64), "records":records, "target_hits":{}},
        ]});
        let value =
            review_with(&check, &f).expect("reviewer must accept #626 declared-symptom evidence");
        assert_eq!(value["checks"][0], check);
        assert_eq!(value["issue"]["title"], "Preserve failure evidence");
        for pointer in [
            "/checks/0/declared_symptom/phase",
            "/checks/0/symptom_evidence/base",
            "/checks/0/symptom_evidence/runs/0/sha256",
            "/checks/0/symptom_evidence/runs/0/records/0/schema",
        ] {
            let mut invalid = value.clone();
            *invalid.pointer_mut(pointer).unwrap() = json!("invalid");
            assert!(
                super::super::protocol::validate_inputs("reviewer", &invalid).is_err(),
                "{pointer}"
            );
        }
        let mut invalid = value;
        invalid["checks"][0]["symptom_evidence"]["unexpected"] = json!(true);
        assert!(super::super::protocol::validate_inputs("reviewer", &invalid).is_err());
    }
}
