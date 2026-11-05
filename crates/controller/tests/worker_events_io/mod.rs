//! H16/C4: cdacce8f extraction equivalence, including each DB write crash.
#![allow(dead_code, unused_imports)] // Verbatim functions retain their original interfaces.
use super::super::{evidence, json, reproducing, review};
use super::{at, cause, protocol};
use crate::{
    Error, Result,
    cli::Input,
    executor::{Delivery, Observation, Reconciliation},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, io::SeekFrom, path::PathBuf};
mod current;
mod io;
mod reference;

#[derive(Debug)]
struct ParsedEvent {
    event: tl3_controller_core::worker_events::WorkerEvent,
    value: Value,
    wait: Option<tl3_store::ExternalWait>,
    model: Option<String>,
}

fn delivery(postmortem: bool) -> Delivery {
    let output = if postmortem {
        json!({"Comment":{"node":0,"kind":"Postmortem","seq":1}})
    } else {
        json!({"Effect":{"node":0,"kind":{"StartRun":"Ingest"},"seq":1,"head":null}})
    };
    serde_json::from_value(
        json!({"key":"worker-events","epoch":1,"generation":0,"output":output,
        "head_sha":null,"repository":{"owner":"RobTand","name":"tl3-sandbox","installation":1},
        "issue":42,"pull_request":null,"paths":[],"attempt":1,"round":0,"checks":null}),
    )
    .unwrap()
}
fn outcome(result: Result<Observation>) -> Value {
    match result {
        Ok(v) => json!({"ok":v}),
        Err(e) => json!({"error":e.to_string(),"debug":format!("{e:?}")}),
    }
}
fn parity(bytes: &[u8], postmortem: bool, inputs: &Value) -> Value {
    let d = delivery(postmortem);
    let old = outcome(reference::parse_reply(bytes, &d, inputs));
    let new = outcome(current::parse_reply(bytes, &d, inputs));
    assert_eq!(old, new, "stream={bytes:?}, postmortem={postmortem}");
    old
}
fn runner(cases: u32) -> proptest::test_runner::TestRunner {
    use proptest::test_runner::*;
    TestRunner::new(Config {
        cases,
        rng_seed: RngSeed::Fixed(0xF176_0202_6005),
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/worker_events_io/proptest-regressions.txt"
        )))),
        ..Config::default()
    })
}
fn line(kind: u8, postmortem: bool) -> Vec<u8> {
    let role = if postmortem { "postmortem" } else { "ingest" };
    let value = match kind % 16 {
        0 => json!({"event":"progress","unit":"model_turn"}),
        1 => json!({"event":"progress","unit":"tool_result"}),
        2 => {
            json!({"event":"progress","unit":"external_wait","wait":{"upstream":"prismabuild","key":"a".repeat(64)}})
        }
        3 => json!({"event":"quota","role":role,"reset_ms":42}),
        4 => json!({"event":"quota","role":role,"reset_ms":42,"model":"grade"}),
        5 => json!({"event":"quota","role":role,"reset_ms":0}),
        6 => json!({"event":"quota","role":role,"reset_ms":42,"model":"other"}),
        7 => {
            json!({"event":"reply","role":role,"reply":{"outcome":"cant_meet","reason":"unavailable"}})
        }
        8 => json!({"event":"progress","unit":"model_turn","extra":true}),
        9 => {
            json!({"event":"progress","unit":"external_wait","wait":{"upstream":"other","key":"bad"}})
        }
        10 => return b"{bad json\n".to_vec(),
        11 => {
            return b"{\"event\":\"progress\",\"event\":\"progress\",\"unit\":\"model_turn\"}\n"
                .to_vec();
        }
        12 => json!({"event":"reply","role":"other","reply":{}}),
        13 => json!({"event":"quota","role":role,"reset_ms":"42"}),
        14 => json!(["event", "progress"]),
        _ => {
            json!({"event":"progress","unit":"external_wait","wait":{"upstream":"prismabuild","key":"bad","extra":1}})
        }
    };
    let mut bytes = serde_json::to_vec(&value).unwrap();
    bytes.push(b'\n');
    bytes
}
#[test]
fn parse_reply_streams() {
    use proptest::prelude::*;
    runner(256)
        .run(
            &(
                proptest::collection::vec(any::<u8>(), 0..40),
                any::<bool>(),
                any::<bool>(),
                0usize..20,
            ),
            |(kinds, postmortem, truncate, tail)| {
                let mut bytes = kinds
                    .into_iter()
                    .flat_map(|k| line(k, postmortem))
                    .collect::<Vec<_>>();
                if truncate {
                    bytes.truncate(bytes.len().saturating_sub(tail));
                }
                parity(&bytes, postmortem, &json!({"grade_model":"grade"}));
                Ok(())
            },
        )
        .unwrap();
}
#[test]
fn mandatory_event_shapes() {
    for postmortem in [false, true] {
        for kind in 0..16 {
            parity(
                &line(kind, postmortem),
                postmortem,
                &json!({"grade_model":"grade"}),
            );
        }
    }
    let mut bytes = line(3, false);
    bytes.extend(line(0, false));
    assert_eq!(
        parity(&bytes, false, &Value::Null)["error"],
        "event after quota ending"
    );
}
#[test]
fn mandatory_quota_compatibility_edges() {
    let result = parity(&line(3, true), true, &json!({"grade_model":"grade"}));
    assert!(result["ok"].get("Quota").is_some());
    let mut repeated = line(3, false);
    repeated.extend(line(0, false));
    repeated.extend(line(3, false));
    assert!(
        parity(&repeated, false, &Value::Null)["ok"]
            .get("Quota")
            .is_some()
    );
}
#[test]
fn refresh_schedules() {
    io::schedules();
}
#[test]
fn mandatory_refresh_branches() {
    io::mandatory();
}

fn function<'a>(source: &'a str, name: &str) -> &'a str {
    let at = source.find(&format!("fn {name}(")).unwrap();
    let start = source[..at].rfind('\n').map_or(0, |v| v + 1);
    let prefix = &source[start..at];
    let indent = &prefix[..prefix.len() - prefix.trim_start().len()];
    let end = source[at..].find(&format!("\n{indent}}}\n")).unwrap() + at + indent.len() + 3;
    &source[start..end]
}
#[test]
fn pinned_reference_and_production_copies() {
    use sha2::{Digest, Sha256};
    let base = include_str!("base-source.rs.txt");
    assert_eq!(
        format!("{:x}", Sha256::digest(base)),
        include_str!("base.sha256").trim()
    );
    let production = include_str!("../../src/adapters/workers/source.rs");
    for name in ["refresh", "progress", "external_wait", "parse_reply"] {
        assert_eq!(
            function(base, name),
            function(include_str!("reference.rs"), name)
        );
    }
    for name in ["refresh", "parse_reply"] {
        assert_eq!(
            function(production, name),
            function(include_str!("current.rs"), name)
        );
    }
}
