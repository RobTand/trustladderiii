use super::*;
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
