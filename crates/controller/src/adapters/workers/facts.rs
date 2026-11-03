//! Completion evidence is imported before ingress and before attempt retirement.
use super::process::read;
use crate::{Error, Result};
use std::path::Path;
use tl3_store::{Store, WorkerBoundaryFailure, WorkerRunFacts};

pub(super) fn record(store: &mut Store, key: &str, attempt: &Path) -> Result<()> {
    let number = attempt
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix("attempt-"))
        .and_then(|name| name.parse::<i64>().ok())
        .ok_or(Error::Invalid("worker facts need their attempt"))?;
    if store.worker_facts_recorded(key, number)? {
        return Ok(());
    }
    if let Some(facts) = completed(attempt)? {
        let request = read(&attempt.join("request.json"))?
            .ok_or(Error::Invalid("worker facts need their request"))?;
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
            session_id: None,
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

#[cfg(test)]
mod tests {
    use super::super::process::write;
    use super::*;
    use serde_json::json;

    fn request(path: &Path) {
        write(
            &path.join("request.json"),
            &json!({"operation":{"node":83}, "role":"maker",
            "model":{"name":"fixture-model"}, "trace":{"release":"release-fixture"}}),
        )
        .unwrap();
    }

    #[test]
    fn worker_timestamps_override_runtime_entry_and_effect_acknowledgement() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path();
        request(path);
        write(
            &path.join("worker-started.json"),
            &json!({"started_ms":1_000_000}),
        )
        .unwrap();
        write(
            &path.join("exit.json"),
            &json!({"code":0, "finished_ms":9_666_000}),
        )
        .unwrap();
        write(
            &path.join("facts.json"),
            &json!({"started_ms":1_001_000, "finished_ms":9_665_000,
            "outcome":"succeeded", "model":"fixture-model", "release":"release-fixture",
            "input_tokens":1_600_000,"cached_tokens":1_000_000,"output_tokens":170_000,
            "trace_id":"tr-0f626041aebfeefcdea5b8365b1a3e46","failure":null}),
        )
        .unwrap();
        let facts = completed(path).unwrap().unwrap();
        assert_eq!(facts.finished_ms.unwrap() - facts.started_ms, 8_666_000);
        assert_eq!(facts.input_tokens, Some(1_600_000));
        assert_eq!(
            facts.trace_id.as_deref(),
            Some("tr-0f626041aebfeefcdea5b8365b1a3e46")
        );
    }

    #[test]
    fn a_killed_worker_keeps_checkpointed_tokens_and_the_waitpid_time() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path();
        request(path);
        write(
            &path.join("worker-started.json"),
            &json!({"started_ms":1000}),
        )
        .unwrap();
        write(
            &path.join("exit.json"),
            &json!({"code":-9,"finished_ms":765_000}),
        )
        .unwrap();
        write(
            &path.join("facts.json"),
            &json!({"started_ms":1000,"finished_ms":null,
            "outcome":"running","release":null,"model":"fixture-model","input_tokens":1_600_000,
            "cached_tokens":null,"output_tokens":170_000,"trace_id":null,"failure":null}),
        )
        .unwrap();
        let facts = completed(path).unwrap().unwrap();
        assert_eq!(facts.outcome, "crashed");
        assert_eq!(facts.finished_ms.unwrap() - facts.started_ms, 764_000);
        assert_eq!(
            facts.input_tokens.unwrap() + facts.output_tokens.unwrap(),
            1_770_000
        );
        assert_eq!(facts.failure.unwrap().class, "ProcessExited");
    }

    #[test]
    fn a_dead_supervisor_cannot_invent_finish_or_usage() {
        let root = tempfile::tempdir().unwrap();
        request(root.path());
        write(
            &root.path().join("worker-started.json"),
            &json!({"started_ms":1000}),
        )
        .unwrap();
        let facts = completed(root.path()).unwrap().unwrap();
        assert_eq!(facts.finished_ms, None);
        assert_eq!(facts.input_tokens, None);
        assert_eq!(facts.outcome, "crashed");
        write(
            &root.path().join("stopped.json"),
            &json!({"finished_ms":2000}),
        )
        .unwrap();
        let stopped = completed(root.path()).unwrap().unwrap();
        assert_eq!(stopped.outcome, "stopped");
        assert_eq!(stopped.finished_ms, Some(2000));
    }

    #[test]
    fn old_attempts_do_not_acquire_effect_times_as_worker_measurements() {
        let root = tempfile::tempdir().unwrap();
        request(root.path());
        write(&root.path().join("exit.json"), &json!({"code":0})).unwrap();
        assert!(completed(root.path()).unwrap().is_none());
    }
}
