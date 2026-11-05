//! Q53 / #759: exception facts are sampled in Python, decisions belong to Rust.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use tl3_controller_core::worker_events::{self as decision, QuotaFacts, QuotaSample};

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Link {
    pub response_status: Option<i64>,
    pub headers: BTreeMap<String, Value>,
    pub api_status: Option<i64>,
    pub api_code: Option<String>,
    pub judge: bool,
    pub judge_status: Option<i64>,
    pub judge_code: Option<String>,
    pub grade_judge: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Facts {
    pub now: String,
    pub role: String,
    pub links: Vec<Link>,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QuotaEvent {
    pub event: String,
    pub role: String,
    pub facts: Facts,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

mod parse;

pub fn decide(facts: &Facts) -> Option<u64> {
    evaluated(facts).map(|(reset, _)| reset)
}

fn evaluated(facts: &Facts) -> Option<(u64, QuotaFacts)> {
    if facts.links.len() > 8 {
        return None;
    }
    let now = parse::python_float(&facts.now).filter(|now| now.is_finite())?;
    let sample = QuotaSample {
        bits: now.to_bits(),
        short_ms: parse::ceil(now + 120.0),
        long_ms: parse::ceil(now + 900.0),
    };
    for link in &facts.links {
        let announced = parse::announced(&link.headers, now);
        let milliseconds = announced.and_then(parse::milliseconds);
        let (status, code) = if facts.role == "postmortem" && link.judge {
            (link.judge_status, link.judge_code.as_deref())
        } else {
            (link.api_status, link.api_code.as_deref())
        };
        let normalized = QuotaFacts {
            sampled_now: sample,
            response_status: link.response_status.unwrap_or(0),
            finite: announced.is_some_and(f64::is_finite),
            future: announced.is_some_and(|value| value > now),
            below_limit: milliseconds.is_some(),
            whole: milliseconds.map_or(0, |value| value.0),
            fraction: milliseconds.is_some_and(|value| value.1),
            stale: link.response_status == Some(429)
                && announced.is_some_and(|value| value.is_finite() && value <= now),
            status,
            unroutable: code == Some("model_not_found"),
        };
        if let Some(reset) = decision::quota_reset(normalized) {
            return Some((reset, normalized));
        }
    }
    None
}

/// Decode the raw wire into F1's existing event type. Historical completed
/// attempts still contain reset_ms; their model and ordering rules stay intact.
pub(crate) fn decode_event(
    value: &Value,
    postmortem: bool,
    inputs: &Value,
) -> decision::WorkerEvent {
    let field = match value.get("model") {
        None => decision::ModelField::Missing,
        Some(name) if name.is_string() && *name == inputs["grade_model"] => {
            decision::ModelField::GradeModel
        }
        Some(_) => decision::ModelField::Other,
    };
    let model = decision::quota_model(postmortem, field);
    if value.get("facts").is_none() {
        return decision::WorkerEvent::Quota {
            model,
            reset_ms: value["reset_ms"].as_u64(),
            facts: None,
        };
    }
    let Ok(event) = serde_json::from_value::<QuotaEvent>(value.clone()) else {
        return decision::WorkerEvent::Invalid;
    };
    if event.facts.role != event.role || event.facts.links.len() > 8 {
        return decision::WorkerEvent::Invalid;
    }
    let evaluated = evaluated(&event.facts);
    decision::WorkerEvent::Quota {
        model,
        reset_ms: evaluated.map(|value| value.0),
        facts: evaluated.map(|value| value.1),
    }
}

pub fn reset(link: &Link, now: &str) -> Option<u64> {
    let now = parse::python_float(now)?;
    let seconds = parse::announced(&link.headers, now)?;
    let (whole, fraction) = parse::milliseconds(seconds)?;
    decision::announced(
        link.response_status.unwrap_or(0),
        seconds.is_finite(),
        seconds > now,
        true,
        whole,
        fraction,
    )
}

pub fn grade_judge(facts: &Facts) -> bool {
    facts.role == "postmortem" && facts.links.iter().any(|link| link.grade_judge)
}

/// The exception edge asks the same Rust decision whether the run ends as quota.
/// The source subsequently recomputes it from raw facts, never its own clock.
pub fn check() -> crate::Result<()> {
    use std::io::{Read, Write};
    if std::env::args().nth(2).as_deref() == Some("schema") {
        serde_json::to_writer_pretty(std::io::stdout().lock(), &schemars::schema_for!(QuotaEvent))?;
        std::io::stdout().write_all(b"\n")?;
        return Ok(());
    }
    let mut bytes = Vec::new();
    std::io::stdin().read_to_end(&mut bytes)?;
    let value = crate::adapters::workers::json::parse(&bytes)?;
    let facts: Facts = serde_json::from_value(value["facts"].clone())?;
    let result = if value["kind"] == "reset" {
        facts.links.first().and_then(|link| reset(link, &facts.now))
    } else {
        decide(&facts)
    };
    serde_json::to_writer(
        std::io::stdout().lock(),
        &serde_json::json!({"reset_ms": result, "grade_judge": grade_judge(&facts)}),
    )?;
    std::io::stdout().write_all(b"\n")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quota_matches_python_shared_vectors() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let output = std::process::Command::new(root.join("workers/.venv/bin/python"))
            .arg(root.join("workers/tests/p5/quota_vectors.py"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let rows: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
        let committed: Vec<Value> = serde_json::from_slice(
            &std::fs::read(root.join("workers/tests/p5/quota-vectors.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            rows, committed,
            "Python quota vectors drifted; regenerate the corpus"
        );
        assert!(rows.len() > 100);
        for row in rows {
            let facts: Facts = serde_json::from_value(row["facts"].clone()).unwrap();
            assert_eq!(serde_json::json!(decide(&facts)), row["expected"], "{row}");
            if row.get("grade").is_some() && !row["expected"].is_null() {
                assert_eq!(serde_json::json!(grade_judge(&facts)), row["grade"]);
            }
        }
    }
}

#[cfg(test)]
mod parser_properties {
    use super::*;
    use proptest::{prelude::*, strategy::ValueTree, test_runner::TestRunner};
    #[test]
    fn parsed_headers_match_python_float_and_http_dates() {
        let mut runner = TestRunner::deterministic();
        let strategy = (
            0u64..1_000_000_000,
            any::<i64>(),
            0u32..9,
            any::<bool>(),
            0u8..4,
        );
        let mut rows = Vec::new();
        let mut actual = Vec::new();
        for _ in 0..256 {
            let (now, numerator, exponent, relative, shape) =
                strategy.new_tree(&mut runner).unwrap().current();
            let value = match shape {
                0 => "Thu, 01 Jan 1970 00:16:48 GMT".to_owned(),
                1 => "Thursday, 01-Jan-70 00:16:48 GMT".to_owned(),
                2 => format!("{numerator}_0e-{exponent}"),
                _ => format!(" {numerator}e-{exponent} "),
            };
            let header = if relative {
                "retry-after"
            } else {
                "x-ratelimit-reset"
            };
            let headers = BTreeMap::from([(header.to_owned(), Value::String(value))]);
            let link = Link {
                response_status: Some(429),
                headers: headers.clone(),
                api_status: Some(429),
                api_code: None,
                judge: false,
                judge_status: None,
                judge_code: None,
                grade_judge: false,
            };
            actual.push(reset(&link, &now.to_string()));
            rows.push(serde_json::json!({"now": now.to_string(), "headers": headers}));
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        use std::io::Write;
        let mut child = std::process::Command::new(root.join("workers/.venv/bin/python"))
            .arg(root.join("workers/tests/p5/parser_oracle.py"))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(serde_json::to_string(&rows).unwrap().as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let expected: Vec<Option<u64>> = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(actual, expected);
    }
}

#[cfg(test)]
mod schema_drift {
    #[test]
    fn quota_event_schema_matches_rust_type() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let committed: serde_json::Value = serde_json::from_slice(
            &std::fs::read(root.join("workers/src/tl3_workers/schemas/quota-event.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(schemars::schema_for!(super::QuotaEvent)).unwrap(),
            committed
        );
    }
}

#[cfg(test)]
mod io_schedules {
    use proptest::{prelude::*, strategy::ValueTree, test_runner::TestRunner};
    #[test]
    fn quota_extraction_preserves_worker_clock_sample() {
        let mut runner = TestRunner::deterministic();
        let strategy = (-1i32..2, 0u32..1000);
        let mut rows = Vec::new();
        // Exact witness: sampled 1000.000, reset 1000.125, controller processing 1000.250.
        rows.push(serde_json::json!({"sample": 1000.0, "reset": "1000.125", "transport_ms": 250}));
        for _ in 0..18 {
            let (offset, transport_ms) = strategy.new_tree(&mut runner).unwrap().current();
            rows.push(serde_json::json!({"sample": 1000.0, "reset": (1000.0 + offset as f64 / 8.0).to_string(), "transport_ms": transport_ms}));
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        use std::io::Write;
        let mut child = std::process::Command::new(root.join("workers/.venv/bin/python"))
            .arg(root.join("workers/tests/p5/quota_schedules.py"))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(serde_json::to_string(&rows).unwrap().as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("stale-reset boundary"));
    }
}
