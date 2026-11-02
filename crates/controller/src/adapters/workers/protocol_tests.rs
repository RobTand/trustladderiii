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
            assert!(
                matches!(mapped.unwrap(), MappedReply::Kernel(Reply::IngestDone { size: actual }) if actual == size),
                "{tokens}"
            );
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
    assert!(matches!(
        map_reply("planner", &reply).unwrap(),
        MappedReply::Kernel(Reply::PlanSplit)
    ));
    reply["forecast"]["file_tokens"]["extra.py"] = json!(0);
    assert!(map_reply("planner", &reply).is_err());
}

#[test]
fn leaf_with_declared_test_edits_maps_by_its_violations() {
    // #438: edits_tests is optional on a leaf; worker code reports its rule breaks.
    let test = "tests/test_greeting.py::test_hello";
    let mut reply = json!({"kind": "leaf", "files": ["greeting.py"], "checks": [
        {"kind": "demonstrating", "node_ids": [test], "goal_ids": [0]},
    ], "edits_tests": [test], "no_design_questions": true, "lands_on_its_own": true,
    "forecast": ingest(1)["forecast"], "violations": []});
    assert!(matches!(
        map_reply("planner", &reply).unwrap(),
        MappedReply::Kernel(Reply::PlanLeaf { fits: true, .. })
    ));
    reply["violations"] = json!(["A test the change edits can't be frozen."]);
    assert!(matches!(
        map_reply("planner", &reply).unwrap(),
        MappedReply::Kernel(Reply::PlanCantMeet)
    ));
    let mut cant_meet = json!({"kind": "cant_meet", "report": "Blocked.", "files": ["greeting.py"],
        "edits_tests": [test], "forecast": ingest(1)["forecast"], "violations": []});
    assert!(map_reply("planner", &cant_meet).is_err());
    cant_meet.as_object_mut().unwrap().remove("edits_tests");
    assert!(matches!(
        map_reply("planner", &cant_meet).unwrap(),
        MappedReply::Kernel(Reply::PlanCantMeet)
    ));
}

#[test]
fn blocked_maker_reply_needs_cited_evidence() {
    // Worker code adds the class (and tool) it recorded; the controller cites only those.
    for evidence in [
        json!({"action_key": "c".repeat(64), "class": "test_infrastructure"}),
        json!({"tool_error": "tool-error-1", "tool": "run_tests", "class": "test_submission"}),
        json!({"tool_error": "tool-error-2", "tool": "write", "class": "EROFS"}),
    ] {
        let reply =
            json!({"kind": "blocked", "evidence": evidence, "report": "PrismaBuild failed."});
        assert!(
            matches!(
                map_reply("maker", &reply),
                Ok(MappedReply::Kernel(Reply::RunFailed))
            ),
            "{reply}"
        );
    }
    for evidence in [
        json!(null),
        json!({}),
        json!({"action_key": "short", "class": "test_infrastructure"}),
        json!({"action_key": "c".repeat(64)}),
        json!({"action_key": "c".repeat(64), "class": "@octocat"}),
        json!({"tool_error": "anything", "tool": "read", "class": "EIO"}),
        json!({"tool_error": "tool-error-1", "class": "EIO"}),
        json!({"tool_error": "tool-error-1", "tool": "shell", "class": "EIO"}),
        json!({"tool_error": "tool-error-1", "tool": "read", "class": "ENOTDIR"}),
        json!({"action_key": "c".repeat(64), "tool_error": "tool-error-1", "class": "EIO"}),
    ] {
        let mut reply = json!({"kind": "blocked", "report": "Broken."});
        if !evidence.is_null() {
            reply["evidence"] = evidence;
        }
        assert!(map_reply("maker", &reply).is_err(), "{reply}");
    }
}

#[test]
fn configured_evidence_preflight_does_not_accept_a_downgraded_leaf() {
    let delivery: crate::executor::Delivery = serde_json::from_value(json!({
        "key":"evidence-preflight", "epoch":1, "generation":3,
        "output":{"Effect":{"node":0,"kind":{"StartRun":"Plan"},"seq":1,"head":null}},
        "head_sha":null,"repository":{"owner":"owner","name":"repo","installation":1},
        "issue":2640,"pull_request":null,"paths":["greeting.py"],"attempt":1,"round":0,"checks":null
    })).unwrap();
    let contract = json!([{"requirement":"PB2640-2650 and PB2654 require BuildBuddy base/head runs under pinned CPU contention.",
        "tool":"run_tests", "executor":"buildbuddy_remote_bazel", "runner_features":["base_head","pinned_cpu_contention"],
        "artifacts":["invocation_url","sha","logs","terminal_verdict"],
        "methods":["red_green","forced_reproduction.contention_wrapper"], "protected_files":[]}]);
    let inputs = json!({"ingest":{"evidence_contract":contract}, "tool_capabilities":[{
        "tool":"run_tests","executor":"prismabuild_pytest", "runner_features":["base_head"],
        "artifacts":["sha","logs","terminal_verdict"],"methods":["red_green"],"protected_files":[]}],
        "protected_paths":[], "forecast":{"fit_tokens":100,"context_tokens":150,"overhead_tokens":{"prompt":0}}});
    let reply = json!({"kind":"leaf","files":["greeting.py"],"checks":[],
        "no_design_questions":true,"lands_on_its_own":true,"forecast":ingest(1)["forecast"],"violations":[]});
    let bytes = format!("{}\n", json!({"event":"reply","role":"planner","reply":reply}));
    let crate::executor::Observation::WorkerReply { reply, .. } =
        super::super::source::parse_reply(bytes.as_bytes(), &delivery, &inputs).unwrap() else { panic!() };
    assert_eq!(reply["kind"], "cant_meet");
    assert!(reply["report"].as_str().unwrap().contains("buildbuddy_remote_bazel"));
    assert!(matches!(map_reply("planner", &reply).unwrap(), MappedReply::Kernel(Reply::PlanCantMeet)));
}
