use super::*;
use serde_json::json;

fn ingest(tokens: u64) -> Value {
    json!({
        "kind": "brief", "brief": "Fix greeting.",
        "goals": [{"description": "Return hello.", "verification": "test"}],
        "memory_ids": [], "files": ["greeting.py"],
        "forecast": {
            "tokenizer": "o200k_base", "brief_tokens": tokens,
            "file_tokens": {"greeting.py": 0}, "overhead_tokens": 0,
            "tokens": tokens, "fit_tokens": 100, "context_tokens": 150,
            "fits": tokens < 100,
        }
    })
}

#[test]
fn sizing_ceil_floor_and_kernel_range() {
    for (tokens, size) in [
        (0, Some(1)),
        (1, Some(1)),
        (100, Some(1)),
        (101, Some(2)),
        (25_500, Some(255)),
        (25_501, None),
    ] {
        let mapped = map_reply("ingest", &ingest(tokens));
        if let Some(size) = size {
            assert_eq!(mapped.unwrap(), Reply::IngestDone { size }, "{tokens}");
        } else {
            assert!(mapped.is_err(), "{tokens} must fail instead of wrapping");
        }
    }
}

#[test]
fn rejects_inconsistent_or_forged_forecasts() {
    for (field, value) in [
        ("tokens", json!(2)),
        ("fits", json!(false)),
        ("fit_tokens", json!(0)),
        ("context_tokens", json!(100)),
        ("brief_tokens", json!(u64::MAX)),
        ("overhead_tokens", json!(u64::MAX)),
        ("tokenizer", json!("other")),
        ("file_tokens", json!({})),
        ("file_tokens", json!({"forged.py": 0})),
        ("file_tokens", json!({"greeting.py": 0, "extra.py": 0})),
    ] {
        let mut reply = ingest(1);
        reply["forecast"][field] = value;
        assert!(map_reply("ingest", &reply).is_err(), "{field}: {reply}");
    }
}

#[test]
fn checks_forecast_against_saved_configuration() {
    let settings =
        json!({"fit_tokens": 100, "context_tokens": 150, "overhead_tokens": {"prompt": 0}});
    let reply = ingest(1);
    validate_forecast_settings(&reply, &settings).unwrap();
    for (field, value) in [
        ("fit_tokens", 101),
        ("context_tokens", 151),
        ("overhead_tokens", 1),
    ] {
        let mut forged = reply.clone();
        forged["forecast"][field] = json!(value);
        if field == "overhead_tokens" {
            forged["forecast"]["tokens"] = json!(2);
        }
        map_reply("ingest", &forged).unwrap();
        assert!(
            validate_forecast_settings(&forged, &settings).is_err(),
            "{field}"
        );
    }
}

#[test]
fn split_forecast_keys_are_the_union_of_piece_files() {
    let mut reply = json!({"kind": "split", "reason": "out_of_room", "pieces": [
        {"brief": "First", "goal_ids": [0], "files": ["greeting.py"]},
        {"brief": "Second", "goal_ids": [0], "files": ["greeting.py", "new.py"]},
    ], "forecast": ingest(1)["forecast"], "violations": []});
    assert!(map_reply("planner", &reply).is_err());
    reply["forecast"]["file_tokens"]["new.py"] = json!(0);
    assert_eq!(map_reply("planner", &reply).unwrap(), Reply::PlanSplit);
    reply["forecast"]["file_tokens"]["extra.py"] = json!(0);
    assert!(map_reply("planner", &reply).is_err());
}
