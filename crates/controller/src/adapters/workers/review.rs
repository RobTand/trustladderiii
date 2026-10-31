use crate::{Error, Result, executor::Delivery};
use serde_json::{Value, json};
use std::{fs, path::Path};
use tl3_store::Store;
use tl3_worktrees::{CommitId, Worktrees};

pub const REPORT_RULE: &str = "test-report-integrity";

pub(super) fn session(delivery: &Delivery, store: &mut Store) -> Result<Option<Value>> {
    Ok(store
        .worker_document(&delivery.node().0.to_string(), "reviewer_session")?
        .filter(|session| {
            session["attempt"] == delivery.attempt && session["replanned"] == delivery.replanned
        }))
}

pub(super) fn inputs(
    delivery: &Delivery,
    store: &mut Store,
    snapshot: &tl3_store::Snapshot,
) -> Result<Value> {
    let run = delivery
        .checks
        .as_ref()
        .ok_or(Error::Invalid("review needs its check plan"))?;
    let head = delivery
        .head_sha
        .as_deref()
        .ok_or(Error::Invalid("review needs its exact head"))?;
    let session = session(delivery, store)?;
    // A reproducing round keeps the reviewed commit, rebased after a clean rebase.
    let mut base = if delivery.round == 1 {
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
    if let Some(mut fixes) = store.worker_document(&node, "review_fixes")?
        && fixes["head_sha"] == head
    {
        fixes.as_object_mut().unwrap().remove("head_sha");
        inputs["review_fixes"] = fixes;
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
    rules_list.retain(|rule| rule["id"] != REPORT_RULE);
    rules_list.push(json!({"id": REPORT_RULE, "description": "Protect test reports. Block code that writes directly to the test process's file descriptors, replaces its output streams, emits text shaped like test-report events, or reaches into pytest hooks, plugins, reporter or configuration internals. Ordinary output from the program under test is allowed. In-process report custody is accepted for slice one."}));
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
    if reply["verdict"] == "approve" && reply["head_sha"] != inputs["head_sha"] {
        return Err(Error::Invalid("review approval names a different head"));
    }
    let findings: Vec<BlockingFinding> = super::protocol::reply_field(reply, "blocking_findings")?;
    for finding in &findings {
        let entries = match finding.citation.kind {
            CitationKind::Goal => &inputs["goals"],
            CitationKind::StandingRule => &inputs["standing_rules"],
        };
        if !entries.as_array().is_some_and(|entries| {
            entries
                .iter()
                .any(|entry| entry["id"] == finding.citation.id)
        }) {
            return Err(Error::Invalid(
                "review blocker cites an unknown goal or rule",
            ));
        }
    }
    Ok(findings)
}

#[cfg(test)]
mod tests {
    use super::*;

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
                let value = inputs(&f.delivery(), &mut store, &snapshot).unwrap();
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
