//! H16/C4: cdacce8f extraction equivalence, including each DB write crash.
#![allow(dead_code, unused_imports)] // Verbatim functions retain their original interfaces.
use super::super::{evidence, json, reproducing, review};
use super::{at, cause, protocol};
use crate::{
    Error, Result,
    cli::Input,
    executor::{Delivery, Observation, Reconciliation},
};
use serde_json::Value;
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
    for name in [
        "refresh",
        "parse_event",
        "decode_event",
        "external_wait",
        "parse_reply",
    ] {
        assert_eq!(
            function(production, name),
            function(include_str!("current.rs"), name)
        );
    }
}

/// C4: typed line decoding round-trips every accepted event shape.
#[test]
fn parser_roundtrip() {
    use proptest::prelude::*;
    use tl3_controller_core::worker_events::*;
    runner(256).run(&(0u8..7, 1u64..u64::MAX, "[a-f0-9]{64}", any::<bool>()), |(kind, reset, key, postmortem)| {
        let role = if postmortem { "postmortem" } else { "ingest" };
        let (value, expected) = match kind {
            0 => (json!({"event":"progress","unit":"model_turn"}), WorkerEvent::Progress {unit:ProgressUnit::ModelTurn}),
            1 => (json!({"event":"progress","unit":"tool_result"}), WorkerEvent::Progress {unit:ProgressUnit::ToolResult}),
            2 => (json!({"event":"progress","unit":"external_wait","wait":{"upstream":"prismabuild","key":key}}), WorkerEvent::Progress {unit:ProgressUnit::ExternalWait}),
            3 => (json!({"event":"reply","role":role,"reply":{"arbitrary":"payload"}}), WorkerEvent::Reply),
            4 => (json!({"event":"quota","role":role,"reset_ms":reset}), WorkerEvent::Quota {model:QuotaModel::Implicit,reset_ms:Some(reset),facts:None}),
            5 => (json!({"event":"quota","role":role,"reset_ms":reset,"model":"grade"}), WorkerEvent::Quota {model:if postmortem {QuotaModel::Grade} else {QuotaModel::Forbidden},reset_ms:Some(reset),facts:None}),
            _ => (json!({"event":"progress","unit":"unknown"}), WorkerEvent::Invalid),
        };
        let bytes=serde_json::to_vec(&value).unwrap();
        let parsed=super::parse_event(&bytes,Some(role),postmortem,&json!({"grade_model":"grade"})).unwrap();
        prop_assert_eq!(parsed.event,expected);
        prop_assert_eq!(&parsed.value,&value);
        if kind==2 {prop_assert_eq!(&parsed.wait.as_ref().unwrap().key,&key);}
        let encoded=serde_json::to_vec(&parsed.value).unwrap();
        let again=super::parse_event(&encoded,Some(role),postmortem,&json!({"grade_model":"grade"})).unwrap();
        prop_assert_eq!(again.event,parsed.event);prop_assert_eq!(again.wait,parsed.wait);
        Ok(())
    }).unwrap();
}

#[test]
fn mandatory_valid_replies_and_after_reply() {
    let cases = include_str!("../fixtures/worker-replies-d8e4f2a8.jsonl");
    let mut valid = 0;
    for case in cases.lines() {
        let case: Value = serde_json::from_str(case).unwrap();
        let output = if case["start"] == "Classify" {
            json!({"Comment":{"node":0,"kind":"Classify"}})
        } else {
            json!({"Effect":{"node":0,"kind":{"StartRun":case["start"]},"seq":1,"head":null}})
        };
        let mut d = delivery(false);
        d.output = serde_json::from_value(output).unwrap();
        let bytes = case["stdout"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|v| {
                let mut b = serde_json::to_vec(v).unwrap();
                b.push(b'\n');
                b
            })
            .collect::<Vec<_>>();
        let old = outcome(reference::parse_reply(&bytes, &d, &case["inputs"]));
        assert!(old.get("ok").is_some(), "fixture {case}: {old}");
        assert_eq!(
            old,
            outcome(current::parse_reply(&bytes, &d, &case["inputs"]))
        );
        valid += 1;
        if case["stdout"].as_array().unwrap().last().unwrap()["event"] == "reply" {
            let mut after = bytes;
            after.extend(line(0, false));
            let old = outcome(reference::parse_reply(&after, &d, &case["inputs"]));
            assert_eq!(old["error"], "event after worker reply");
            assert_eq!(
                old,
                outcome(current::parse_reply(&after, &d, &case["inputs"]))
            );
        }
    }
    assert!(valid >= 25);
}

#[test]
fn valid_reply_streams() {
    use proptest::prelude::*;
    let cases = include_str!("../fixtures/worker-replies-d8e4f2a8.jsonl")
        .lines()
        .map(|v| serde_json::from_str::<Value>(v).unwrap())
        .collect::<Vec<_>>();
    runner(128).run(&(0usize..cases.len(), proptest::collection::vec(0u8..3,0..30),0u8..5), |(index,prefix,tail)| {
        let case=&cases[index];
        let output=if case["start"]=="Classify" {json!({"Comment":{"node":0,"kind":"Classify"}})} else {
            json!({"Effect":{"node":0,"kind":{"StartRun":case["start"]},"seq":1,"head":null}})
        };
        let mut d=delivery(false);d.output=serde_json::from_value(output).unwrap();
        let mut bytes=prefix.into_iter().flat_map(|k|line(k,false)).collect::<Vec<_>>();
        for value in case["stdout"].as_array().unwrap() {bytes.extend(serde_json::to_vec(value).unwrap());bytes.push(b'\n');}
        match tail {1=>bytes.extend(line(0,false)),2=>bytes.extend(b"{\n"),3=>{bytes.pop();},4=>bytes.extend(b"\xff\n"),_=>()}
        prop_assert_eq!(outcome(reference::parse_reply(&bytes,&d,&case["inputs"])),outcome(current::parse_reply(&bytes,&d,&case["inputs"])));
        Ok(())
    }).unwrap();
}
#[test]
fn arbitrary_byte_streams() {
    use proptest::prelude::*;
    runner(128)
        .run(
            &(
                proptest::collection::vec(any::<u8>(), 0..300),
                any::<bool>(),
            ),
            |(mut bytes, postmortem)| {
                bytes.push(b'\n');
                parity(&bytes, postmortem, &Value::Null);
                Ok(())
            },
        )
        .unwrap();
}
#[test]
fn mandatory_postmortem_model_and_reply() {
    use tl3_controller_core::worker_events::*;
    let inputs = json!({"grade_model":"grade"});
    let reply = crate::postmortems::simulated_reply();
    let mut bytes =
        serde_json::to_vec(&json!({"event":"reply","role":"postmortem","reply":reply})).unwrap();
    bytes.push(b'\n');
    assert!(parity(&bytes, true, &inputs).get("ok").is_some());
    assert!(
        parity(&line(4, true), true, &inputs)["ok"]
            .get("Quota")
            .is_some()
    );
    assert_eq!(
        parity(&line(4, true), true, &Value::Null)["error"],
        "invalid quota model"
    );
    assert_eq!(
        parity(&line(4, false), false, &inputs)["error"],
        "invalid quota model"
    );
    assert_eq!(
        parity(&line(6, true), true, &inputs)["error"],
        "invalid quota model"
    );
    let mut bad = serde_json::to_vec(
        &json!({"event":"quota","role":"postmortem","reset_ms":0,"model":false}),
    )
    .unwrap();
    bad.push(b'\n');
    assert_eq!(parity(&bad, true, &inputs)["error"], "invalid quota model");
    assert_eq!(
        super::parse_event(&line(2, true), None, false, &Value::Null)
            .unwrap()
            .event,
        WorkerEvent::Progress {
            unit: ProgressUnit::ExternalWait
        }
    );
}

/// P5/Q53: the existing typed event uses the worker sample through decoding,
/// the core reset decision, and the original quota terminal-order decision.
#[test]
fn sampled_quota_uses_f1_event_core() {
    use tl3_controller_core::worker_events::*;
    let link = json!({"response_status":429,"headers":{"x-ratelimit-reset":"1000.125"},
        "api_status":429,"api_code":null,"judge":false,"judge_status":null,
        "judge_code":null,"grade_judge":false});
    for (now, expected) in [
        ("1000.0", 1_000_125),
        ("1000.125", 1_120_125),
        ("1000.25", 1_120_250),
    ] {
        let value = json!({"event":"quota","role":"ingest",
            "facts":{"now":now,"role":"ingest","links":[link.clone()]}});
        let mut bytes = serde_json::to_vec(&value).unwrap();
        bytes.push(b'\n');
        let parsed = super::parse_event(&bytes, Some("ingest"), false, &Value::Null).unwrap();
        let WorkerEvent::Quota {
            reset_ms,
            facts: Some(facts),
            ..
        } = parsed.event
        else {
            panic!("{parsed:?}")
        };
        assert_eq!(reset_ms, Some(expected));
        assert_eq!(
            facts.sampled_now.bits,
            now.parse::<f64>().unwrap().to_bits()
        );
        assert_eq!(quota_reset(facts), Some(expected));
        assert_eq!(
            reply_step(parsed.event, false, false),
            ReplyDecision::AfterQuota
        );
        assert_eq!(
            reply_step(parsed.event, true, true),
            ReplyDecision::AfterReply
        );
        let result = outcome(super::parse_reply(&bytes, &delivery(false), &Value::Null));
        assert_eq!(result["ok"]["Quota"]["reset_ms"], expected);
        bytes.extend(line(0, false));
        assert_eq!(
            outcome(super::parse_reply(&bytes, &delivery(false), &Value::Null))["error"],
            "event after quota ending"
        );
    }
}
