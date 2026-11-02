//! Completion evidence is imported before ingress and before attempt retirement.
use super::process::read;
use crate::{Error, Result};
use std::path::Path;
use tl3_store::{Store, WorkerBoundaryFailure, WorkerRunFacts};

pub(super) fn record(store: &mut Store, key: &str, attempt: &Path) -> Result<()> {
    if let Some(facts) = completed(attempt)? {
        let request = read(&attempt.join("request.json"))?
            .ok_or(Error::Invalid("worker facts need their request"))?;
        let number = attempt
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix("attempt-"))
            .and_then(|name| name.parse::<i64>().ok())
            .ok_or(Error::Invalid("worker facts need their attempt"))?;
        let node = request["operation"]["node"]
            .as_u64()
            .ok_or(Error::Invalid("worker facts need their node"))?
            .to_string();
        let role = request["role"]
            .as_str()
            .ok_or(Error::Invalid("worker facts need their role"))?;
        store.record_worker_facts(key, &node, number, role, &facts)?;
    }
    Ok(())
}

fn completed(attempt: &Path) -> Result<Option<WorkerRunFacts>> {
    let path = attempt.join("facts.json");
    if path.try_exists()? && std::fs::metadata(&path)?.len() > 8192 {
        return Err(Error::Invalid("worker facts exceed boundary limit"));
    }
    let measured = read(&path)?;
    let started = read(&attempt.join("worker-started.json"))?;
    let exit = read(&attempt.join("exit.json"))?;
    let stopped = read(&attempt.join("stopped.json"))?;
    // Old releases have no actual worker start. Never substitute an effect's time.
    let Some(started_ms) = started
        .as_ref()
        .and_then(|v| v["started_ms"].as_i64())
        .or_else(|| measured.as_ref().and_then(|v| v["started_ms"].as_i64()))
    else {
        return Ok(None);
    };
    let Some(request) = read(&attempt.join("request.json"))? else {
        return Err(Error::Invalid("worker facts need their request"));
    };
    let mut facts = match measured {
        Some(value) => serde_json::from_value::<WorkerRunFacts>(value)?,
        None => WorkerRunFacts {
            started_ms,
            finished_ms: None,
            outcome: "crashed".into(),
            release: request["trace"]["release"].as_str().map(str::to_owned),
            model: request["model"]["name"]
                .as_str()
                .ok_or(Error::Invalid("worker facts need their model"))?
                .into(),
            input_tokens: None,
            cached_tokens: None,
            output_tokens: None,
            trace_id: None,
            failure: None,
        },
    };
    facts.started_ms = started_ms;
    facts.finished_ms = exit
        .as_ref()
        .and_then(|v| v["finished_ms"].as_i64())
        .or(facts.finished_ms);
    if facts.outcome == "running" || facts.finished_ms.is_none() {
        facts.outcome = "crashed".into();
        facts.failure = Some(WorkerBoundaryFailure {
            origin: "worker".into(),
            class: "ProcessExited".into(),
            status: None,
            request_id: None,
        });
    }
    if stopped.is_some() && exit.is_none() {
        facts.outcome = "stopped".into();
        facts.finished_ms = stopped.and_then(|v| v["finished_ms"].as_i64());
        facts.failure = Some(WorkerBoundaryFailure {
            origin: "worker".into(),
            class: "WorkerStopped".into(),
            status: None,
            request_id: None,
        });
    }
    facts.validate()?;
    Ok(Some(facts))
}
