use crate::{Error, Result, executor::Delivery};
use serde_json::{Value, json};
use std::{fs, path::Path, process::Command};
use tl3_store::Store;

pub const REPORT_RULE: &str = "test-report-integrity";

fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git")
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(["-c", "core.hooksPath=/dev/null"])
        .args(args)
        .output()?;
    if !output.status.success() {
        return Err(Error::Invalid("review snapshot Git operation failed"));
    }
    Ok(output.stdout)
}

pub(super) fn session(
    delivery: &Delivery,
    store: &mut Store,
    snapshot: &tl3_store::Snapshot,
) -> Result<Option<Value>> {
    let restarted =
        crate::snapshot::decode(snapshot)?.nodes[delivery.node().0 as usize].review_restarted;
    Ok(store
        .worker_document(&delivery.node().0.to_string(), "reviewer_session")?
        .filter(|session| {
            session["attempt"] == delivery.attempt
                && session["replanned"] == delivery.replanned
                && session["review_restarted"].as_bool().unwrap_or(false) == restarted
                && session["out_of_room"] != true
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
    let session = session(delivery, store, snapshot)?;
    // A reproducing round keeps the reviewed commit, rebased after a clean rebase.
    let mut base = if delivery.round == 1 && session.is_some() {
        match run.reviewed_sha.as_deref() {
            Some(reviewed) => reviewed,
            None => session
                .as_ref()
                .map(|s| {
                    s["head_sha"]
                        .as_str()
                        .ok_or(Error::Invalid("reviewer session has no reviewed head"))
                })
                .transpose()?
                .unwrap_or(&run.base_sha),
        }
    } else {
        &run.base_sha
    };
    for sha in [head, base, &run.base_sha] {
        if !matches!(sha.len(), 40 | 64)
            || !sha
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::Invalid("invalid review commit"));
        }
    }
    if base != run.base_sha {
        let common = git(&run.repository, &["merge-base", base, &run.base_sha])?;
        if common.trim_ascii() != run.base_sha.as_bytes() {
            // Rebasing moves the check base beyond the previously reviewed history.
            base = &run.base_sha;
        }
    }
    let node = delivery.node().0.to_string();
    let report = snapshot
        .rows
        .iter()
        .find(|r| r.table == "check_reports" && r.id == format!("{node}:{head}"))
        .map(|r| &r.value)
        .ok_or(Error::Invalid("review needs exact-head check evidence"))?;
    if report["base_sha"] != run.base_sha {
        return Err(Error::Invalid("review and checks disagree on base"));
    }
    let criteria = criteria(store, delivery.node())?;
    let diff = git(
        &run.repository,
        &["diff", "--no-ext-diff", "--no-textconv", base, head, "--"],
    )?;
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

pub(crate) fn criteria(store: &mut Store, node: tl3_kernel::NodeId) -> Result<Value> {
    let node = node.0.to_string();
    let goals = store
        .worker_document(&node, "goals")?
        .ok_or(Error::Invalid("review needs goals"))?;
    let goals: Vec<_> = goals.as_array().ok_or(Error::Invalid("review goals must be a list"))?.iter().enumerate().map(|(index, goal)| {
        json!({"id": goal.get("id").cloned().unwrap_or(json!(format!("goal-{}", index + 1))), "description": goal["description"], "verification": goal["verification"]})
    }).collect();
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
    git(
        root.parent()
            .ok_or(Error::Invalid("review directory needs parent"))?,
        &[
            "clone",
            "--quiet",
            "--no-local",
            "--no-checkout",
            "--template=",
            "--",
            run.repository
                .to_str()
                .ok_or(Error::Invalid("review repository must be UTF-8"))?,
            root.to_str()
                .ok_or(Error::Invalid("review directory must be UTF-8"))?,
        ],
    )?;
    git(root, &["checkout", "--quiet", "--detach", head])?;
    Ok(())
}

pub(crate) fn validate_reply(reply: &Value, inputs: &Value) -> Result<()> {
    if reply["verdict"] == "out_of_room" {
        return Ok(());
    }
    if reply["verdict"] == "approve" && reply["head_sha"] != inputs["head_sha"] {
        return Err(Error::Invalid("review approval names a different head"));
    }
    for finding in reply["blocking_findings"]
        .as_array()
        .ok_or(Error::Invalid("review findings missing"))?
    {
        let citation = &finding["citation"];
        let entries = if citation["kind"] == "goal" {
            &inputs["goals"]
        } else {
            &inputs["standing_rules"]
        };
        if !entries
            .as_array()
            .is_some_and(|entries| entries.iter().any(|entry| entry["id"] == citation["id"]))
        {
            return Err(Error::Invalid(
                "review blocker cites an unknown goal or rule",
            ));
        }
    }
    Ok(())
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
