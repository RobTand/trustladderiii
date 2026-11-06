//! Exact existing worker frames. Outcome/import judging stays at the worker edge.
use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::Value;
use std::collections::BTreeSet;

const TEST: &[u8] = b"TL3_TEST_SUMMARY ";
const SYMPTOM: &[u8] = b"TL3_SYMPTOM_RUN ";
const GENERATOR: &[u8] = b"TL3_GENERATOR_ARTIFACT ";

enum Parsed<'a> {
    Tests {
        summary: &'a [u8],
        symptoms: Vec<&'a [u8]>,
    },
    Generator(&'a [u8]),
}

fn invalid() -> BackendError {
    BackendError::invalid("worker frame structure, linkage or artifact bytes invalid")
}

fn value(bytes: &[u8]) -> Result<Value> {
    let value: Value = serde_json::from_slice(bytes)?;
    if !value.is_object() {
        return Err(invalid());
    }
    Ok(value)
}

fn safe_metadata(value: &Value) -> Result<()> {
    let mut sanitized = value.clone();
    evidence::sanitized_json_using(&mut sanitized, 0, safe_report)?;
    if sanitized != *value {
        return Err(BackendError::invalid(
            "worker metadata requires producer redaction or exceeds text bounds",
        ));
    }
    Ok(())
}

fn summary(bytes: &[u8], symptoms: &[&[u8]]) -> Result<()> {
    let summary = value(bytes)?;
    safe_metadata(&summary)?;
    if !matches!(
        summary["status"].as_str(),
        Some("completed" | "infrastructure_error" | "not_available")
    ) || !summary["note"].is_string()
        || summary["tests"].as_array().is_none_or(|tests| {
            tests.iter().any(|test| {
                !test.is_object()
                    || !test["test"].is_string()
                    || !matches!(
                        test["outcome"].as_str(),
                        Some("passed" | "failed" | "error" | "skipped")
                    )
            })
        })
    {
        return Err(invalid());
    }
    let totals = summary["totals"].as_object().ok_or_else(invalid)?;
    if totals.len() != 4
        || ["passed", "failed", "error", "skipped"]
            .iter()
            .any(|key| totals.get(*key).is_none_or(|v| v.as_u64().is_none()))
    {
        return Err(invalid());
    }
    let declared = summary
        .get("declared_symptom")
        .is_some_and(|v| !v.is_null());
    let entries = summary
        .get("symptom_runs")
        .map(|v| v.as_array().ok_or_else(invalid))
        .transpose()?;
    if !declared && (!symptoms.is_empty() || entries.is_some_and(|entries| !entries.is_empty())) {
        return Err(invalid());
    }
    if declared {
        let _: crate::contracts::checks::DeclaredSymptom =
            serde_json::from_value(summary["declared_symptom"].clone())?;
        let entries = entries.ok_or_else(invalid)?;
        if symptoms.len() > 10 || entries.len() != symptoms.len() {
            return Err(invalid());
        }
        for (index, (entry, bytes)) in entries.iter().zip(symptoms).enumerate() {
            let mut receipt = value(bytes)?;
            safe_metadata(&receipt)?;
            if receipt["run"].as_u64() != Some(index as u64 + 1)
                || receipt["sha256"].as_str().is_none_or(|hash| !hex(hash, 64))
                || !receipt["records"].is_array()
            {
                return Err(invalid());
            }
            receipt
                .as_object_mut()
                .ok_or_else(invalid)?
                .remove("records");
            if receipt != *entry {
                return Err(invalid());
            }
        }
    }
    // Existing worker validation recomputes Python's canonical symptom-record hash
    // and classifies the declared symptom. This transport preserves every byte.
    Ok(())
}

fn artifact(bytes: &[u8], suite: &Suite) -> Result<()> {
    let mut metadata = value(bytes)?;
    if !matches!(
        metadata["status"].as_str(),
        Some("completed" | "failed" | "refused")
    ) || !metadata["message"].is_string()
        || metadata.get("exit_code").is_none()
        || !(metadata["exit_code"].is_null() || metadata["exit_code"].as_i64().is_some())
    {
        return Err(invalid());
    }
    let bounds = output_bounds(suite);
    let outputs = metadata
        .as_object_mut()
        .ok_or_else(invalid)?
        .remove("outputs");
    safe_metadata(&metadata)?;
    if metadata["status"] == "completed"
        && (metadata["input_manifest"]
            .as_str()
            .is_none_or(|v| !hex(v, 64))
            || metadata["output_manifest"]
                .as_str()
                .is_none_or(|v| !hex(v, 64))
            || outputs.is_none())
    {
        return Err(invalid());
    }
    let Some(outputs) = outputs else {
        return Ok(());
    };
    let outputs = outputs.as_array().ok_or_else(invalid)?;
    if outputs.len() > bounds.artifact_files {
        return Err(invalid());
    }
    let mut total = 0usize;
    let mut names = BTreeSet::new();
    for output in outputs {
        let name = output["path"].as_str().ok_or_else(invalid)?;
        if name.is_empty()
            || name.len() > 3072
            || name.starts_with('/')
            || name.contains('\\')
            || name.chars().any(char::is_control)
            || name.split('/').any(|part| {
                part.is_empty() || matches!(part, "." | ".." | ".git") || part.len() > 255
            })
            || name.rsplit('/').next() == Some(".tl3-generator-import")
            || !names.insert(name)
        {
            return Err(invalid());
        }
        let encoded = output["data"].as_str().ok_or_else(invalid)?;
        if encoded.len() > bounds.artifact_file_bytes.div_ceil(3) * 4 {
            return Err(invalid());
        }
        let raw = STANDARD.decode(encoded).map_err(|_| invalid())?;
        total = total.checked_add(raw.len()).ok_or_else(invalid)?;
        if raw.len() > bounds.artifact_file_bytes
            || total > bounds.decoded_artifact_bytes
            || std::str::from_utf8(&raw).is_err()
            || raw.iter().any(|b| *b < 32 && !matches!(*b, 9 | 10 | 13))
            || output["entry"]["sha256"].as_str() != Some(digest(&raw).as_str())
            || output["entry"]["bytes"].as_u64() != Some(raw.len() as u64)
            || !matches!(output["entry"]["mode"].as_u64(), Some(0o644 | 0o755))
        {
            return Err(invalid());
        }
        let mut item_metadata = output.clone();
        item_metadata
            .as_object_mut()
            .ok_or_else(invalid)?
            .remove("data");
        safe_metadata(&item_metadata)?;
    }
    Ok(())
}

fn parse<'a>(raw: &'a [u8], suite: &Suite) -> Result<Parsed<'a>> {
    std::str::from_utf8(raw).map_err(|_| invalid())?;
    let bounds = output_bounds(suite);
    if raw.len() > bounds.stdout_bytes {
        return Err(invalid());
    }
    let mut tests = Vec::new();
    let mut symptoms = Vec::new();
    let mut artifacts = Vec::new();
    for line in raw.split(|byte| *byte == b'\n') {
        if let Some(frame) = line.strip_prefix(TEST) {
            tests.push(frame);
        }
        if let Some(frame) = line.strip_prefix(SYMPTOM) {
            symptoms.push(frame);
        }
        if let Some(frame) = line.strip_prefix(GENERATOR) {
            artifacts.push(frame);
        }
    }
    match suite {
        Suite::WorkerTests { .. } if tests.len() == 1 && artifacts.is_empty() => {
            summary(tests[0], &symptoms)?;
            Ok(Parsed::Tests {
                summary: tests[0],
                symptoms,
            })
        }
        Suite::Generator { .. }
            if artifacts.len() == 1 && tests.is_empty() && symptoms.is_empty() =>
        {
            if artifacts[0].len() > bounds.payload_bytes {
                return Err(invalid());
            }
            artifact(artifacts[0], suite)?;
            Ok(Parsed::Generator(artifacts[0]))
        }
        _ => Err(invalid()),
    }
}

pub(super) fn report_identity(record: &Record, report: &RunnerReport) -> Result<bool> {
    if report.outcome != ReportOutcome::WorkerPayloadCompleted
        || report.payload.is_some()
        || !report.cases.is_empty()
        || report.tests_started != 0
    {
        return Ok(false);
    }
    let bounds = output_bounds(&record.request.suite);
    let Some(stdout) = &report.stdout else {
        return Ok(false);
    };
    let raw = checked_blob_limit(stdout, &record.intent.output_dir, bounds.stdout_bytes)?;
    match (parse(&raw, &record.request.suite)?, &report.worker_payload) {
        (
            Parsed::Tests { summary, symptoms },
            Some(WorkerPayload::Tests {
                summary: retained,
                symptom_runs,
            }),
        ) => {
            if checked_blob_limit(retained, &record.intent.output_dir, bounds.payload_bytes)?
                != summary
                || symptom_runs.len() != symptoms.len()
            {
                return Ok(false);
            }
            for (reference, expected) in symptom_runs.iter().zip(symptoms) {
                if checked_blob_limit(reference, &record.intent.output_dir, bounds.payload_bytes)?
                    != expected
                {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        (Parsed::Generator(bytes), Some(WorkerPayload::Generator { artifact })) => Ok(
            checked_blob_limit(artifact, &record.intent.output_dir, bounds.payload_bytes)? == bytes,
        ),
        _ => Ok(false),
    }
}

impl Flux {
    pub fn collect_worker(
        &self,
        owner: &Owner,
        input: &CollectorInput,
    ) -> Result<WorkerCollection> {
        let record = self.journal.load(owner)?;
        self.instance(&record)?;
        validate::trusted_artifacts(&record.request)?;
        input.run.validate()?;
        if record.terminal.is_some()
            || !record.dispatched
            || record
                .binding
                .as_ref()
                .is_none_or(|binding| binding.id != input.run)
        {
            return Err(BackendError::invalid(
                "worker collector requires exact durable live binding",
            ));
        }
        let bounds = output_bounds(&record.request.suite);
        let out = &record.intent.output_dir;
        let raw = checked_blob_limit(&input.stdout, out, bounds.stdout_bytes)?;
        if let Some(stderr) = &input.stderr {
            checked_blob_limit(stderr, out, bounds.stderr_bytes)?;
        }
        let payload = match parse(&raw, &record.request.suite)? {
            Parsed::Tests { summary, symptoms } => WorkerPayload::Tests {
                summary: blob(&out.join("worker-summary.json"), summary)?,
                symptom_runs: symptoms
                    .iter()
                    .enumerate()
                    .map(|(index, bytes)| {
                        blob(&out.join(format!("worker-symptom-{index}.json")), bytes)
                    })
                    .collect::<Result<_>>()?,
            },
            Parsed::Generator(bytes) => WorkerPayload::Generator {
                artifact: blob(&out.join("worker-artifact.json"), bytes)?,
            },
        };
        if let Some(reference) = &input.cleanup {
            self.retain_cleanup(owner, reference)?;
        }
        let latest = self.journal.load(owner)?;
        let report = RunnerReport {
            schema: "tl3.runner_report.v1".into(),
            run: input.run.clone(),
            request_digest: record.intent.request_digest,
            tree: record.request.tree.clone(),
            runner_digest: record.request.runner.executable_digest.clone(),
            runtime_digest: record.request.runtime.manifest_digest.clone(),
            suite: record.request.suite.clone(),
            complete: true,
            cleanup_complete: cleanup::all_stopped(&latest),
            tests_started: 0,
            outcome: ReportOutcome::WorkerPayloadCompleted,
            cases: vec![],
            payload: None,
            runtime_binding: input.runtime_binding.clone(),
            runtime_bindings: input.runtime_bindings.clone(),
            cleanup: input.cleanup.clone(),
            stdout: Some(input.stdout.clone()),
            stderr: input.stderr.clone(),
            worker_payload: Some(payload.clone()),
        };
        let binding = latest.binding.as_ref().ok_or_else(invalid)?;
        if !self.report_identity(&latest, binding, &report)? {
            return Err(invalid());
        }
        let report = blob(&out.join("report.json"), &serde_json::to_vec(&report)?)?;
        Ok(WorkerCollection { report, payload })
    }

    pub(super) fn verified_worker_payload(
        &self,
        owner: &Owner,
        terminal: &Terminal,
    ) -> Result<WorkerPayload> {
        let report = self.verified_report(owner, terminal)?;
        if !matches!(report.suite, Suite::WorkerTests { .. } | Suite::Generator { .. })
            || report.outcome != ReportOutcome::WorkerPayloadCompleted
        {
            return Err(invalid());
        }
        report.worker_payload.ok_or_else(|| BackendError::invalid("verified worker payload absent"))
    }
}
