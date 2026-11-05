//! Owed postmortems and code grades (#403, Q32). Both are recorded in the
//! commit that makes them owed, outside the kernel. The runner uses a
//! separate lane with at most two concurrent postmortems (Q39, #642).

use crate::executor::{CommentKind, Completion, Delivery, Observation, Output};
use crate::status::PostmortemBlock;
use crate::steering::Task;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tl3_kernel::{Node, NodeId, Outcome, Phase, World};
use tl3_store::{
    CodeGrade, Commit, OwedPostmortem, PostmortemResult, PostmortemUpdate, Recommendation,
    RowChange, Snapshot, Store,
};

/// The world table that keeps a claimed postmortem's delivery live.
pub(crate) const JOBS: &str = "postmortem_jobs";

/// Q39, Rob 2026-11-03 (#642): postmortems have their own lane.
const CONCURRENCY: usize = 2;

/// `rec-` and 16 hex digits of sha256 over the normalised target, operation,
/// before and after: the same change counts the same in every postmortem.
pub(crate) fn fingerprint(target: &str, operation: &str, before: &str, after: &str) -> String {
    let words = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut digest = Sha256::new();
    for field in [
        words(target).to_lowercase(),
        words(operation).to_lowercase(),
        words(before),
        words(after),
    ] {
        digest.update((field.len() as u64).to_be_bytes());
        digest.update(field.as_bytes());
    }
    let hex: String = digest.finalize()[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("rec-{hex}")
}

/// A postmortem reply as the worker protocol accepts it: the role's schema,
/// and at most three recommendations, which code checks as well.
pub(crate) fn check_reply(reply: &Value) -> Result<()> {
    crate::postmortem_reply::validate(reply)
}

fn recommendations(reply: &Value) -> Result<&Vec<Value>> {
    crate::postmortem_reply::recommendations(reply)
}

/// Redact every string in a JSON value, as TL3 stores and retains it.
fn redacted(value: &Value) -> Value {
    match value {
        Value::String(text) => json!(crate::memory::redact_text(text)),
        Value::Array(items) => items.iter().map(redacted).collect(),
        Value::Object(items) => items
            .iter()
            .map(|(key, item)| (key.clone(), redacted(item)))
            .collect(),
        other => other.clone(),
    }
}

fn text(value: &Value) -> String {
    crate::memory::redact_text(value.as_str().unwrap_or_default())
}

/// The postmortem a run claimed, as its result is stored.
#[derive(Clone, Debug)]
pub(crate) struct Claimed {
    pub id: i64,
    pub node: String,
    pub repo: String,
    pub issue: u64,
    pub title: String,
}

/// What a valid reply stores: redacted text, ranked and fingerprinted
/// recommendations, tokens and cache rate as later code grades, and the
/// lessons for Hindsight. A fourth recommendation is refused here too.
pub(crate) fn result(claimed: &Claimed, reply: &Value) -> Result<PostmortemResult> {
    let written = &reply["postmortem"];
    let mut ranked = Vec::new();
    for (rank, item) in recommendations(reply)?.iter().enumerate() {
        let [
            phase,
            kind,
            target,
            operation,
            before,
            after,
            evidence,
            effect,
        ] = [
            "phase",
            "kind",
            "target",
            "operation",
            "before",
            "after",
            "evidence",
            "effect",
        ]
        .map(|field| text(&item[field]));
        ranked.push(Recommendation {
            fingerprint: fingerprint(&target, &operation, &before, &after),
            rank: i16::try_from(rank + 1).map_err(|_| Error::Invalid("recommendation rank"))?,
            phase,
            kind,
            target,
            operation,
            before,
            after,
            evidence,
            effect,
        });
    }
    // Grades keep their MLflow assessment ids as written: they are ids, not text.
    let grades: Vec<Value> = reply["grades"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|grade| {
            let mut grade = grade.clone();
            grade["rationale"] = json!(text(&grade["rationale"]));
            grade
        })
        .collect();
    let notes = [text(&written["went_wrong"]), text(&written["notes"])]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    let usage = &reply["usage"];
    let count = |field: &str| usage[field].as_u64().unwrap_or(0);
    let later = |dimension: &str, fact: Value| CodeGrade {
        node: claimed.node.clone(),
        trigger: format!("postmortem:{}", claimed.id),
        rubric_version: RUBRIC_VERSION,
        dimension: dimension.into(),
        grade: None,
        fact,
    };
    let (input, cached) = (count("input_tokens"), count("cache_read_input_tokens"));
    let rate = if input > 0 {
        Some(cached as f64 / input as f64)
    } else {
        None
    };
    let code_grades = vec![
        later(
            "tokens",
            json!({"traces": count("traces"), "input_tokens": input,
                "output_tokens": count("output_tokens"), "cache_read_input_tokens": cached,
                "cache_creation_input_tokens": count("cache_creation_input_tokens")}),
        ),
        later(
            "cache_rate",
            json!({"rate": rate,
                "input_tokens": input, "cache_read_input_tokens": cached}),
        ),
    ];
    let cause = text(&written["cause"]);
    let summary = text(&written["summary"]);
    let memory = lessons(claimed, &cause, &summary, written, &ranked);
    Ok(PostmortemResult {
        model: text(&reply["models"]["postmortem"]),
        cause,
        summary,
        timeline: redacted(&written["timeline"]),
        process_failures: redacted(&written["process_failures"]),
        grades: Value::Array(grades),
        notes,
        assessment_ids: reply["assessment_ids"].clone(),
        recommendations: ranked,
        code_grades,
        memory: Some(memory),
    })
}

/// The postmortem's lessons as one Hindsight record, tagged `postmortem`, with
/// an operation id that is the same on every replay.
fn lessons(
    claimed: &Claimed,
    cause: &str,
    summary: &str,
    written: &Value,
    recommendations: &[Recommendation],
) -> (String, Value) {
    let mut hash = Sha256::digest(
        json!(["postmortem", claimed.id, claimed.node, claimed.repo])
            .to_string()
            .as_bytes(),
    );
    hash[6] = (hash[6] & 0x0f) | 0x50;
    hash[8] = (hash[8] & 0x3f) | 0x80;
    let h: String = hash[..16].iter().map(|b| format!("{b:02x}")).collect();
    let operation = format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    );
    let record = json!({
        "repo": claimed.repo, "issue": claimed.issue,
        "title": crate::memory::redact_text(&claimed.title), "cause": cause, "summary": summary,
        "lessons": redacted(&written["lessons"]),
        "recommendations": recommendations.iter().map(|item| json!({
            "fingerprint": item.fingerprint, "target": item.target,
            "operation": item.operation, "effect": item.effect})).collect::<Vec<_>>(),
    });
    let item = json!({
        "content": record.to_string(),
        "document_id": format!("tl3-postmortem-{}", claimed.id),
        "context": "TL3 postmortem lessons",
        "tags": ["postmortem", format!("repo:{}", claimed.repo),
            format!("request:{}", claimed.node), format!("cause:{cause}")],
        "metadata": {"source": "tl3", "postmortem": claimed.id.to_string()},
    });
    (operation, item)
}

/// The simulated worker's reply: a fixed, valid postmortem.
pub(crate) fn simulated_reply() -> Value {
    json!({
        "postmortem": {
            "summary": "The maker needed one fix round before the change merged.",
            "cause": "tl3",
            "went_wrong": "The maker's first head failed a check the plan named.",
            "timeline": [{"at": "Making", "event": "The maker ran."},
                         {"at": "Reviewing", "event": "The reviewer asked for a fix."}],
            "process_failures": [{"phase": "maker", "kind": "prompt or skill defect",
                "what": "The maker skipped a planned check.", "evidence": "The review's finding."}],
            "recommendations": [{"phase": "maker", "kind": "prompt", "target": "maker",
                "operation": "add", "before": "", "after": "Run every check the plan names.",
                "evidence": "The review's finding.", "effect": "Fewer fix rounds."}],
            "notes": "",
            "lessons": ["A plan's named checks must all run before a maker replies."]
        },
        "grades": [{"dimension": "correctness", "grade": 4, "rationale": "The review's finding.",
            "judge_model": "simulated", "assessment_id": null}],
        "assessment_ids": [],
        "usage": {"traces": 0, "input_tokens": 0, "output_tokens": 0,
            "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0},
        "models": {"postmortem": "simulated", "grades": "simulated"}
    })
}

/// The case file for one postmortem, written into the worker's inputs: the
/// request, its outcome and grades, phase times, questions with their #322
/// findings, incidents, plan, brief, audit and the release that ran, with
/// the run's whole wall-clock allowance.
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

/// Case-file documents stay readable but bounded; the judge reads the rest in traces.
const DOCUMENT_BYTES: usize = 64 * 1024;

fn bounded(value: &Value) -> Value {
    let encoded = value.to_string();
    if encoded.len() <= DOCUMENT_BYTES {
        value.clone()
    } else {
        json!({"truncated": true, "summary": bounded_text(&encoded)})
    }
}

fn bounded_text(text: &str) -> String {
    let mut end = text.len().min(DOCUMENT_BYTES / 2);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

/// A postmortem failure class from the fixed set; anything else is a worker failure.
fn failure_class(failure: Option<&str>) -> &'static str {
    match failure {
        Some("invalid_reply") => "invalid_reply",
        Some("launch_failed") => "launch_failed",
        Some("no_progress") => "no_progress",
        Some("deadline") => "deadline",
        _ => "worker_failed",
    }
}

/// Failed runs in a row since startup or recovery that pause the runner.
const PAUSE_AFTER_FAILURES: usize = 3;

/// The incident class of a paused runner.
pub(crate) const PAUSED: &str = "postmortems_paused";

/// A postmortem the controller stopped for making no progress failed with
/// `no_progress`, whatever the stopped worker's record says (#486).
pub(crate) fn stalled(mut completion: Completion) -> Completion {
    for observation in &mut completion.observations {
        if let Observation::Postmortem {
            reply: None,
            failure,
            judge_failure,
            ..
        } = observation
        {
            *failure = Some("no_progress".into());
            *judge_failure = None;
        }
    }
    completion
}

/// The models whose quota the postmortem lane needs.
#[derive(Clone, Debug, PartialEq)]
pub struct PostmortemPace {
    /// Every entry of the postmortem ladder, in order. Entry 0 writes the
    /// postmortem and entry 1, if present, grades it; each falls back down the
    /// whole ladder past models that are out, so nothing is claimed only while
    /// every entry is out (#759).
    pub models: Vec<String>,
}

impl PostmortemPace {
    pub fn from_config(config: &tl3_config::Config) -> Self {
        Self {
            models: config
                .models
                .postmortem
                .iter()
                .map(|model| model.name.clone())
                .collect(),
        }
    }
}

pub(crate) const SCHEDULER: &str = "postmortem_scheduler";

#[derive(Serialize, Deserialize)]
struct Recovery {
    since: i64,
    validation: Validation,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Validation {
    Pending,
    Running { key: String },
    Passed,
    Failed,
}

impl Recovery {
    fn load(snapshot: &Snapshot) -> Result<Option<Self>> {
        snapshot
            .rows
            .iter()
            .find(|row| row.table == SCHEDULER && row.id == "recovery")
            .map(|row| crate::decode_stored(row.value.clone()))
            .transpose()
    }

    fn row(&self) -> RowChange {
        crate::put(SCHEDULER, "recovery".into(), json!(self))
    }
}

fn record_block(snapshot: &Snapshot, reason: Option<PostmortemBlock>, commit: &mut Commit) {
    let current = snapshot
        .rows
        .iter()
        .find(|row| row.table == SCHEDULER && row.id == "status");
    let value = json!({"blocked": reason});
    if current.is_some_and(|row| row.value == value) || (current.is_none() && reason.is_none()) {
        return;
    }
    commit
        .rows
        .push(crate::put(SCHEDULER, "status".into(), value));
}

impl crate::Controller {
    /// Judge owed postmortems from now on, at `pace`. tl3d turns this on as it
    /// starts; embedders and tests whose workers run no postmortem role leave
    /// it off, and owed rows then wait.
    pub fn run_postmortems(&mut self, pace: PostmortemPace) {
        self.postmortems = Some(pace);
    }

    /// Claim the oldest owed postmortem when the lane has a free slot, in
    /// the commit that creates its worker delivery.
    pub(crate) fn start_postmortem(&mut self, snapshot: &Snapshot) -> Result<bool> {
        let Some(pace) = &self.postmortems else {
            return Ok(false);
        };
        // Q53 (#759): the lane falls back like other roles, so it waits only
        // while every model in its ladder is out.
        let now = self.now_ms();
        let quota = !pace.models.is_empty()
            && pace.models.iter().all(|model| {
                snapshot.rows.iter().any(|row| {
                    row.table == "model_quotas"
                        && row.id == *model
                        && row.value["reset_ms"]
                            .as_u64()
                            .is_some_and(|reset| reset > now)
                })
            });
        let running = snapshot.rows.iter().filter(|row| row.table == JOBS).count();
        let Some(owed) = self.store.owed_postmortem()? else {
            return self.postmortem_blocked(snapshot, None);
        };
        let mut recovery = Recovery::load(snapshot)?;
        let since = recovery.as_ref().map_or(self.postmortem_since, |state| {
            state.since.max(self.postmortem_since)
        });
        let failures = self.store.postmortem_failure_streak(since)?;
        if failures.len() >= PAUSE_AFTER_FAILURES
            || recovery
                .as_ref()
                .is_some_and(|state| matches!(state.validation, Validation::Failed))
        {
            let mut commit = Commit::new(
                format!("postmortem:paused:{}", snapshot.revision.get()),
                json!({"postmortems_paused": failures}),
                snapshot,
            );
            self.pause_postmortems(snapshot, &failures, &mut commit)?;
            if commit.rows.is_empty() {
                return Ok(false);
            }
            self.store.commit(&commit)?;
            return Ok(true);
        }
        let validation = recovery
            .as_ref()
            .is_some_and(|state| matches!(state.validation, Validation::Pending));
        if quota {
            return self.postmortem_blocked(snapshot, Some(PostmortemBlock::ModelQuota));
        }
        let world = crate::snapshot::decode(snapshot)?;
        if recovery
            .as_ref()
            .is_some_and(|state| matches!(state.validation, Validation::Running { .. }))
        {
            return self.postmortem_blocked(snapshot, Some(PostmortemBlock::Validating));
        }
        if running >= CONCURRENCY {
            return self.postmortem_blocked(snapshot, Some(PostmortemBlock::LaneFull));
        }
        let node = NodeId(
            owed.node
                .parse()
                .map_err(|_| Error::Invalid("invalid postmortem node"))?,
        );
        let n = world
            .nodes
            .get(node.0 as usize)
            .ok_or(Error::Invalid("postmortem node missing"))?;
        let request = crate::request(snapshot, node)?;
        let key = format!("postmortem:{}:{}", owed.id, snapshot.revision.get());
        let delivery = Delivery {
            key: key.clone(),
            epoch: snapshot.epoch.get(),
            output: Output::Comment {
                node,
                kind: CommentKind::Postmortem,
            },
            generation: n.generation,
            head_sha: None,
            repository: request.repo,
            issue: request.issue,
            pull_request: None,
            paths: vec![],
            attempt: 1,
            round: 0,
            replanned: n.replanned,
            checks: None,
            job_generation: None,
            question: None,
            marker_key: None,
            note: None,
            scope_removals: None,
            question_context: None,
            request_text: None,
            evidence: None,
            proposed: None,
            steering: Some(Task::Postmortem {
                id: owed.id,
                run_key: key.clone(),
            }),
        };
        let mut commit = Commit::new(
            format!("postmortem:claim:{key}"),
            json!({"postmortem_claim": owed.id, "run_key": key, "postmortem_validation": validation}),
            snapshot,
        );
        record_block(snapshot, None, &mut commit);
        if validation && let Some(state) = recovery.as_mut() {
            state.validation = Validation::Running { key: key.clone() };
            commit.rows.push(state.row());
        }
        commit.rows.push(self.allowance_row(delivery.clone()));
        commit
            .rows
            .push(crate::put(JOBS, key.clone(), json!(delivery)));
        commit
            .rows
            .push(crate::put("watches", key.clone(), json!(delivery)));
        commit.effects.push(delivery.into_effect());
        commit.postmortem_updates.push(PostmortemUpdate::Claim {
            id: owed.id,
            run_key: key,
            rubric_version: RUBRIC_VERSION,
        });
        self.store.commit(&commit)?;
        Ok(true)
    }

    fn postmortem_blocked(
        &mut self,
        snapshot: &Snapshot,
        reason: Option<PostmortemBlock>,
    ) -> Result<bool> {
        let mut commit = Commit::new(
            format!("postmortem:gate:{}", snapshot.revision.get()),
            json!({"postmortem_blocked": reason}),
            snapshot,
        );
        record_block(snapshot, reason, &mut commit);
        if commit.rows.is_empty() {
            return Ok(false);
        }
        self.store.commit(&commit)?;
        Ok(true)
    }

    /// The clear and its recovery permit share the incident's commit.
    pub(crate) fn reset_postmortem_pause(&self, snapshot: &Snapshot, commit: &mut Commit) {
        commit.rows.push(
            Recovery {
                since: snapshot.revision.get(),
                validation: Validation::Pending,
            }
            .row(),
        );
        record_block(snapshot, None, commit);
    }

    fn pause_postmortems(
        &self,
        snapshot: &Snapshot,
        failures: &[String],
        commit: &mut Commit,
    ) -> Result<()> {
        let title = format!(
            "Postmortems paused after failed runs: {}",
            failures.join(", ")
        );
        let already_open = snapshot.rows.iter().any(|row| {
            row.table == "incidents"
                && row.value["open"] == true
                && row.value["signature"]["class"] == PAUSED
        });
        if !already_open {
            self.input_failed(
                snapshot,
                "postmortems",
                None,
                &title,
                None,
                crate::executor::Upstream::Workers,
                Error::Permanent {
                    class: PAUSED,
                    source: "postmortem failure streak or recovery validation failed".into(),
                },
                commit,
            )?;
        }
        record_block(snapshot, Some(PostmortemBlock::PausedStreak), commit);
        Ok(())
    }

    fn finish_validation(
        &self,
        snapshot: &Snapshot,
        key: &str,
        failure: Option<&str>,
        commit: &mut Commit,
    ) -> Result<()> {
        let Some(mut state) = Recovery::load(snapshot)? else {
            return Ok(());
        };
        if !matches!(&state.validation, Validation::Running { key: claimed } if claimed == key) {
            return Ok(());
        }
        state.validation = if let Some(failure) = failure {
            self.pause_postmortems(snapshot, &[failure.into()], commit)?;
            Validation::Failed
        } else {
            Validation::Passed
        };
        commit.rows.push(state.row());
        Ok(())
    }

    /// Store a postmortem's result, or its failure, for the claim's run key only.
    pub(crate) fn postmortem_finished(
        &mut self,
        snapshot: &Snapshot,
        key: &str,
        reply: Option<Value>,
        failure: Option<String>,
        judge_failure: Option<tl3_store::JudgeFailure>,
        commit: &mut Commit,
    ) -> Result<()> {
        let Some(job) = snapshot
            .rows
            .iter()
            .find(|row| row.table == JOBS && row.id == key)
        else {
            // A result for a finished or released claim changes nothing.
            return Ok(());
        };
        let delivery: Delivery = crate::decode_stored(job.value.clone())?;
        let Some(Task::Postmortem { id, run_key }) = delivery.steering.clone() else {
            return Err(Error::Invalid("invalid postmortem job"));
        };
        retire_job(commit, key);
        let judged = reply.map(|reply| {
            let request = crate::request(snapshot, delivery.node())?;
            let claimed = Claimed {
                id,
                node: delivery.node().0.to_string(),
                repo: format!("{}/{}", request.repo.owner, request.repo.name),
                issue: request.issue,
                title: request.title,
            };
            check_reply(&reply)?;
            result(&claimed, &reply)
        });
        let update = match judged {
            Some(Ok(result)) => PostmortemUpdate::Done {
                id,
                run_key,
                result: Box::new(result),
            },
            Some(Err(Error::Store(error))) => return Err(Error::Store(error)),
            Some(Err(_)) => PostmortemUpdate::Failed {
                id,
                run_key,
                judge_failure: None,
                failure: "invalid_reply".into(),
            },
            None => PostmortemUpdate::Failed {
                id,
                run_key,
                judge_failure,
                failure: failure_class(failure.as_deref()).into(),
            },
        };
        let failure = match &update {
            PostmortemUpdate::Failed {
                failure,
                judge_failure,
                ..
            } => Some(judge_failure.map_or_else(|| failure.clone(), |cause| cause.to_string())),
            _ => None,
        };
        self.finish_validation(snapshot, key, failure.as_deref(), commit)?;
        commit.postmortem_updates.push(update);
        Ok(())
    }

    /// A permanent launch failure fails its claim and re-pauses a validation run.
    pub(crate) fn postmortem_failed(
        &mut self,
        snapshot: &Snapshot,
        delivery: &Delivery,
        class: &str,
        commit: &mut Commit,
    ) -> Result<()> {
        let Some(Task::Postmortem { id, run_key }) = &delivery.steering else {
            return Err(Error::Invalid("invalid postmortem delivery"));
        };
        retire_job(commit, &delivery.key);
        self.finish_validation(snapshot, run_key, Some(failure_class(Some(class))), commit)?;
        commit.postmortem_updates.push(PostmortemUpdate::Failed {
            id: *id,
            run_key: run_key.clone(),
            judge_failure: None,
            failure: failure_class(Some(class)).into(),
        });
        Ok(())
    }

    /// At startup, a `running` postmortem with no live delivery is owed again.
    pub(crate) fn recover_postmortems(&mut self) -> Result<()> {
        let snapshot = self.store.load()?;
        // Runs that failed before this start never pause the runner.
        self.postmortem_since = snapshot.revision.get();
        let has = |table: &str, key: &str| {
            snapshot
                .rows
                .iter()
                .any(|row| row.table == table && row.id == key)
        };
        // A claim is live while its effect is pending, its watch reads the
        // worker's record, or its result waits in the inbox.
        let live = |key: &str| {
            snapshot
                .pending_effects
                .iter()
                .any(|effect| effect.key == key)
                || has("watches", key)
                || snapshot
                    .rows
                    .iter()
                    .any(|row| row.table == "inbox" && row.value["Postmortem"]["key"] == key)
        };
        let mut commit = Commit::new(
            format!("postmortem:recover:{}", snapshot.revision.get()),
            json!({"postmortem_recovery": snapshot.revision.get()}),
            &snapshot,
        );
        for job in snapshot.rows.iter().filter(|row| row.table == JOBS) {
            if !live(&job.id) {
                retire_job(&mut commit, &job.id);
            }
        }
        for record in self.store.running_postmortems()? {
            let Some(run_key) = record.run_key else {
                continue;
            };
            if !(has(JOBS, &run_key) && live(&run_key)) {
                self.finish_validation(&snapshot, &run_key, Some("worker_failed"), &mut commit)?;
                commit.postmortem_updates.push(PostmortemUpdate::Release {
                    id: record.id,
                    run_key,
                });
            }
        }
        if !commit.rows.is_empty() || !commit.postmortem_updates.is_empty() {
            self.store.commit(&commit)?;
        }
        Ok(())
    }
}

fn retire_job(commit: &mut Commit, key: &str) {
    for table in [JOBS, "watches"] {
        commit.rows.push(RowChange::Delete {
            table: table.into(),
            id: key.into(),
        });
    }
}

/// TL3's rubric version, in `rubric.json`. Raise it with any change that could
/// change a grade; regrading adds grades and leaves old ones as they are.
pub(crate) const RUBRIC_VERSION: i32 = 1;

/// Facts about a node's life that its world row no longer holds.
#[derive(Clone, Debug, Default)]
pub(crate) struct History {
    /// The kinds of the questions it asked Rob.
    pub questions: Vec<String>,
    /// Its incidents' cause classes, and whether each needed Rob.
    pub incidents: Vec<(String, bool)>,
    /// A worker run failed and was retried.
    pub run_failure: bool,
    /// Failed runs, effects and plans that were retried, from the retry ledger.
    pub retries: u32,
    /// Times the queue sent it back: CI failures, conflicts and dequeues.
    pub dequeues: u32,
    /// It merged with failing checks.
    pub escaped_ci: bool,
    /// Milliseconds spent in each phase that has ended, in order.
    pub phases: Vec<(String, u64)>,
}

/// Why a postmortem is owed, as the console lists it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reason {
    Escalation,
    NeededFix,
    Retry,
    ClosedWithoutMerge,
    Sampled,
    RobAsked,
}

impl Reason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Reason::Escalation => "escalation",
            Reason::NeededFix => "needed_fix",
            Reason::Retry => "retry",
            Reason::ClosedWithoutMerge => "closed_without_merge",
            Reason::Sampled => "sampled",
            Reason::RobAsked => "rob_asked",
        }
    }
}

/// Every close owes a postmortem (#642). Keep the stored reasons and record
/// a share of 1 for clean merges, so existing readers need no schema change.
pub(crate) fn close_reason(
    node: &Node,
    outcome: Outcome,
    history: &History,
) -> (Reason, Option<f64>) {
    let reason = if !history.questions.is_empty()
        || history.incidents.iter().any(|(_, needs_rob)| *needs_rob)
    {
        Reason::Escalation
    } else if node.attempt > 1 || node.round > 0 || node.replanned || node.review_stop.is_some() {
        // Q54: a stop for TL3 review is TL3's own failure to fix.
        Reason::NeededFix
    } else if history.run_failure
        || history.retries > 0
        || !history.incidents.is_empty()
        || node.run_incidents > 0
        || node.fx_incidents > 0
        || node.plan_retries > 0
    {
        Reason::Retry
    } else if outcome != Outcome::Merged {
        Reason::ClosedWithoutMerge
    } else {
        return (Reason::Sampled, Some(1.0));
    };
    (reason, None)
}

/// 5 for none, then one grade lower for each more, down to 1.
fn fewer_is_better(count: u32) -> i16 {
    5 - i16::try_from(count.min(4)).unwrap_or(4)
}

/// The rubric's code grades at close: (dimension, grade, fact).
pub(crate) fn close_grades(
    node: &Node,
    outcome: Outcome,
    history: &History,
) -> Vec<(&'static str, Option<i16>, Value)> {
    let merged = outcome == Outcome::Merged;
    let tries = u32::from(node.attempt.saturating_sub(1))
        + u32::from(node.round)
        + u32::from(node.replanned)
        + history.retries;
    let needed_rob = history.questions.len() as u32
        + history
            .incidents
            .iter()
            .filter(|(_, needs_rob)| *needs_rob)
            .count() as u32;
    let queue = if !merged {
        None
    } else if history.escaped_ci {
        Some(1)
    } else {
        Some(5 - i16::try_from(history.dequeues.min(3)).unwrap_or(3))
    };
    let credit = node.credit;
    vec![
        (
            "reliability",
            Some(if merged { fewer_is_better(tries) } else { 1 }),
            json!({"attempt": node.attempt, "round": node.round, "replanned": node.replanned,
                "retries": history.retries, "outcome": format!("{outcome:?}")}),
        ),
        (
            "escalation",
            Some(fewer_is_better(needed_rob)),
            json!({"needed_rob": needed_rob}),
        ),
        (
            "queue",
            queue,
            json!({"merged": merged, "dequeues": history.dequeues, "escaped_ci": history.escaped_ci}),
        ),
        (
            "credit",
            None,
            json!({"start": credit.start, "earned": credit.earned, "added": credit.added,
                "spent": credit.spent, "balance": serde_json::Number::from_i128(credit.balance())
                    .map(Value::Number)
                    .unwrap_or_else(|| Value::String(credit.balance().to_string()))}),
        ),
        ("questions", None, json!({"kinds": history.questions})),
        (
            "incidents",
            None,
            json!({"classes": history.incidents.iter().map(|(class, _)| class).collect::<Vec<_>>(),
                "needed_rob": history.incidents.iter().filter(|(_, needs_rob)| *needs_rob).count()}),
        ),
        (
            "phase_time",
            None,
            Value::Object(history.phases.iter().fold(
                serde_json::Map::new(),
                |mut seconds, (phase, spent)| {
                    let total = seconds.get(phase).and_then(Value::as_u64).unwrap_or(0) + spent;
                    seconds.insert(phase.clone(), json!(total));
                    seconds
                },
            )),
        ),
    ]
}

/// A postmortem owed for `node`, made owed by `trigger`.
pub(crate) fn owed(
    snapshot: &Snapshot,
    node: NodeId,
    trigger: String,
    reason: Reason,
    sample_share: Option<f64>,
) -> Result<OwedPostmortem> {
    let request = crate::request(snapshot, node)?;
    Ok(OwedPostmortem {
        node: node.0.to_string(),
        trigger,
        reason: reason.as_str().into(),
        sample_share,
        repo: format!("{}/{}", request.repo.owner, request.repo.name),
        issue: i64::try_from(request.issue)
            .map_err(|_| crate::Error::Invalid("issue number overflow"))?,
        title: request.title,
    })
}

/// The kernel zeroes its failure counters once a step succeeds, so each retry
/// is counted in the commit where its counter rises.
fn record_retries(
    snapshot: &Snapshot,
    previous: &World,
    world: &World,
    commit: &mut Commit,
) -> Result<()> {
    for (index, node) in world.nodes.iter().enumerate() {
        let Some(before) = previous.nodes.get(index) else {
            continue;
        };
        let rose = |now: u8, then: u8| u64::from(now > then);
        let added = [
            (
                "run",
                rose(node.run_failures, before.run_failures)
                    + rose(node.run_incidents, before.run_incidents),
            ),
            (
                "effect",
                rose(node.fx_failures, before.fx_failures)
                    + rose(node.fx_incidents, before.fx_incidents),
            ),
            ("plan", rose(node.plan_retries, before.plan_retries)),
        ];
        if added.iter().all(|(_, count)| *count == 0) {
            continue;
        }
        let id = index.to_string();
        let mut counts = commit
            .rows
            .iter()
            .rev()
            .find_map(|row| match row {
                RowChange::Put {
                    table,
                    id: row,
                    value,
                } if table == "retries" && *row == id => Some(value.clone()),
                _ => None,
            })
            .or_else(|| {
                snapshot
                    .rows
                    .iter()
                    .find(|row| row.table == "retries" && row.id == id)
                    .map(|row| row.value.clone())
            })
            .unwrap_or_else(|| json!({}));
        for (kind, count) in added {
            counts[kind] = json!(counts[kind].as_u64().unwrap_or(0) + count);
        }
        commit.rows.push(crate::put("retries", id, counts));
    }
    Ok(())
}

/// A phase's name as the board shows it: Planning, not Planning { must_split }.
fn phase_name(phase: &Value) -> String {
    match phase {
        Value::String(name) => name.clone(),
        Value::Object(fields) => fields.keys().next().cloned().unwrap_or_default(),
        _ => String::new(),
    }
}

impl crate::Controller {
    /// Code grades and an owed postmortem for every closed node (#642).
    pub(crate) fn record_postmortems(
        &mut self,
        snapshot: &Snapshot,
        previous: &World,
        world: &World,
        commit: &mut Commit,
    ) -> Result<()> {
        record_retries(snapshot, previous, world, commit)?;
        for (index, node) in world.nodes.iter().enumerate() {
            let Phase::Closed(outcome) = node.phase else {
                continue;
            };
            if previous
                .nodes
                .get(index)
                .is_some_and(|before| before.phase.is_closed())
            {
                continue;
            }
            let id = NodeId(index as u64);
            let history = self.history(snapshot, id, commit)?;
            for (dimension, grade, fact) in close_grades(node, outcome, &history) {
                commit.grades.push(CodeGrade {
                    node: id.0.to_string(),
                    trigger: "close".into(),
                    rubric_version: RUBRIC_VERSION,
                    dimension: dimension.into(),
                    grade,
                    fact,
                });
            }
            let (reason, share) = close_reason(node, outcome, &history);
            commit
                .postmortems
                .push(owed(snapshot, id, "close".into(), reason, share)?);
        }
        Ok(())
    }

    fn history(&mut self, snapshot: &Snapshot, node: NodeId, commit: &Commit) -> Result<History> {
        let id = node.0.to_string();
        let mut rows: std::collections::BTreeMap<(String, String), Value> = snapshot
            .rows
            .iter()
            .map(|row| ((row.table.clone(), row.id.clone()), row.value.clone()))
            .collect();
        for row in &commit.rows {
            match row {
                RowChange::Put { table, id, value } => {
                    rows.insert((table.clone(), id.clone()), value.clone());
                }
                RowChange::Delete { table, id } => {
                    rows.remove(&(table.clone(), id.clone()));
                }
            }
        }
        let mut history = History::default();
        for ((table, row), value) in &rows {
            match table.as_str() {
                "question_context" if value["node"] == json!(node) => history
                    .questions
                    .push(value["kind"].as_str().unwrap_or("unknown").into()),
                "incidents" if value["node"] == json!(node) => history.incidents.push((
                    value["signature"]["class"]
                        .as_str()
                        .unwrap_or("unknown")
                        .into(),
                    value["needs_rob"] == true,
                )),
                "worker_failures" if *row == id => history.run_failure = true,
                "retries" if *row == id => {
                    history.retries = ["run", "effect", "plan"]
                        .iter()
                        .filter_map(|kind| value[kind].as_u64())
                        .sum::<u64>() as u32;
                }
                "queue_facts" => {
                    let Ok(fact) = serde_json::from_value::<crate::adapters::mergify::QueueFact>(
                        value.clone(),
                    ) else {
                        continue;
                    };
                    if fact.node(snapshot)? != Some(node) {
                        continue;
                    }
                    use crate::adapters::mergify::Evidence;
                    match fact.evidence {
                        Evidence::CiFailed {
                            head_check: false, ..
                        }
                        | Evidence::Conflict { .. }
                        | Evidence::DequeuedByCommand { .. }
                        | Evidence::DequeuedByPerson { .. } => history.dequeues += 1,
                        Evidence::Merged {
                            checks_passed: false,
                            ..
                        } => history.escaped_ci = true,
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        // The phase that ends at close is open-ended here, so it is left out and
        // grades depend only on stored times.
        for pair in self.store.phase_history(&id)?.windows(2) {
            let ((phase, since), (_, until)) = (&pair[0], &pair[1]);
            history
                .phases
                .push((phase_name(phase), u64::try_from(until - since).unwrap_or(0)));
        }
        Ok(history)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tl3_kernel::RepoId;

    fn clean() -> Node {
        let mut node = Node::new(RepoId(0));
        node.attempt = 1;
        node
    }

    fn reason(node: &Node, outcome: Outcome, history: &History) -> Option<Reason> {
        Some(close_reason(node, outcome, history).0)
    }

    #[test]
    fn a_stop_for_tl3_review_owes_needed_fix() {
        // Q54 (#440): a planner cant_meet stop has a plan retry and no fix
        // round, and a maker cant_meet stop may have neither.
        let retried = History {
            retries: 1,
            ..History::default()
        };
        for step in [
            tl3_kernel::ReviewStop::PlanCantMeet,
            tl3_kernel::ReviewStop::MakerCantMeet,
        ] {
            let mut stopped = clean();
            stopped.plan_retries = 1;
            stopped.review_stop = Some(step);
            assert_eq!(
                reason(&stopped, Outcome::Stopped, &retried),
                Some(Reason::NeededFix),
                "{step:?}"
            );
        }
    }

    #[test]
    fn close_reasons_preserve_history() {
        let quiet = History::default();
        assert_eq!(
            reason(&clean(), Outcome::Merged, &quiet),
            Some(Reason::Sampled)
        );
        let mut fixed = clean();
        fixed.round = 1;
        assert_eq!(
            reason(&fixed, Outcome::Merged, &quiet),
            Some(Reason::NeededFix)
        );
        let mut later = clean();
        later.attempt = 2;
        assert_eq!(
            reason(&later, Outcome::Merged, &quiet),
            Some(Reason::NeededFix)
        );
        let mut replanned = clean();
        replanned.replanned = true;
        assert_eq!(
            reason(&replanned, Outcome::Merged, &quiet),
            Some(Reason::NeededFix)
        );
        let retried = History {
            run_failure: true,
            ..History::default()
        };
        assert_eq!(
            reason(&clean(), Outcome::Merged, &retried),
            Some(Reason::Retry)
        );
        // The kernel zeroes its failure counters once a retry succeeds.
        let recovered = History {
            retries: 1,
            ..History::default()
        };
        assert_eq!(
            reason(&clean(), Outcome::Merged, &recovered),
            Some(Reason::Retry),
            "a retry that succeeded still needed a retry"
        );
        assert!(
            graded(&clean(), Outcome::Merged, &recovered).contains(&("reliability", Some(4))),
            "a retried merge is not a first-time merge"
        );
        let incident = History {
            incidents: vec![("transport".into(), false)],
            ..History::default()
        };
        assert_eq!(
            reason(&clean(), Outcome::Merged, &incident),
            Some(Reason::Retry)
        );
        let asked = History {
            questions: vec!["cant_meet".into()],
            ..History::default()
        };
        assert_eq!(
            reason(&fixed, Outcome::Merged, &asked),
            Some(Reason::Escalation),
            "escalation outranks the fix it led to"
        );
        let needed_rob = History {
            incidents: vec![("permission".into(), true)],
            ..History::default()
        };
        assert_eq!(
            reason(&clean(), Outcome::Merged, &needed_rob),
            Some(Reason::Escalation)
        );
        for outcome in [
            Outcome::Stopped,
            Outcome::Replaced,
            Outcome::Duplicate,
            Outcome::AlreadyDone,
            Outcome::NotActionable,
        ] {
            assert_eq!(
                reason(&clean(), outcome, &quiet),
                Some(Reason::ClosedWithoutMerge),
                "{outcome:?}: every node that closes without merging gets one"
            );
        }
    }

    #[test]
    fn every_clean_merge_owes_a_postmortem() {
        assert_eq!(
            close_reason(&clean(), Outcome::Merged, &History::default()),
            (Reason::Sampled, Some(1.0))
        );
    }

    fn graded(
        node: &Node,
        outcome: Outcome,
        history: &History,
    ) -> Vec<(&'static str, Option<i16>)> {
        close_grades(node, outcome, history)
            .into_iter()
            .map(|(dimension, grade, _)| (dimension, grade))
            .collect()
    }

    #[test]
    fn code_grades_record_the_facts_at_close() {
        let quiet = History {
            phases: vec![("Making".into(), 90), ("Checking".into(), 30)],
            ..History::default()
        };
        let grades = graded(&clean(), Outcome::Merged, &quiet);
        for expected in [
            ("reliability", Some(5)),
            ("escalation", Some(5)),
            ("queue", Some(5)),
            ("credit", None),
            ("questions", None),
            ("incidents", None),
            ("phase_time", None),
        ] {
            assert!(grades.contains(&expected), "{expected:?} in {grades:?}");
        }
        let facts = close_grades(&clean(), Outcome::Merged, &quiet);
        let phases = &facts.iter().find(|g| g.0 == "phase_time").unwrap().2;
        assert_eq!(phases["Making"], 90);
        let mut troubled = clean();
        troubled.attempt = 2;
        troubled.round = 1;
        let history = History {
            questions: vec!["cant_meet".into(), "out_of_credit".into()],
            incidents: vec![("permission".into(), true), ("transport".into(), false)],
            dequeues: 1,
            ..History::default()
        };
        let grades = graded(&troubled, Outcome::Merged, &history);
        assert!(grades.contains(&("reliability", Some(3))), "{grades:?}");
        assert!(grades.contains(&("escalation", Some(2))), "{grades:?}");
        assert!(grades.contains(&("queue", Some(4))), "{grades:?}");
        let escaped = History {
            escaped_ci: true,
            ..History::default()
        };
        assert!(graded(&clean(), Outcome::Merged, &escaped).contains(&("queue", Some(1))));
        let stopped = graded(&clean(), Outcome::Stopped, &History::default());
        assert!(stopped.contains(&("reliability", Some(1))), "{stopped:?}");
        assert!(
            stopped.contains(&("queue", None)),
            "an unmerged node has no queue grade"
        );
    }

    #[test]
    fn fingerprints_are_stable_across_spacing_and_case_of_the_target() {
        let one = fingerprint(
            "maker",
            "replace",
            "Run the tests.",
            "Run  the tests\nfirst.",
        );
        assert!(one.starts_with("rec-"), "{one}");
        assert_eq!(one.len(), 4 + 16, "{one}");
        assert!(
            one[4..]
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        );
        assert_eq!(
            one,
            fingerprint(
                " Maker ",
                "replace",
                " Run the tests. ",
                "Run the tests first."
            )
        );
        assert_ne!(
            one,
            fingerprint("maker", "replace", "Run the tests.", "Run the tests later.")
        );
        assert_ne!(one, fingerprint("maker", "add", "", "Run the tests first."));
        assert_ne!(
            fingerprint("ab", "add", "", "c"),
            fingerprint("a", "add", "", "bc"),
            "fields are delimited"
        );
    }

    fn claimed() -> Claimed {
        Claimed {
            id: 7,
            node: "3".into(),
            repo: "RobTand/tl3-sandbox".into(),
            issue: 42,
            title: "Fix the sandbox build".into(),
        }
    }

    fn recommendation(n: usize) -> Value {
        json!({"phase": "maker", "kind": "prompt", "target": "maker", "operation": "add",
            "before": "", "after": format!("Run check {n}."), "evidence": "The review.",
            "effect": "Fewer fix rounds."})
    }

    fn with_recommendations(count: usize) -> Value {
        let mut reply = simulated_reply();
        reply["postmortem"]["recommendations"] = (0..count).map(recommendation).collect();
        reply
    }

    #[test]
    fn a_fourth_recommendation_is_refused_by_the_schema_and_by_code() {
        check_reply(&with_recommendations(3)).unwrap();
        assert!(check_reply(&with_recommendations(4)).is_err(), "schema");
        assert!(check_reply(&json!({"postmortem": {}})).is_err());
        // Code refuses a fourth even when a reply skipped the schema.
        assert!(
            result(&claimed(), &with_recommendations(4)).is_err(),
            "code"
        );
        result(&claimed(), &with_recommendations(3)).unwrap();
    }

    #[test]
    fn a_result_stores_the_postmortem_its_ranked_recommendations_and_later_grades() {
        let mut reply = with_recommendations(2);
        reply["grades"] = json!([{"dimension": "correctness", "grade": 4,
            "rationale": "The review.", "judge_model": "claude-sonnet-5-5", "assessment_id": "a-9"}]);
        reply["assessment_ids"] = json!(["a-9"]);
        reply["usage"] = json!({"traces": 3, "input_tokens": 1000, "output_tokens": 40,
            "cache_read_input_tokens": 600, "cache_creation_input_tokens": 100});
        let stored = result(&claimed(), &reply).unwrap();
        assert_eq!(stored.cause, "tl3");
        assert_eq!(stored.summary, reply["postmortem"]["summary"]);
        assert_eq!(stored.timeline, reply["postmortem"]["timeline"]);
        assert_eq!(
            stored.process_failures,
            reply["postmortem"]["process_failures"]
        );
        assert_eq!(stored.assessment_ids, json!(["a-9"]));
        assert_eq!(stored.grades[0]["dimension"], "correctness");
        assert_eq!(stored.grades[0]["grade"], 4);
        let ranks: Vec<_> = stored.recommendations.iter().map(|r| r.rank).collect();
        assert_eq!(ranks, [1, 2]);
        let first = &stored.recommendations[0];
        assert_eq!(
            first.fingerprint,
            fingerprint(&first.target, &first.operation, &first.before, &first.after)
        );
        assert_eq!(first.after, "Run check 0.");
        let grades: Vec<_> = stored
            .code_grades
            .iter()
            .map(|g| {
                (
                    g.dimension.as_str(),
                    g.trigger.as_str(),
                    g.node.as_str(),
                    g.grade,
                )
            })
            .collect();
        assert_eq!(
            grades,
            [
                ("tokens", "postmortem:7", "3", None),
                ("cache_rate", "postmortem:7", "3", None)
            ]
        );
        assert_eq!(stored.code_grades[0].fact["input_tokens"], 1000);
        assert_eq!(stored.code_grades[1].fact["rate"], 0.6);
        let (operation, item) = stored.memory.clone().expect("lessons go to Hindsight");
        assert_eq!(operation.len(), 36, "{operation}");
        assert_eq!(
            result(&claimed(), &reply).unwrap().memory.unwrap().0,
            operation,
            "a replay retains the same memory"
        );
        let tags: Vec<_> = item["tags"].as_array().unwrap().iter().collect();
        assert!(tags.contains(&&json!("postmortem")), "{tags:?}");
        assert!(
            tags.contains(&&json!("repo:RobTand/tl3-sandbox")),
            "{tags:?}"
        );
        assert!(
            item["content"]
                .as_str()
                .unwrap()
                .contains("A plan's named checks")
        );
    }

    #[test]
    fn stored_and_retained_text_is_redacted() {
        let planted = [
            "sk-planted403abcdefghijklmnop",
            "ghp_planted403abcdefghijklmnop",
            "0123456789abcdef0123456789abcdef01234567",
        ];
        let text = format!(
            "The key {} with token={} and {}.",
            planted[0], planted[1], planted[2]
        );
        let mut reply = with_recommendations(1);
        for field in ["summary", "went_wrong", "notes"] {
            reply["postmortem"][field] = json!(text);
        }
        reply["postmortem"]["lessons"] = json!([text]);
        reply["postmortem"]["timeline"][0]["event"] = json!(text);
        reply["postmortem"]["recommendations"][0]["after"] = json!(text);
        reply["postmortem"]["recommendations"][0]["evidence"] = json!(text);
        let stored = result(&claimed(), &reply).unwrap();
        let mut everything = format!(
            "{} {} {} {} {}",
            stored.summary, stored.notes, stored.timeline, stored.process_failures, stored.grades
        );
        for recommendation in &stored.recommendations {
            everything.push_str(&serde_json::to_string(recommendation).unwrap());
        }
        everything.push_str(&stored.memory.unwrap().1.to_string());
        for secret in planted {
            assert!(!everything.contains(secret), "{secret} was stored");
        }
    }

    #[test]
    fn the_rubric_names_every_code_dimension() {
        let rubric: Value = serde_json::from_str(include_str!("rubric.json")).unwrap();
        assert_eq!(rubric["version"], RUBRIC_VERSION);
        for (dimension, _, _) in close_grades(&clean(), Outcome::Merged, &History::default()) {
            assert_eq!(
                rubric["dimensions"][dimension]["grader"], "code",
                "{dimension}"
            );
        }
        // #403: the runner's later grades are code dimensions too.
        for dimension in ["tokens", "cache_rate"] {
            assert_eq!(
                rubric["dimensions"][dimension]["grader"], "code",
                "{dimension}"
            );
        }
    }
}
