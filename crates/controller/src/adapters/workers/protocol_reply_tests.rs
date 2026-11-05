use super::*;
use proptest::prelude::*;
use serde_json::json;

include!("protocol_reply_reference.rs");

fn outcome(value: Result<MappedReply>) -> String {
    match value {
        Ok(MappedReply::Kernel(reply)) => format!("kernel:{reply:?}"),
        Ok(MappedReply::Made(head)) => format!("made:{head}"),
        Ok(MappedReply::NeedsFiles { paths, fits }) => format!("files:{paths:?}:{fits}"),
        Err(error) => format!("error:{error}"),
    }
}

fn compare(role: &str, reply: &Value) {
    assert_eq!(
        outcome(map_reply(role, reply)),
        outcome(reference_map_reply(role, reply)),
        "{role}: {reply}"
    );
}

#[test]
fn mapping_matches_existing_reply_corpus() {
    let corpus = include_str!("../../../tests/fixtures/worker-replies-d8e4f2a8.jsonl");
    let mut count = 0;
    for line in corpus.lines() {
        let record: Value = serde_json::from_str(line).unwrap();
        for event in record["stdout"].as_array().unwrap() {
            if event["event"] == "reply" {
                compare(event["role"].as_str().unwrap(), &event["reply"]);
                count += 1;
            }
        }
    }
    assert!(count >= 10, "controller reply corpus is missing");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]
    #[test]
    fn mapping_preserves_maker_checks_and_paths(path in "[a-z./\\\\]{0,35}", fits in any::<bool>(), met in any::<bool>()) {
        compare("maker", &json!({"kind":"needs_files", "files":[{"path":path,"why":"needed"}],"fits":fits}));
        let check = if met { json!({"goal_id":"g","status":"met","evidence":"done"}) } else {json!({"goal_id":"g","status":"not_met","why":"pending"})};
        compare("maker", &json!({"kind":"done","head":"a".repeat(40),"self_check":[check]}));
    }
}

#[test]
fn ingest_schema_is_byte_identical() {
    assert_eq!(
        crate::contracts::roles::ingest_reply::schema_text(),
        include_str!("../../../../../workers/src/tl3_workers/schemas/ingest-reply.json")
    );
}

#[test]
fn classifier_schema_is_byte_identical() {
    assert_eq!(
        crate::contracts::roles::classifier_reply::schema_text(),
        include_str!("../../../../../workers/src/tl3_workers/schemas/classifier-reply.json")
    );
}

#[test]
fn maker_schema_is_byte_identical() {
    assert_eq!(
        crate::contracts::roles::maker_reply::schema_text(),
        include_str!("../../../../../workers/src/tl3_workers/schemas/maker-reply.json")
    );
}

#[test]
fn writer_schema_is_byte_identical() {
    assert_eq!(
        crate::contracts::roles::writer_reply::schema_text(),
        include_str!("../../../../../workers/src/tl3_workers/schemas/writer-reply.json")
    );
}

#[test]
fn shared_types_round_trip_the_controller_reply_corpus() {
    use crate::contracts::roles::{
        classifier_reply::ClassifierReply, ingest_reply::IngestReply, maker_reply::MakerReply,
    };
    fn check<T: serde::Serialize + serde::de::DeserializeOwned>(reply: &Value) {
        let typed: T = serde_json::from_value(reply.clone()).unwrap();
        assert_eq!(serde_json::to_value(typed).unwrap(), *reply);
    }
    let corpus = include_str!("../../../tests/fixtures/worker-replies-d8e4f2a8.jsonl");
    let mut counts = [0usize; 3];
    for line in corpus.lines() {
        let record: Value = serde_json::from_str(line).unwrap();
        for event in record["stdout"].as_array().unwrap() {
            if event["event"] != "reply" {
                continue;
            }
            let role = event["role"].as_str().unwrap();
            let reply = &event["reply"];
            if !validator(role, "reply").unwrap().is_valid(reply) {
                continue;
            }
            match role {
                "ingest" => {
                    check::<IngestReply>(reply);
                    counts[0] += 1;
                }
                "classifier" => {
                    check::<ClassifierReply>(reply);
                    counts[1] += 1;
                }
                "maker" => {
                    check::<MakerReply>(reply);
                    counts[2] += 1;
                }
                _ => {}
            }
        }
    }
    assert!(
        counts.iter().all(|n| *n > 0),
        "missing corpus role: {counts:?}"
    );
}
