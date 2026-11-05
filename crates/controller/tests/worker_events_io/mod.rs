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
mod pre_fix;
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
    assert_eq!(
        old,
        outcome(pre_fix::parse_reply(bytes, &d, inputs)),
        "F1/F2 reference drift: stream={bytes:?}"
    );
    // The sole intended divergence: the old parser accepts a non-final quota
    // because its bytes equal the final line, ignoring all intervening events.
    let lines = bytes.split_inclusive(|b| *b == b'\n').collect::<Vec<_>>();
    let early_quota = old["ok"].get("Quota").is_some()
        && lines.len() > 1
        && lines[..lines.len() - 1].contains(lines.last().unwrap());
    if early_quota {
        assert_eq!(
            new,
            outcome(Err(Error::Invalid("event after quota ending")))
        );
    } else {
        assert_eq!(old, new, "stream={bytes:?}, postmortem={postmortem}");
    }
    new
}
/// Copies the committed regressions to writable test output so proptest can
/// append new minimized failures there instead of the read-only runfiles tree.
/// One file per test (libtest names each test thread) so parallel runners
/// cannot clobber each other.
fn writable_regressions() -> &'static str {
    let output = std::env::var_os("TEST_UNDECLARED_OUTPUTS_DIR")
        .or_else(|| std::env::var_os("TEST_TMPDIR"))
        .unwrap_or_else(|| std::env::temp_dir().into_os_string());
    let test = std::thread::current()
        .name()
        .unwrap_or("runner")
        .replace(|c: char| !c.is_ascii_alphanumeric(), "_");
    let name = format!("worker_events_io-{test}-regressions.txt");
    let path = std::path::PathBuf::from(output).join(name);
    std::fs::write(&path, include_str!("proptest-regressions.txt")).unwrap();
    Box::leak(path.to_str().unwrap().to_owned().into_boxed_str())
}
fn runner(cases: u32) -> proptest::test_runner::TestRunner {
    use proptest::test_runner::*;
    TestRunner::new(Config {
        cases,
        rng_seed: RngSeed::Fixed(0xF176_0202_6005),
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(
            writable_regressions(),
        ))),
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
    assert_eq!(
        parity(&repeated, false, &Value::Null)["error"],
        "event after quota ending"
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
    // Actual F2 base d8f58cc383661f06338cc4f0f0c2b4bd2ebe7e02, not a shared shim over the changed core.
    for (name, digest) in [
        (
            "parse_reply",
            "2696c4294b20061aeb3cebacfbf1d727f43db92a37553f00e82175d0128e0170",
        ),
        (
            "reply_step",
            "b2a68b003639a1e65b7fbb770763302996cb73abcf440affadef21bf69e6e253",
        ),
    ] {
        let original = function(include_str!("pre_fix.rs"), name);
        let adapted = if name == "parse_reply" {
            let field = "        plan_check: None,\n";
            assert_eq!(original.matches(field).count(), 1);
            original.replacen(field, "", 1)
        } else {
            original.to_owned()
        };
        assert_eq!(format!("{:x}", Sha256::digest(adapted)), digest);
    }
    for name in ["refresh", "progress", "external_wait", "parse_reply"] {
        let reference = function(include_str!("reference.rs"), name);
        // #832 adds an optional observation field. The frozen shell still supplies
        // None, and every original statement must remain byte-identical.
        let adapted = if name == "parse_reply" {
            let field = "        plan_check: None,\n";
            assert_eq!(reference.matches(field).count(), 1);
            reference.replacen(field, "", 1)
        } else {
            reference.to_owned()
        };
        assert_eq!(function(base, name), adapted);
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
            4 => (json!({"event":"quota","role":role,"reset_ms":reset}), WorkerEvent::Quota {model:QuotaModel::Implicit,reset_ms:Some(reset)}),
            5 => (json!({"event":"quota","role":role,"reset_ms":reset,"model":"grade"}), WorkerEvent::Quota {model:if postmortem {QuotaModel::Grade} else {QuotaModel::Forbidden},reset_ms:Some(reset)}),
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

include!("quota_position_red.rs");

#[test]
fn mandatory_quota_position_and_precedence() {
    let inputs = json!({"grade_model":"grade"});
    for postmortem in [false, true] {
        for kind in if postmortem { vec![3, 4] } else { vec![3] } {
            let quota = line(kind, postmortem);
            assert!(
                parity(&quota, postmortem, &inputs)["ok"]
                    .get("Quota")
                    .is_some()
            );
            for prefix in 0..3 {
                for tail in 0..18 {
                    let mut bytes = (0..prefix)
                        .flat_map(|_| line(0, postmortem))
                        .collect::<Vec<_>>();
                    bytes.extend(&quota);
                    bytes.extend(match tail {
                        16 => b"\n".to_vec(),
                        17 => b"not-json\n".to_vec(),
                        _ => line(tail, postmortem),
                    });
                    // Every byte of the middle is ignored by the buggy parser.
                    bytes.extend(&quota);
                    let d = delivery(postmortem);
                    assert!(reference::parse_reply(&bytes, &d, &inputs).is_ok());
                    assert_eq!(
                        parity(&bytes, postmortem, &inputs)["error"],
                        "event after quota ending"
                    );
                }
            }
            let mut after = quota.clone();
            after.extend(b"\n");
            assert_eq!(
                parity(&after, postmortem, &inputs)["error"],
                "event after quota ending"
            );
            let mut partial = quota.clone();
            partial.extend(b"{");
            assert_eq!(
                parity(&partial, postmortem, &inputs)["error"],
                "incomplete worker output"
            );
        }
        // Model/reset validation retains priority over position validation.
        for (kind, error) in [(5, "invalid quota reset"), (6, "invalid quota model")] {
            let mut bytes = line(kind, postmortem);
            bytes.extend(line(0, postmortem));
            bytes.extend(line(kind, postmortem));
            assert_eq!(parity(&bytes, postmortem, &inputs)["error"], error);
        }
    }
}

#[test]
fn generated_quota_position_divergence() {
    use proptest::prelude::*;
    runner(256)
        .run(
            &(
                any::<bool>(),
                any::<bool>(),
                1u64..u64::MAX,
                proptest::collection::vec(0u8..3, 0..20),
                proptest::collection::vec(any::<u8>(), 0..512),
            ),
            |(postmortem, explicit, reset, prefix, middle)| {
                let role = if postmortem { "postmortem" } else { "ingest" };
                let mut value = json!({"event":"quota","role":role,"reset_ms":reset});
                if postmortem && explicit {
                    value["model"] = json!("grade");
                }
                let mut quota = serde_json::to_vec(&value).unwrap();
                quota.push(b'\n');
                let mut bytes = prefix
                    .into_iter()
                    .flat_map(|k| line(k, postmortem))
                    .collect::<Vec<_>>();
                bytes.extend(&quota);
                bytes.extend(middle);
                bytes.push(b'\n');
                bytes.extend(&quota);
                let inputs = json!({"grade_model":"grade"});
                prop_assert!(
                    reference::parse_reply(&bytes, &delivery(postmortem), &inputs).is_ok()
                );
                let result = parity(&bytes, postmortem, &inputs);
                prop_assert_eq!(result["error"].as_str(), Some("event after quota ending"));
                Ok(())
            },
        )
        .unwrap();
}
