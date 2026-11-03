//! Request records shared by closing summaries and published proof.
use crate::executor::Output;
use crate::{Result, executor::Delivery};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use tl3_kernel::{Notice, Outcome};
use tl3_store::RowChange;
use tl3_store::{Commit, Snapshot};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PullRequestStatus {
    #[default]
    NotCaptured,
    Open,
    Closed,
    Merged,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Evidence {
    #[serde(default)]
    pub pull_request_status: PullRequestStatus,
    #[serde(default)]
    pub stop_reason: Option<String>,
    pub checks: Vec<Value>,
    pub reviews: Vec<Value>,
    pub audit: Option<Value>,
    pub queue: Vec<Value>,
    pub scratch: Vec<String>,
    #[serde(default)]
    pub scratch_history: Vec<Value>,
    pub goals: Vec<Value>,
    pub remaining_failures: Vec<Value>,
    pub artifacts: Vec<String>,
}

/// The delivered effect is the durable archive. Failed posts retain live history.
pub(crate) fn retire_delivered(
    snapshot: &Snapshot,
    commit: &mut Commit,
    delivery: &Delivery,
    ack: tl3_kernel::Ack,
) {
    if ack != tl3_kernel::Ack::Confirmed
        || !matches!(
            delivery.output,
            Output::Notice {
                notice: Notice::Summary(_),
                ..
            }
        )
    {
        return;
    }
    let prefix = format!("{}:", delivery.node().0);
    for row in &snapshot.rows {
        let owned = match row.table.as_str() {
            "check_evidence" | "review_evidence" => row.id.starts_with(&prefix),
            "scratch_evidence" => row.value["node"].as_u64() == Some(delivery.node().0),
            _ => false,
        };
        if owned {
            commit.rows.push(RowChange::Delete {
                table: row.table.clone(),
                id: row.id.clone(),
            });
        }
    }
}

const MAX_ITEMS: usize = 32;
const PROOF_BYTES: usize = 24_000;
const RECORD_BYTES: usize = 8_000;
pub(crate) const BODY_BYTES: usize = 60_000;

fn fields(value: &Value, names: &[&str]) -> Value {
    let mut out = serde_json::Map::new();
    for name in names {
        if let Some(value) = value.get(name) {
            out.insert((*name).into(), public_value(value, Some(name)));
        }
    }
    Value::Object(out)
}

fn complete_check(record: &Value, red_green: bool) -> bool {
    record["proof_missing"] != true
        && (!red_green || record["red_green_missing"] != true)
        && record["results"].as_array().is_some_and(|results| {
            !results.is_empty()
                && results.iter().all(|result| {
                    result["verdict"].as_str().is_some()
                        && (!red_green
                            || result["kind"] != "demonstrating"
                            || result["tests"].as_array().is_some_and(|tests| {
                                !tests.is_empty()
                                    && tests.iter().all(|test| {
                                        test["base"].as_str().is_some()
                                            && test["head"].as_str().is_some()
                                    })
                            }))
                        && (result.get("forcing").is_none()
                            || result["tests"].as_array().is_some_and(|tests| {
                                !tests.is_empty()
                                    && tests
                                        .iter()
                                        .all(|test| test["head_forced"].as_str().is_some())
                            }))
                })
        })
}

/// Select proof fields before cloning; reports and diagnostics can be megabytes.
pub(crate) fn check_record(record: &Value) -> Value {
    let mut out = fields(
        record,
        &[
            "head_sha",
            "base_sha",
            "action_key",
            "report",
            "pass",
            "url",
            "receipt_sha256",
            "truncated",
        ],
    );
    out["proof_missing"] = json!(!complete_check(record, false));
    out["red_green_missing"] = json!(!complete_check(record, true));
    if let Some(results) = record["results"].as_array() {
        out["failed_results"] = record
            .get("failed_results")
            .cloned()
            .unwrap_or_else(|| json!(results.iter().filter(|r| r["verdict"] != "pass").count()));
        out["forced_passed"] = record.get("forced_passed").cloned().unwrap_or_else(|| {
            json!(
                results
                    .iter()
                    .filter(|r| r.get("forcing").is_some())
                    .flat_map(|r| r["tests"].as_array().into_iter().flatten())
                    .filter(|t| t["head_forced"] == "passed")
                    .count()
            )
        });
        out["forced_other"] = record.get("forced_other").cloned().unwrap_or_else(|| {
            json!(
                results
                    .iter()
                    .filter(|r| r.get("forcing").is_some())
                    .flat_map(|r| r["tests"].as_array().into_iter().flatten())
                    .filter(|t| t["head_forced"] != "passed")
                    .count()
            )
        });
        out["result_count"] = record
            .get("result_count")
            .cloned()
            .unwrap_or(json!(results.len()));
        out["results"] = Value::Array(
            results
                .iter()
                .take(MAX_ITEMS)
                .map(|result| {
                    let mut item = fields(result, &["name", "kind", "verdict", "url"]);
                    if let Some(forcing) = result.get("forcing") {
                        // Environment values and patch bodies are opaque inputs, not proof.
                        let mut summary = fields(forcing, &["symptom"]);
                        summary["environment_count"] = forcing
                            .get("environment_count")
                            .cloned()
                            .unwrap_or_else(|| {
                                json!(forcing["environment"].as_object().map_or(0, |v| v.len()))
                            });
                        summary["patch_count"] =
                            forcing.get("patch_count").cloned().unwrap_or_else(|| {
                                json!(forcing["patches"].as_array().map_or(0, |v| v.len()))
                            });
                        item["forcing"] = summary;
                    }
                    if let Some(tests) = result["tests"].as_array() {
                        item["test_count"] = result
                            .get("test_count")
                            .cloned()
                            .unwrap_or(json!(tests.len()));
                        item["tests"] = Value::Array(
                            tests
                                .iter()
                                .take(MAX_ITEMS)
                                .map(|test| {
                                    fields(
                                        test,
                                        &["test", "base", "head", "head_forced", "passed", "url"],
                                    )
                                })
                                .collect(),
                        );
                    }
                    item
                })
                .collect(),
        );
    }
    // Keep complete entries and explicit counts. Never slice serialized JSON.
    while encode_proof(&out).expect("proof JSON").len() > RECORD_BYTES {
        out["truncated"] = json!(true);
        let results = out["results"].as_array_mut().expect("check results");
        let last = results.last_mut().expect("one bounded result fits");
        if let Some(tests) = last["tests"].as_array_mut().filter(|tests| tests.len() > 1) {
            tests.pop();
        } else if results.len() > 1 {
            results.pop();
        } else {
            break;
        }
    }
    out
}

pub(crate) fn review_record(record: &Value) -> Value {
    let mut out = fields(record, &["head_sha", "verdict", "url"]);
    out["blocking_findings"] = Value::Array(
        record["blocking_findings"]
            .as_array()
            .into_iter()
            .flatten()
            .take(MAX_ITEMS)
            .map(|finding| {
                let mut out = fields(finding, &["problem"]);
                if let Some(citation) = finding.get("citation") {
                    out["citation"] = fields(citation, &["kind", "id"]);
                }
                out
            })
            .collect(),
    );
    out
}

/// Include writes made by the event that emits the delivery. Check reports and
/// queue facts retired at close remain reporting evidence for that close.
pub fn capture(snapshot: &Snapshot, commit: &Commit, delivery: &Delivery) -> Result<Evidence> {
    let id = delivery.node().0.to_string();
    let prefix = format!("{id}:");
    let relevant = |table: &str, row: &str, value: &Value| match table {
        "check_evidence" | "review_evidence" | "check_reports" => row.starts_with(&prefix),
        "nodes" | "audits" | "goals" | "close_reasons" => row == id,
        "saved_scratch" | "scratch_evidence" => value["node"].as_u64() == Some(delivery.node().0),
        "queue_facts" => {
            value["number"].as_u64() == delivery.pull_request
                && value["repository"]["owner"].as_str() == Some(delivery.repository.owner.as_str())
                && value["repository"]["name"].as_str() == Some(delivery.repository.name.as_str())
                && value["repository"]["installation"].as_u64()
                    == Some(delivery.repository.installation)
        }
        _ => false,
    };
    // Borrow only this request's rows. Never copy or sort the whole snapshot.
    let mut rows: BTreeMap<_, _> = snapshot
        .rows
        .iter()
        .filter(|row| relevant(&row.table, &row.id, &row.value))
        .map(|row| {
            (
                (row.table.as_str(), row.id.as_str()),
                (row.arrival, &row.value),
            )
        })
        .collect();
    let last = rows
        .values()
        .map(|(arrival, _)| *arrival)
        .max()
        .unwrap_or(0);
    for (index, row) in commit.rows.iter().enumerate() {
        match row {
            RowChange::Put { table, id, value } if relevant(table, id, value) => {
                let arrival = rows
                    .get(&(table.as_str(), id.as_str()))
                    .map_or(last + index as i64 + 1, |r| r.0);
                rows.insert((table.as_str(), id.as_str()), (arrival, value));
            }
            RowChange::Delete { table, id }
                if !matches!(table.as_str(), "check_reports" | "queue_facts") =>
            {
                rows.remove(&(table.as_str(), id.as_str()));
            }
            _ => {}
        }
    }
    let mut facts = Evidence::default();
    let mut ordered: Vec<_> = rows.into_iter().collect();
    ordered.sort_by_key(|(_, (arrival, _))| *arrival);
    for ((table, row), (_, value)) in &ordered {
        match *table {
            "check_evidence" if row.starts_with(&prefix) => facts.checks.push(check_record(value)),
            "review_evidence" if row.starts_with(&prefix) => {
                facts.reviews.push(review_record(value))
            }
            "nodes" if *row == id.as_str() => {
                facts.pull_request_status = if value["merged"].as_u64().is_some() {
                    PullRequestStatus::Merged
                } else if value["pr_open"] == true {
                    PullRequestStatus::Open
                } else if value["had_pr"] == true && value["pr_open"] == false {
                    PullRequestStatus::Closed
                } else {
                    PullRequestStatus::NotCaptured
                };
            }
            "close_reasons" if *row == id.as_str() => {
                facts.stop_reason = value["reason"].as_str().map(str::to_owned)
            }
            "audits" if *row == id.as_str() => facts.audit = Some(fields(value, &["verdict"])),
            "goals" if *row == id.as_str() => {
                facts.goals = value
                    .as_array()
                    .into_iter()
                    .flatten()
                    .take(MAX_ITEMS)
                    .map(|goal| fields(goal, &["id", "description"]))
                    .collect()
            }
            "saved_scratch" if value["node"] == json!(delivery.node()) => {
                if let Some(reference) = value["reference"].as_str() {
                    facts.scratch.push(reference.into());
                }
            }
            "scratch_evidence" if value["node"] == json!(delivery.node()) => facts
                .scratch_history
                .push(fields(value, &["node", "reference", "status"])),
            "queue_facts" => {
                let raw = &value["evidence"];
                let mut compact = serde_json::Map::new();
                if let Some((kind, detail)) = raw.as_object().and_then(|v| v.iter().next()) {
                    let mut selected =
                        fields(detail, &["head", "infra", "head_check", "checks_passed"]);
                    let checks = detail["details"]["checks"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .take(MAX_ITEMS)
                        .map(|check| fields(check, &["name", "url"]))
                        .collect::<Vec<_>>();
                    if !checks.is_empty() {
                        selected["details"] = json!({"checks":checks});
                    }
                    compact.insert(kind.clone(), selected);
                }
                facts.queue.push(Value::Object(compact));
            }
            _ => {}
        }
    }
    // Older releases only kept the current check report. Preserve its exact
    // results without inventing a run key or a top-level verdict.
    for ((table, row), (_, value)) in &ordered {
        if *table == "check_reports" && row.starts_with(&prefix) {
            let head = &row[prefix.len()..];
            if !facts.checks.iter().any(|record| record["head_sha"] == head) {
                let mut record = check_record(value);
                record["head_sha"] = json!(head);
                facts.checks.push(record);
            }
        }
    }
    if let Some(check) = facts
        .checks
        .iter()
        .rev()
        .find(|c| c["head_sha"].as_str() == delivery.head_sha.as_deref())
    {
        if check["pass"] == false
            || check["report"] == false
            || check["failed_results"].as_u64().is_some_and(|n| n > 0)
        {
            facts.remaining_failures.push(check.clone());
        } else {
            facts.remaining_failures.extend(
                check["results"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|result| result["verdict"] == "fail")
                    .cloned(),
            );
        }
    }
    if let Some(review) = facts
        .reviews
        .iter()
        .rev()
        .find(|r| r["head_sha"].as_str() == delivery.head_sha.as_deref())
    {
        facts.remaining_failures.extend(
            review["blocking_findings"]
                .as_array()
                .into_iter()
                .flatten()
                .cloned(),
        );
    }
    if let Some(queue) = facts.queue.last().filter(|q| {
        q.get("CiFailed").is_some()
            || q.get("Conflict").is_some()
            || q.get("QueueBlocked").is_some()
    }) {
        facts.remaining_failures.push(queue.clone());
    }
    let mut links = BTreeSet::new();
    collect_links(
        &json!({"checks":facts.checks,"reviews":facts.reviews,"queue":facts.queue}),
        &mut links,
    );
    facts.artifacts = links.into_iter().collect();
    facts.scratch.sort();
    facts.scratch.dedup();
    Ok(facts)
}

fn collect_links(value: &Value, links: &mut BTreeSet<String>) {
    match value {
        Value::String(text) => {
            for word in text.split_whitespace() {
                let word = word.trim_matches(['(', ')', '[', ']', ',', ';', '"']);
                if word.starts_with("https://") || word.starts_with("http://") {
                    links.insert(word.to_owned());
                }
            }
        }
        Value::Array(values) => values.iter().for_each(|v| collect_links(v, links)),
        Value::Object(values) => values.values().for_each(|v| collect_links(v, links)),
        _ => {}
    }
}

/// Plain statements whose scope is the record, not everything that may have run.
pub fn summary(delivery: &Delivery) -> String {
    let Some(facts) = &delivery.evidence else {
        return "Details of checks and review were not captured in this summary.".into();
    };
    let mut lines: Vec<String> = Vec::new();
    for check in &facts.checks {
        let results = check["results"].as_array();
        let passed = check["report"] != false
            && (check["pass"] == true
                || (check.get("pass").is_none()
                    && results.is_some_and(|r| {
                        !r.is_empty()
                            && check["failed_results"].as_u64().map_or_else(
                                || r.iter().all(|c| c["verdict"] == "pass"),
                                |n| n == 0,
                            )
                    })));
        lines.push(
            if passed {
                "The recorded checks passed."
            } else {
                "Check results were recorded."
            }
            .into(),
        );
        for result in results.into_iter().flatten() {
            let verdict = match result["verdict"].as_str() {
                Some("pass") => "passed",
                Some("fail") => "failed",
                Some("infrastructure-failure") => "infrastructure failed",
                _ => "verdict not captured",
            };
            lines.push(format!(
                "Recorded check {}: {verdict}.",
                label(result["name"].as_str().unwrap_or("unnamed"))
            ));
        }
        if results.is_some_and(|r| {
            r.iter().any(|c| {
                c.get("forcing").is_some()
                    && c["tests"].as_array().is_some_and(|tests| {
                        tests
                            .iter()
                            .any(|test| test["head_forced"].as_str().is_some())
                    })
            })
        }) {
            lines.push("These checks include forced reproduction results.".into());
            lines.push(format!(
                "Forced reproduction results: {} passed, {} had another or uncaptured outcome.",
                check["forced_passed"].as_u64().unwrap_or(0),
                check["forced_other"].as_u64().unwrap_or(0)
            ));
        }
    }
    for review in &facts.reviews {
        lines.push(
            match review["verdict"].as_str() {
                Some("approve") => "The independent review approved the change.",
                Some("changes") => "The independent review requested changes.",
                _ => "An independent review result was recorded.",
            }
            .into(),
        );
    }
    if facts
        .audit
        .as_ref()
        .is_some_and(|a| a["verdict"] == "agree")
    {
        lines.push("The request's independent audit agreed with its goals.".into());
    }
    if facts.queue.iter().any(|q| q.get("CiFailed").is_some()) {
        lines.push("The merge queue later reported a check failure.".into());
    }
    if !facts.remaining_failures.is_empty() {
        lines.push("Recorded failures remain unresolved.".into());
    }
    if !matches!(
        delivery.output,
        Output::Notice {
            notice: Notice::Summary(Outcome::Merged),
            ..
        }
    ) && facts.pull_request_status != PullRequestStatus::Merged
    {
        lines.push("A merge has not been recorded for this request.".into());
    }
    if facts.checks.is_empty() {
        lines.push("Check results were not captured.".into());
    }
    if facts.reviews.is_empty() {
        lines.push("A review verdict was not captured.".into());
    }
    lines.push(
        match facts.pull_request_status {
            PullRequestStatus::Open => "The pull request is open.",
            PullRequestStatus::Closed => "The pull request is closed without a recorded merge.",
            PullRequestStatus::Merged => "The pull request was merged.",
            PullRequestStatus::NotCaptured => "The pull request status was not captured.",
        }
        .into(),
    );
    if let Some(number) = delivery.pull_request {
        lines.push(format!(
            "Proposed change: https://github.com/{}/{}/pull/{number}.",
            delivery.repository.owner, delivery.repository.name
        ));
    }
    for link in facts.artifacts.iter().take(MAX_ITEMS) {
        lines.push(format!("Recorded artifact: {}.", label(link)));
    }
    if let Some(last) = facts.queue.last() {
        lines.push(
            if last.get("Merged").is_some() {
                "The last recorded queue state is merged."
            } else if last.get("Conflict").is_some() {
                "The last recorded queue state is a conflict."
            } else if last.get("QueueBlocked").is_some() {
                "The last recorded queue state is blocked."
            } else if last.get("CiFailed").is_some() {
                "The last recorded queue state is a check failure."
            } else if last.get("DequeuedByPerson").is_some()
                || last.get("DequeuedByCommand").is_some()
            {
                "The pull request was removed from the queue."
            } else {
                "Other queue activity was recorded."
            }
            .into(),
        );
    } else {
        lines.push("The queue status was not captured.".into());
    }
    if matches!(
        delivery.output,
        Output::Notice {
            notice: Notice::Summary(Outcome::Stopped | Outcome::NotActionable | Outcome::Replaced),
            ..
        }
    ) {
        lines.push(
            match facts.stop_reason.as_deref() {
                Some("user_stop") => "Work stopped because you asked to stop.",
                Some("pr_closed") => "Work stopped because someone closed the pull request.",
                Some("pr_dequeued") => {
                    "Work stopped because someone removed the pull request from the queue."
                }
                Some("replaced") => "Work stopped because the request was replaced.",
                Some("protected_paths") => {
                    "Work stopped because the request requires changes to protected paths."
                }
                _ => "The reason for stopping was not captured.",
            }
            .into(),
        );
    }
    for blocker in facts
        .remaining_failures
        .iter()
        .filter_map(|v| v["problem"].as_str())
    {
        lines.push(format!("Recorded remaining blocker: {}.", label(blocker)));
    }
    for goal in unmet_goals(Some(facts)) {
        lines.push(format!("Recorded unmet goal: {}.", label(&goal)));
    }
    if facts.remaining_failures.is_empty() {
        lines.push("Remaining failures were not captured; this does not establish that every goal was completed.".into());
    }
    if !facts.scratch.is_empty() {
        lines.push(format!(
            "Saved scratch references: {}.",
            facts.scratch.len()
        ));
    }
    if facts
        .scratch_history
        .iter()
        .any(|v| v["status"] == "pruned")
    {
        lines.push("Previously saved scratch was pruned.".into());
    }
    lines.dedup();
    lines.join(" ")
}

pub(crate) fn label(text: &str) -> String {
    let text: String = crate::memory::redact_text(text)
        .chars()
        .take(512)
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect();
    serde_json::to_string(&text)
        .expect("string label")
        .replace('`', "'")
        .replace('<', "[")
        .replace('>', "]")
        .replace('@', "[at]")
}

fn proof_required(text: &str) -> bool {
    static PUBLICATION: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)\b(?:pr|pull request|publish(?:ed|ing)?)\b")
            .expect("publication pattern")
    });
    let text = text.to_lowercase();
    PUBLICATION.is_match(&text)
        && ["evidence", "proof", "red/green", "red and green"]
            .iter()
            .any(|word| text.contains(word))
}

/// Only explicit goal-cited blockers establish unmet goals.
pub fn unmet_goals(evidence: Option<&Evidence>) -> Vec<String> {
    let Some(evidence) = evidence else {
        return Vec::new();
    };
    evidence
        .goals
        .iter()
        .filter(|goal| {
            evidence.remaining_failures.iter().any(|failure| {
                failure["citation"]["kind"] == "goal" && failure["citation"]["id"] == goal["id"]
            })
        })
        .filter_map(|goal| goal["description"].as_str().map(str::to_owned))
        .collect()
}

fn current<'a>(records: &'a [Value], delivery: &Delivery) -> Vec<&'a Value> {
    records
        .iter()
        .filter(|record| {
            delivery
                .head_sha
                .as_deref()
                .is_some_and(|head| record["head_sha"] == head)
        })
        .collect()
}

/// PR evidence is rendered by code from records for the published head.
/// Missing proof fails before any GitHub write.
pub fn publication(delivery: &Delivery, request: &str) -> Result<String> {
    let red_green = |text: &str| {
        let text = text.to_lowercase();
        text.contains("red/green") || text.contains("red and green")
    };
    let needs_red_green = red_green(request)
        || delivery
            .evidence
            .as_ref()
            .is_some_and(|e| red_green(&serde_json::to_string(&e.goals).expect("JSON goals")));
    let required = proof_required(request)
        || delivery
            .evidence
            .as_ref()
            .is_some_and(|e| e.goals.iter().any(|g| proof_required(&g.to_string())));
    let checks = delivery
        .evidence
        .as_ref()
        .map(|e| current(&e.checks, delivery))
        .unwrap_or_default();
    let complete = !checks.is_empty()
        && checks
            .iter()
            .all(|record| complete_check(record, needs_red_green));
    if required && !complete {
        return Err(crate::Error::Permanent {class:"pr_evidence", source:"The request requires published proof, but completed evidence for this head was not captured".into()});
    }
    if checks.is_empty() {
        return Ok(String::new());
    }
    let reviews = delivery
        .evidence
        .as_ref()
        .map(|e| current(&e.reviews, delivery))
        .unwrap_or_default();
    let mut proof = json!({"head_sha":delivery.head_sha,"checks":[],"reviews":[],"artifacts":[],"truncated":false});
    for (field, records) in [
        (
            "checks",
            checks.into_iter().map(check_record).collect::<Vec<_>>(),
        ),
        (
            "reviews",
            reviews.into_iter().map(review_record).collect::<Vec<_>>(),
        ),
        (
            "artifacts",
            delivery
                .evidence
                .as_ref()
                .into_iter()
                .flat_map(|e| &e.artifacts)
                .take(MAX_ITEMS)
                .map(|link| public_value(&json!(link), None))
                .collect::<Vec<_>>(),
        ),
    ] {
        for record in records {
            proof[field]
                .as_array_mut()
                .expect("proof array")
                .push(record);
            // JSON may escape Markdown characters to six bytes, so reserve for that expansion.
            if encode_proof(&proof)?.len() > PROOF_BYTES - 512 {
                proof[field].as_array_mut().expect("proof array").pop();
                proof["truncated"] = json!(true);
                break;
            }
        }
    }
    let records = encode_proof(&proof)?;
    Ok(format!(
        "<!-- tl3:recorded-evidence:start -->\n## Recorded evidence\n\n```json\n{records}\n```\n<!-- tl3:recorded-evidence:end -->"
    ))
}

fn encode_proof(value: &Value) -> Result<String> {
    Ok(serde_json::to_string(value)?
        .replace('`', "\\u0060")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('@', "\\u0040"))
}

pub(crate) fn validate_body(body: &str) -> Result<()> {
    if body.len() > BODY_BYTES {
        return Err(crate::Error::Permanent {class:"pr_evidence", source:"PR body exceeds the 60000-byte publication budget; shorten the existing prose or publish report artifacts".into()});
    }
    Ok(())
}

// Redact values before serializing. Redacting JSON text can consume its quotes,
// and memory's generic hash filter also removes the IDs needed to verify proof.
fn public_value(value: &Value, field: Option<&str>) -> Value {
    if let (Some(field), Some(text)) = (field, value.as_str())
        && matches!(
            field,
            "head_sha" | "base_sha" | "action_key" | "receipt_sha256" | "log_sha256" | "tree"
        )
        && matches!(text.len(), 40 | 64)
        && text.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return value.clone();
    }
    if field.is_some_and(crate::memory::sensitive_key) {
        return json!("[redacted]");
    }
    match value {
        Value::String(text) => json!(
            crate::memory::redact_text(text)
                .chars()
                .take(512)
                .collect::<String>()
        ),
        Value::Array(values) => Value::Array(
            values
                .iter()
                .take(MAX_ITEMS)
                .map(|v| public_value(v, None))
                .collect(),
        ),
        Value::Object(values) => Value::Object(
            values
                .iter()
                .take(MAX_ITEMS)
                .map(|(k, v)| (crate::memory::redact_text(k), public_value(v, Some(k))))
                .collect(),
        ),
        _ => value.clone(),
    }
}

/// Replace only our generated section. Repeated publication preserves other text.
pub fn with_publication(body: &str, evidence: &str) -> String {
    if evidence.is_empty() {
        return body.into();
    }
    let start = "<!-- tl3:recorded-evidence:start -->";
    let end = "<!-- tl3:recorded-evidence:end -->";
    if let Some((before, rest)) = body.split_once(start)
        && let Some((_, after)) = rest.split_once(end)
    {
        return format!("{before}{evidence}{after}");
    }
    format!("{}\n\n{evidence}", body.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::{
        github::writer_input,
        workers::writer::{
            FakeWriter, TextDraft, WriteRequest, Writer, WriterReply, checked_draft,
        },
    };
    use serde_json::json;
    use tl3_kernel::{Notice, Outcome};
    use tl3_store::{Epoch, Revision, WorldRow};

    fn delivery() -> Delivery {
        serde_json::from_value(json!({
            "key":"summary:118", "epoch":1, "generation":7,
            "output":{"Notice":{"node":118,"notice":{"Summary":"Stopped"}}},
            "head_sha":"reviewed-head", "repository":{"owner":"RobTand","name":"prismabuild","installation":1},
            "issue":2625,"pull_request":2660,"paths":[],"attempt":1,"round":0
        })).unwrap()
    }

    fn snapshot(rows: Vec<(&str, &str, Value)>) -> Snapshot {
        Snapshot {
            revision: Revision::ZERO,
            epoch: Epoch::ZERO,
            rows: rows
                .into_iter()
                .map(|(table, id, value)| WorldRow {
                    arrival: 0,
                    version: Revision::ZERO,
                    table: table.into(),
                    id: id.into(),
                    value,
                })
                .collect(),
            pending_effects: vec![],
            credit: vec![],
            interned: vec![],
        }
    }

    // Representative recorded shapes for PB #2625 req118. The source trace is
    // tr-3cbbebc9cd399bb9a8742a2018df4b8b; no live database or worker is read.
    fn fixture() -> (Snapshot, Delivery) {
        let d = delivery();
        let s = snapshot(vec![
            (
                "check_reports",
                "118:reviewed-head",
                json!({"base_sha":"base", "results":[{
                    "name":"check-1","kind":"demonstrating","verdict":"pass",
                    "forcing":{"symptom":"PID 4242 is live", "environment":{},"patches":[]},
                    "tests":[{"test":"test_departure","base":"assertion","head":"passed","head_forced":"passed","passed":true}]
                }]}),
            ),
            (
                "review_evidence",
                "118:7",
                json!({"head_sha":"reviewed-head","verdict":"approve","blocking_findings":[],"findings":[]}),
            ),
            ("audits", "118", json!({"verdict":"agree"})),
            (
                "nodes",
                "118",
                json!({"pr_open":false,"had_pr":true,"merged":null}),
            ),
            (
                "queue_facts",
                "queue-failure",
                json!({"repository":d.repository,"number":2660,
                "evidence":{"CiFailed":{"check":99,"head":"reviewed-head","infra":false,"head_check":false,
                    "details":{"checks":[{"name":"Gate","summary":"Queue check failed","url":"https://github.com/RobTand/prismabuild/actions/runs/99"}]}}}}),
            ),
            (
                "saved_scratch",
                "scratch118",
                json!({"node":118,"reference":"refs/tl3/scratch/issue-2625-node-118"}),
            ),
            (
                "goals",
                "118",
                json!([{"id":"g1","description":"Give red/green evidence in the PR body","verification":"review"}]),
            ),
            (
                "check_reports",
                "119:other-head",
                json!({"results":[{"name":"foreign","verdict":"fail"}]}),
            ),
        ]);
        (s, d)
    }

    #[test]
    fn req118_keeps_completed_forcing_review_and_later_queue_failure() {
        let (s, mut d) = fixture();
        d.evidence = Some(capture(&s, &Commit::new("stop", json!({}), &s), &d).unwrap());
        let input = writer_input(
            &d,
            Some(Notice::Summary(Outcome::Stopped)),
            "Fix process departure",
            "Fix the test",
        )
        .unwrap();
        assert_eq!(
            input.facts["evidence"]["checks"][0]["results"][0]["verdict"],
            "pass"
        );
        assert_eq!(input.facts["evidence"]["reviews"][0]["verdict"], "approve");
        assert_eq!(input.facts["evidence"]["pull_request_status"], "closed");
        assert_eq!(
            input.facts["evidence"]["queue"].as_array().unwrap().len(),
            1
        );
        assert!(input.facts["unmet_goals"].as_array().unwrap().is_empty());
        let text = checked_draft(&mut FakeWriter, "summary", input, &Default::default()).unwrap();
        assert!(text.contains("checks passed"), "{text}");
        assert!(text.contains("review approved"), "{text}");
        assert!(text.contains("queue"), "{text}");
    }

    #[test]
    fn missing_records_are_not_claimed_as_unperformed_work() {
        let d = delivery();
        let input = writer_input(
            &d,
            Some(Notice::Summary(Outcome::Stopped)),
            "Fix process departure",
            "Fix the test",
        )
        .unwrap();
        assert!(
            input.facts["unmet_goals"].as_array().unwrap().is_empty(),
            "missing facts do not prove unmet goals"
        );
        assert_eq!(input.facts["evidence_status"], "not_captured");
    }

    #[test]
    fn capture_includes_pending_scratch_and_preserves_history_after_report_deletion() {
        let (s, d) = fixture();
        let mut c = Commit::new("close", json!({}), &s);
        c.rows.push(crate::put(
            "saved_scratch",
            "new".into(),
            json!({"node":118,"reference":"new-scratch"}),
        ));
        c.rows.push(crate::put(
            "check_evidence",
            "118:6".into(),
            json!({"head_sha":"earlier-head","pass":false,"results":[]}),
        ));
        c.rows.push(tl3_store::RowChange::Delete {
            table: "check_reports".into(),
            id: "118:reviewed-head".into(),
        });
        let facts = capture(&s, &c, &d).unwrap();
        assert!(facts.scratch.contains(&"new-scratch".into()));
        assert_eq!(facts.checks.len(), 2);
        assert_eq!(
            facts.artifacts,
            vec!["https://github.com/RobTand/prismabuild/actions/runs/99"]
        );
    }

    struct FalseNegative;
    impl Writer for FalseNegative {
        fn draft(&mut self, _: &str, _: &WriteRequest) -> Result<WriterReply> {
            Ok(WriterReply::Text(TextDraft {
                text: "No tests were run. No review was completed. No work was done.".into(),
            }))
        }
    }

    #[test]
    fn summary_bypasses_untrusted_writer_verdicts() {
        let input = writer_input(
            &delivery(),
            Some(Notice::Summary(Outcome::Stopped)),
            "Fix process departure",
            "Fix the test",
        )
        .unwrap();
        let text =
            checked_draft(&mut FalseNegative, "summary", input, &Default::default()).unwrap();
        assert!(!text.contains("No tests were run"));
        assert!(text.contains("not captured"));
    }

    #[test]
    fn required_pr_proof_is_rendered_from_checks_and_forcing_results() {
        let (s, mut d) = fixture();
        d.evidence = Some(capture(&s, &Commit::new("publish", json!({}), &s), &d).unwrap());
        let body = publication(&d, "The PR body must give red/green evidence.").unwrap();
        for fact in [
            "check-1",
            "pass",
            "head_forced",
            "PID 4242 is live",
            "approve",
        ] {
            assert!(body.contains(fact), "missing {fact}: {body}");
        }
    }

    #[test]
    fn required_pr_proof_fails_closed_when_missing_or_for_another_head() {
        let mut d = delivery();
        for evidence in [
            None,
            Some(Evidence {
                checks: vec![json!({"head_sha":"other","results":[{"verdict":"pass"}]})],
                ..Default::default()
            }),
        ] {
            d.evidence = evidence;
            assert!(publication(&d, "Publish red/green evidence in the PR body.").is_err());
        }
    }

    #[test]
    fn pr_without_proof_requirement_can_have_no_recorded_checks() {
        for request in ["Update the documentation.", "Improve evidence parsing."] {
            assert_eq!(publication(&delivery(), request).unwrap(), "");
        }
    }

    #[test]
    fn repeated_publication_preserves_other_body_text_and_redacts_proof() {
        let (s, mut d) = fixture();
        d.evidence = Some(capture(&s, &Commit::new("publish", json!({}), &s), &d).unwrap());
        d.evidence.as_mut().unwrap().checks[0]["diagnostic"] =
            json!("password=secret-fixture-value");
        let evidence = publication(&d, "Publish proof in the PR body").unwrap();
        assert!(!evidence.contains("secret-fixture-value"));
        let body = with_publication(
            "Refs #2625.\nChange type: test\nUser explanation.",
            &evidence,
        );
        assert_eq!(with_publication(&body, &evidence), body);
        assert!(body.contains("User explanation."));
    }

    #[test]
    fn required_proof_comes_from_goals_and_needs_actual_forcing_results() {
        let (s, mut d) = fixture();
        d.evidence = Some(capture(&s, &Commit::new("publish", json!({}), &s), &d).unwrap());
        d.evidence.as_mut().unwrap().checks[0]["results"][0]["tests"][0]["head_forced"] =
            Value::Null;
        assert!(publication(&d, "Fix the departure test").is_err());
    }

    #[test]
    fn unmet_goals_are_only_the_reviewers_explicit_goal_blockers() {
        let facts = Evidence {
            goals: vec![
                json!({"id":"g1","description":"Publish proof"}),
                json!({"id":"g2","description":"Fix departure"}),
            ],
            remaining_failures: vec![
                json!({"citation":{"kind":"goal","id":"g1"},"problem":"Proof is missing"}),
            ],
            ..Default::default()
        };
        assert_eq!(unmet_goals(Some(&facts)), vec!["Publish proof"]);
    }
    #[test]
    fn published_proof_is_valid_json_and_preserves_verifiable_hashes() {
        let (snapshot, mut delivery) = fixture();
        let head = "a".repeat(40);
        let action = "b".repeat(64);
        delivery.evidence = Some(
            capture(
                &snapshot,
                &Commit::new("publish", json!({}), &snapshot),
                &delivery,
            )
            .unwrap(),
        );
        delivery.head_sha = Some(head.clone());
        let check = &mut delivery.evidence.as_mut().unwrap().checks[0];
        check["head_sha"] = json!(head);
        check["action_key"] = json!(action);
        check["diagnostic"] = json!("password=secret-fixture-value");
        let body = publication(&delivery, "Publish red/green evidence in the PR body").unwrap();
        let data = body
            .split_once("```json\n")
            .unwrap()
            .1
            .split_once("\n```")
            .unwrap()
            .0;
        let proof: Value =
            serde_json::from_str(data).expect("proof must remain valid JSON after redaction");
        assert_eq!(proof["head_sha"], head);
        assert_eq!(proof["checks"][0]["action_key"], action);
        assert!(!body.contains("secret-fixture-value"));
    }

    #[test]
    fn fix_round_summary_never_calls_a_model_and_reports_recorded_status() {
        struct UntrustedWriter(usize);
        impl Writer for UntrustedWriter {
            fn draft(&mut self, _: &str, _: &WriteRequest) -> Result<WriterReply> {
                self.0 += 1;
                Ok(WriterReply::Text(TextDraft {
                    text: "Tests never ran. Review did not happen. Everything is merged.".into(),
                }))
            }
        }
        let (snapshot, mut delivery) = fixture();
        delivery.evidence = Some(
            capture(
                &snapshot,
                &Commit::new("stop", json!({}), &snapshot),
                &delivery,
            )
            .unwrap(),
        );
        let input = writer_input(
            &delivery,
            Some(Notice::Summary(Outcome::Stopped)),
            "Fix process departure",
            "Fix the test",
        )
        .unwrap();
        let mut writer = UntrustedWriter(0);
        let text = checked_draft(&mut writer, "summary", input, &Default::default()).unwrap();
        assert_eq!(writer.0, 0, "closing summaries must not call the model");
        for fact in [
            "checks passed",
            "forced reproduction",
            "review approved",
            "queue",
            "stopped",
            "closed",
            "unresolved",
        ] {
            assert!(text.contains(fact), "missing {fact}: {text}");
        }
        assert!(!text.contains("never ran"));
        assert!(text.contains("https://github.com/RobTand/prismabuild/actions/runs/99"));
        assert!(text.contains("https://github.com/RobTand/prismabuild/pull/2660"));
    }

    #[test]
    fn fix_round_publication_uses_shared_sensitive_key_and_text_redaction() {
        let planted = "Z7vQ9mN4pR6xT8cL2hK5sD3f";
        let mut fields = serde_json::Map::new();
        for key in [
            "token",
            "apikey",
            "cookie",
            "passwd",
            "PGPASSWORD",
            "refresh_token",
        ] {
            fields.insert(key.into(), json!({"nested": planted}));
        }
        fields.insert(
            "environment".into(),
            json!({"ALLOWED_FORCE_VALUE":"ghp_abcdefgh1234567890abcdefgh1234567890"}),
        );
        let redacted = public_value(&Value::Object(fields), None).to_string();
        assert!(
            !redacted.contains(planted),
            "sensitive-key values escaped redaction"
        );
        assert!(
            !redacted.contains("ghp_abcdefgh"),
            "forcing values escaped text redaction"
        );
        let (snapshot, mut delivery) = fixture();
        delivery.evidence = Some(
            capture(
                &snapshot,
                &Commit::new("publish", json!({}), &snapshot),
                &delivery,
            )
            .unwrap(),
        );
        delivery.evidence.as_mut().unwrap().checks[0]["results"][0]["forcing"]["environment"] =
            json!({"ALLOWED_FORCE_VALUE":planted});
        let published = publication(&delivery, "Publish proof in the PR").unwrap();
        assert!(
            !published.contains(planted),
            "opaque forcing values must not be published"
        );
    }

    #[test]
    fn fix_round_eight_mib_report_publishes_bounded_proof_and_artifacts() {
        let (snapshot, mut delivery) = fixture();
        delivery.evidence = Some(
            capture(
                &snapshot,
                &Commit::new("publish", json!({}), &snapshot),
                &delivery,
            )
            .unwrap(),
        );
        let evidence = delivery.evidence.as_mut().unwrap();
        evidence.checks[0]["results"][0]["diagnostic"] =
            json!("report noise ".repeat(8 * 1024 * 1024 / 13));
        evidence.artifacts = vec!["https://example.org/results/receipt".into()];
        let proof = publication(&delivery, "Publish red/green proof in the PR body").unwrap();
        assert!(
            proof.len() <= 24_000,
            "published proof exceeds its byte budget: {}",
            proof.len()
        );
        assert!(!proof.contains("report noise"));
        for fact in [
            "test_departure",
            "pass",
            "head_forced",
            "https://example.org/results/receipt",
        ] {
            assert!(proof.contains(fact), "missing {fact}");
        }
    }

    #[test]
    fn fix_round_capture_ignores_foreign_rows_and_projects_reports() {
        let (mut snapshot, delivery) = fixture();
        snapshot.rows.push(tl3_store::WorldRow {
            arrival: 9,
            version: Revision::ZERO,
            table: "queue_facts".into(),
            id: "foreign".into(),
            value: json!({"number":999,"evidence":"unrelated obsolete shape"}),
        });
        snapshot
            .rows
            .iter_mut()
            .find(|r| r.table == "check_reports")
            .unwrap()
            .value["results"][0]["diagnostic"] = json!("noise".repeat(1024 * 1024));
        let captured = capture(
            &snapshot,
            &Commit::new("capture", json!({}), &snapshot),
            &delivery,
        )
        .unwrap();
        assert!(
            serde_json::to_vec(&captured).unwrap().len() < 24_000,
            "capture cloned full report data"
        );
        assert_eq!(captured.queue.len(), 1);
    }

    #[test]
    fn compact_proof_preserves_verdicts_when_test_details_are_omitted() {
        let (snapshot, mut delivery) = fixture();
        let mut raw = snapshot
            .rows
            .iter()
            .find(|r| r.table == "check_reports")
            .unwrap()
            .value
            .clone();
        raw["head_sha"] = json!("reviewed-head");
        raw["results"][0]["tests"] = json!((0..1000).map(|i| json!({"test":format!("test_case_{i}_{}", "x".repeat(512)),"base":"assertion","head":"passed","head_forced":"passed"})).collect::<Vec<_>>());
        let record = check_record(&raw);
        assert!(encode_proof(&record).unwrap().len() <= RECORD_BYTES);
        assert_eq!(record["results"][0]["test_count"], 1000);
        assert_eq!(record["forced_passed"], 1000);
        delivery.evidence = Some(Evidence {
            checks: vec![record],
            stop_reason: Some("user_stop".into()),
            ..Default::default()
        });
        let proof = publication(&delivery, "Publish red/green proof in the PR").unwrap();
        assert!(proof.contains("head_forced") && proof.contains("test_case_0"));
        assert!(summary(&delivery).contains("you asked to stop"));
        assert!(validate_body(&"x".repeat(BODY_BYTES + 1)).is_err());
        assert!(validate_body(&with_publication("Refs #2625.", &proof)).is_ok());
        raw["results"][0]["tests"][999]["head_forced"] = Value::Null;
        delivery.evidence.as_mut().unwrap().checks = vec![check_record(&raw)];
        assert!(
            publication(&delivery, "Publish proof in the PR").is_err(),
            "omitted evidence cannot hide incomplete forcing proof"
        );
    }

    #[test]
    fn fix_round_retention_waits_for_confirmed_summary_and_preserves_other_requests() {
        use tl3_kernel::Ack;
        let (mut snapshot, mut delivery) = fixture();
        for (table, id, value) in [
            (
                "check_evidence",
                "118:7",
                json!({"head_sha":"reviewed-head"}),
            ),
            ("review_evidence", "119:7", json!({"head_sha":"other"})),
            (
                "scratch_evidence",
                "saved",
                json!({"node":118,"status":"saved"}),
            ),
        ] {
            snapshot.rows.push(tl3_store::WorldRow {
                arrival: 0,
                version: Revision::ZERO,
                table: table.into(),
                id: id.into(),
                value,
            });
        }
        delivery.evidence = Some(
            capture(
                &snapshot,
                &Commit::new("capture", json!({}), &snapshot),
                &delivery,
            )
            .unwrap(),
        );
        for ack in [Ack::Retryable, Ack::Permanent] {
            let mut commit = Commit::new("retry", json!({}), &snapshot);
            retire_delivered(&snapshot, &mut commit, &delivery, ack);
            assert!(
                commit.rows.is_empty(),
                "evidence must survive failed delivery"
            );
        }
        let mut commit = Commit::new("delivered", json!({}), &snapshot);
        retire_delivered(&snapshot, &mut commit, &delivery, Ack::Confirmed);
        for (table, id) in [
            ("check_evidence", "118:7"),
            ("review_evidence", "118:7"),
            ("scratch_evidence", "saved"),
        ] {
            assert!(
                commit
                    .rows
                    .iter()
                    .any(|r| matches!(r, RowChange::Delete {table:t,id:i} if t==table && i==id)),
                "missing retirement {table}:{id}"
            );
        }
        assert!(
            !commit
                .rows
                .iter()
                .any(|r| matches!(r, RowChange::Delete {id,..} if id.starts_with("119:")))
        );
        assert!(
            !delivery.evidence.unwrap().checks.is_empty(),
            "durable delivery retains captured proof"
        );
    }
}
