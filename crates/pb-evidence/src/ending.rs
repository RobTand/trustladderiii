use crate::error::{Refusal, Result, get, parse};
use crate::python_json::{dumps, equal, integer, sha, truth};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const SUMMARY_LIMIT: usize = 1_048_576;
pub const RECORD_LIMIT: usize = 2_097_152;
pub const ARTIFACT_LIMIT: usize = 48 * 1_048_576;
pub const PREFIX: &str = "TL3_TEST_SUMMARY ";

#[derive(Clone, Debug, Default, Serialize, JsonSchema)]
#[serde(transparent)]
pub struct Field(Option<Value>);
impl<'de> Deserialize<'de> for Field {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        Value::deserialize(deserializer).map(|value| Self(Some(value)))
    }
}
impl Field {
    fn required(&self, name: &str) -> Result<&Value> {
        self.0.as_ref().ok_or_else(|| Refusal::key(name))
    }
    fn optional(&self) -> &Value {
        self.0.as_ref().unwrap_or(&Value::Null)
    }
}

/// External PB fields preserve Python scalar distinctions until boundary decoding.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct PbEnding {
    #[serde(default)]
    pub action_key: Field,
    #[serde(default)]
    pub status: Field,
    #[serde(default)]
    pub exit_code: Field,
    #[serde(default)]
    pub succeeded: Field,
    #[serde(default)]
    pub result: Field,
    #[serde(default)]
    pub attempt: Field,
    #[serde(default)]
    pub publication: Field,
    #[serde(default)]
    pub diagnostics: Field,
    #[serde(default)]
    pub reason: Field,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct LogReceipt {
    pub path: Value,
    pub bytes: u64,
    pub sha256: Value,
}

pub fn digest(value: &Value) -> bool {
    value.as_str().is_some_and(|s| {
        s.len() == 64
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

pub fn start(ending: &PbEnding, key: &str, generator: bool) -> Result<LogReceipt> {
    let mut cause = "invalid_ending";
    let decide = || -> Result<LogReceipt> {
        let key_matches = ending.action_key.required("action_key")? == key;
        let status_matches = !key_matches
            || matches!(
                ending.status.required("status")?.as_str(),
                Some("executed" | "cache_hit")
            );
        let exit_matches = !(key_matches && status_matches)
            || equal(ending.exit_code.required("exit_code")?, &json!(0));
        let succeeded = !(key_matches && status_matches && exit_matches)
            || if generator {
                ending.succeeded.required("succeeded")? == &Value::Bool(true)
            } else {
                truth(ending.succeeded.required("succeeded")?)
            };
        let accepted =
            crate::decision::header(key_matches, status_matches, exit_matches, succeeded);
        if crate::decision::ending(accepted, true, true) != 0 {
            return Err(Refusal::value(if generator {
                "Generator action did not succeed; inspect its PrismaBuild terminal record."
            } else {
                "PrismaBuild action did not succeed"
            }));
        }
        cause = "unverified_receipt";
        let evidence = ending.result.required("result")?;
        let verified_flag = get(evidence, "verified")? == &Value::Bool(true);
        let receipt_digest = !verified_flag || digest(get(evidence, "receipt_sha256")?);
        let verified = crate::decision::receipt(verified_flag, receipt_digest);
        if crate::decision::ending(true, verified, true) != 0 {
            return Err(Refusal::value(if generator {
                "Generator action has no verified PrismaBuild receipt."
            } else {
                "PrismaBuild result lacks verified receipt evidence"
            }));
        }
        cause = "invalid_log_receipt";
        let log = get(evidence, "stdout")?;
        let count = integer(get(log, "bytes")?);
        let valid = count.is_some_and(|n| {
            n <= if generator {
                ARTIFACT_LIMIT as u64
            } else {
                SUMMARY_LIMIT as u64
            } && (!generator || n > 0)
        });
        if crate::decision::ending(true, true, valid) != 0 {
            return Err(Refusal::value(if generator {
                "Generator artifact exceeds its limit."
            } else {
                "PrismaBuild result log exceeded its limit"
            }));
        }
        Ok(LogReceipt {
            path: get(log, "path")?.clone(),
            bytes: count.unwrap_or(0),
            sha256: log.get("sha256").cloned().unwrap_or(Value::Null),
        })
    };
    let mut decide = decide;
    decide().map_err(|error| {
        if generator || error.kind == "AttributeError" {
            error
        } else {
            Refusal::transport("summary", cause, Some(error))
        }
    })
}

fn metadata(ending: &PbEnding, receipt: &mut Value) {
    if let Some(attempt) = integer(ending.attempt.optional()).filter(|n| *n > 0) {
        receipt["attempt"] = json!(attempt);
    }
    let generation = &ending.publication.optional()["generation"];
    if digest(generation) {
        receipt["generation"] = generation.clone();
    }
}

fn lines(raw: &str) -> impl Iterator<Item = &str> {
    // str.splitlines handles the full set of Python Unicode line boundaries.
    raw.split([
        '\n', '\r', '\u{b}', '\u{c}', '\u{1c}', '\u{1d}', '\u{1e}', '\u{85}', '\u{2028}',
        '\u{2029}',
    ])
}
fn summary_shape(value: &Value) -> bool {
    let outcomes = ["passed", "failed", "error", "skipped"];
    crate::decision::shape(
        value.is_object(),
        matches!(
            value["status"].as_str(),
            Some("completed" | "infrastructure_error" | "not_available")
        ),
        value["note"].is_string(),
        value["totals"].as_object().is_some_and(|totals| {
            totals.len() == 4
                && outcomes
                    .iter()
                    .all(|o| totals.get(*o).is_some_and(|n| integer(n).is_some()))
        }),
        value["tests"].as_array().is_some_and(|tests| {
            tests.iter().all(|t| {
                t.is_object()
                    && t["test"].is_string()
                    && t["outcome"].as_str().is_some_and(|o| outcomes.contains(&o))
            })
        }),
    )
}

pub fn finish(ending: &PbEnding, key: &str, raw: &[u8], generator: bool) -> Result<Value> {
    let log = start(ending, key, generator)?;
    let mut cause = "invalid_log_receipt";
    let mut decide = || -> Result<Value> {
        let length_matches = raw.len() as u64 == log.bytes;
        let digest_matches = !length_matches
            || *get(get(ending.result.required("result")?, "stdout")?, "sha256")? == sha(raw);
        if !crate::decision::log(length_matches, digest_matches) {
            return Err(Refusal::value(if generator {
                "Generator artifact digest mismatch."
            } else {
                "PrismaBuild result log changed"
            }));
        }
        cause = "missing_unique_summary";
        let text = std::str::from_utf8(raw).map_err(|e| Refusal::utf8(raw, e))?;
        let prefix = if generator {
            "TL3_GENERATOR_ARTIFACT "
        } else {
            PREFIX
        };
        let records: Vec<_> = lines(text)
            .filter_map(|line| line.strip_prefix(prefix))
            .collect();
        if !crate::decision::unique(records.len() as u64) {
            return Err(Refusal::value(if generator {
                "Generator action returned no unique complete artifact."
            } else {
                "PrismaBuild returned no unique test summary"
            }));
        }
        cause = "invalid_summary";
        let mut result = parse(records[0])?;
        if generator {
            let mut receipt = json!({"action_key":key,"artifact_sha256":log.sha256,"receipt_sha256":ending.result.optional()["receipt_sha256"],
                "exit_code":get(&result,"exit_code")?,"inner":{"status":get(&result,"status")?,"exit_code":get(&result,"exit_code")?},
                "outer":{"status":ending.status.optional(),"exit_code":ending.exit_code.optional(),"elapsed_s":elapsed(ending)}});
            for key in ["command", "argv", "identity", "budget", "phase", "timings"] {
                receipt[key] = result[key].clone();
            }
            return Ok(json!([result, receipt]));
        }
        if !result.is_object() {
            return Err(Refusal::typed(
                "AttributeError",
                format!(
                    "'{}' object has no attribute 'get'",
                    crate::error::python_type(&result)
                ),
            ));
        }
        if !result["declared_symptom"].is_null() {
            let records: Vec<_> = lines(text)
                .filter_map(|line| line.strip_prefix("TL3_SYMPTOM_RUN "))
                .map(parse)
                .collect::<Result<_>>()?;
            let summaries = result
                .get("symptom_runs")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if records.len() != summaries.len() {
                return Err(Refusal::value("missing symptom run receipt"));
            }
            for (index, (record, summary)) in records.iter().zip(summaries).enumerate() {
                let mut without = record.clone();
                if let Some(fields) = without.as_object_mut() {
                    fields.remove("records");
                }
                let sequence = equal(get(record, "run")?, &json!(index + 1));
                let digest = !sequence
                    || *get(record, "sha256")?
                        == sha(dumps(get(record, "records")?, false, true).as_bytes());
                let summary = !(sequence && digest) || equal(&summary, &without);
                if !crate::decision::symptom(sequence, digest, summary) {
                    return Err(Refusal::value("symptom run receipt changed"));
                }
            }
        }
        if !summary_shape(&result) {
            return Err(Refusal::value("Invalid summary structure"));
        }
        result["action_key"] = json!(key);
        let mut receipt = json!({"action_key":key,"status":ending.status.optional(),"exit_code":ending.exit_code.optional(),
            "receipt_sha256":ending.result.optional()["receipt_sha256"],"log_sha256":log.sha256});
        metadata(ending, &mut receipt);
        result["receipts"] = json!([receipt]);
        Ok(result)
    };
    decide().map_err(|error| {
        if generator || error.kind == "AttributeError" {
            error
        } else {
            Refusal::transport("summary", cause, Some(error))
        }
    })
}

fn elapsed(ending: &PbEnding) -> Value {
    let value = &ending.diagnostics.optional()["elapsed_s"];
    if value.is_number() && !value.to_string().starts_with('-') {
        value.clone()
    } else {
        Value::Null
    }
}

pub fn failure_read(ending: &PbEnding) -> bool {
    let diagnostics = ending.diagnostics.optional();
    let reason = if truth(&diagnostics["termination_reason"]) {
        &diagnostics["termination_reason"]
    } else {
        ending.reason.optional()
    };
    reason != "memory_limit_oom" && !ending.publication.optional()["published_unix"].is_null()
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FailedRecord {
    pub action_key: Value,
    pub published_unix: Value,
    pub termination_reason: Value,
}

pub fn failure(ending: &PbEnding, record: Option<&FailedRecord>) -> Result<Value> {
    let diagnostics = ending.diagnostics.optional();
    let mut receipt = json!({"action_key":ending.action_key.required("action_key")?,"status":ending.status.required("status")?,
        "exit_code":if ending.exit_code.optional().is_null(){&diagnostics["returncode"]}else{ending.exit_code.optional()}});
    let mut reason = if truth(&diagnostics["termination_reason"]) {
        diagnostics["termination_reason"].clone()
    } else {
        ending.reason.optional().clone()
    };
    if failure_read(ending) {
        if let Some(record) = record {
            if equal(&record.action_key, ending.action_key.optional())
                && equal(
                    &record.published_unix,
                    &ending.publication.optional()["published_unix"],
                )
                && truth(&record.termination_reason)
            {
                reason = record.termination_reason.clone();
            }
        }
    }
    if reason.as_str().is_some_and(|s| {
        !s.is_empty()
            && s.len() <= 128
            && s.as_bytes()[0].is_ascii_lowercase()
            && s.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    }) {
        receipt["termination_reason"] = reason;
    }
    metadata(ending, &mut receipt);
    if let Some(peak) = integer(&diagnostics["resource_profile"]["scope"]["memory_peak_bytes"]) {
        receipt["memory_peak_bytes"] = json!(peak);
    }
    Ok(receipt)
}
