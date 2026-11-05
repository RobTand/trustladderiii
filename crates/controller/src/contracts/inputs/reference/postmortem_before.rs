// Test-only reference copied verbatim from cdacce8f99c28ca8dba8f5f76831b77b4dfcd836 crates/controller/src/postmortems.rs
pub(crate) fn case_file(
    store: &mut Store,
    snapshot: &Snapshot,
    delivery: &Delivery,
    id: i64,
    models: &tl3_config::Models,
    deadline_secs: u32,
) -> Result<Value> {
    let node = delivery.node();
    let key = node.0.to_string();
    let record = store
        .postmortem(id)?
        .ok_or(Error::Invalid("the claimed postmortem is missing"))?
        .record;
    let request = crate::request(snapshot, node)?;
    let repo = format!("{}/{}", request.repo.owner, request.repo.name);
    let world = crate::snapshot::decode(snapshot)?;
    let outcome = match world
        .nodes
        .get(node.0 as usize)
        .ok_or(Error::Invalid("postmortem node missing"))?
        .phase
    {
        Phase::Closed(outcome) => format!("{outcome:?}"),
        phase => format!("{phase:?}"),
    };
    let grades: Vec<Value> = store
        .code_grades(&key)?
        .into_iter()
        .map(|grade| {
            json!({"trigger": grade.trigger, "dimension": grade.dimension,
                "grade": grade.grade, "fact": grade.fact})
        })
        .collect();
    let phases: Vec<Value> = store
        .phase_history(&key)?
        .into_iter()
        .map(|(phase, since)| json!({"phase": phase_name(&phase), "since_ms": since}))
        .collect();
    let rows = |table: &str| -> Vec<Value> {
        snapshot
            .rows
            .iter()
            .filter(|row| row.table == table && row.value["node"] == json!(node))
            .map(|row| bounded(&row.value))
            .collect()
    };
    let document = |table: &str| {
        snapshot
            .rows
            .iter()
            .find(|row| row.table == table && row.id == key)
            .map_or(Value::Null, |row| bounded(&row.value))
    };
    let release = snapshot
        .rows
        .iter()
        .find(|row| row.table == "releases" && row.id == "tl3d")
        .and_then(|row| row.value["id"].as_str().map(str::to_owned))
        .unwrap_or_else(crate::release::id);
    // Q53 (#759): the grade judges fall back like the writer, from the
    // ladder's second entry on, past models that are out.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let grade_model = crate::adapters::workers::ladder::running(
        &crate::adapters::workers::ladder::candidates(models, "postmortem", 1, false, None)?,
        &crate::adapters::workers::ladder::quota_resets(snapshot),
        now,
    );
    Ok(json!({
        "record": {"id": id, "trigger": record.trigger, "reason": record.reason},
        "case": {
            "request": {"node": node.0, "repo": repo, "issue": request.issue,
                "title": request.title, "text": request.source.as_deref().map(bounded_text)},
            "outcome": outcome,
            "grades": grades,
            "phases": phases,
            "questions": rows("question_context"),
            "incidents": rows("incidents"),
            "plan": document("plans"),
            "brief": document("briefs"),
            "audit": document("audits"),
            "findings": document("findings"),
            "release": release,
        },
        "rubric": serde_json::from_str::<Value>(include_str!("rubric.json"))?,
        "release": release,
        "grade_model": grade_model.name,
        "deadline_secs": deadline_secs,
        "trace_tags": {"node": key, "request": key, "repo": repo,
            "issue": request.issue.to_string()},
    }))
}
