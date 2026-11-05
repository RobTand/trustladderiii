use super::*;
use crate::adapters::workers::protocol::{MappedReply, map_reply};
use serde_json::json;

// Verbatim planner arm from cdacce8f, with the original schema boundary. Kept as
// the test-only migration oracle until P2b ships; no sampled I/O occurs here.
fn legacy(reply: &Value) -> Result<Reply> {
    let validator = super::super::validator("planner", "reply")?;
    if !validator.is_valid(reply) {
        return Err(Error::Invalid("worker reply breaks role schema"));
    }
    super::super::forecast(reply)?;
    let violations = reply["violations"]
        .as_array()
        .ok_or(Error::Invalid("planner violations missing"))?;
    if !violations.is_empty() || reply["kind"] == "cant_meet" {
        Ok(Reply::PlanCantMeet)
    } else if reply["kind"] == "split" {
        Ok(Reply::PlanSplit)
    } else {
        Ok(Reply::PlanLeaf {
            footprint: tl3_kernel::Footprint::default(),
            fits: reply["forecast"]["fits"] == true,
        })
    }
}

#[test]
fn planner_reply_mapping_matches_pre_extraction_arm() {
    let forecast = json!({"tokenizer":"o200k_base", "brief_tokens":1,
        "file_tokens":{"a.py":0}, "overhead_tokens":0,"tokens":1,
        "fit_tokens":100,"context_tokens":150,"fits":true});
    let cases = [
        json!({"kind":"leaf", "files":["a.py"], "checks":[],
            "no_design_questions":true,"lands_on_its_own":true}),
        json!({"kind":"split","reason":"out_of_room","pieces":[
            {"brief":"First", "goal_ids":[0],"files":["a.py"]}]}),
        json!({"kind":"cant_meet","report":"Cannot fit.","files":["a.py"]}),
    ];
    for mut case in cases {
        case["forecast"] = forecast.clone();
        for violations in [json!([]), json!(["Cannot proceed."])] {
            case["violations"] = violations;
            let expected = legacy(&case).unwrap();
            assert_eq!(map(&case).unwrap(), expected, "{case}");
        }
    }
}

#[test]
fn planner_reply_preserves_errors_and_forecast_precedence() {
    let leaf = serde_json::json!({"kind":"leaf","files":[],"checks":[],
        "no_design_questions":true,"lands_on_its_own":true,
        "forecast":{"tokenizer":"o200k_base","brief_tokens":0,"file_tokens":{},
            "overhead_tokens":0,"tokens":0,"fit_tokens":100,"context_tokens":150,"fits":true},
        "violations":[]});
    let mut cases = vec![leaf.clone()];
    for key in ["forecast", "violations", "files", "kind"] {
        let mut case = leaf.clone();
        case.as_object_mut().unwrap().remove(key);
        cases.push(case);
    }
    for (path, value) in [
        ("/forecast/tokens", serde_json::json!(0.0)),
        ("/forecast/tokens", serde_json::json!(1)),
        ("/violations", serde_json::json!(["blocked"])),
        ("/files", serde_json::json!(["a.py", "a.py"])),
        (
            "/checks",
            serde_json::json!([{"kind":"reproducing","node_ids":[],"goal_ids":[]}]),
        ),
    ] {
        let mut case = leaf.clone();
        *case.pointer_mut(path).unwrap() = value;
        cases.push(case);
    }
    for case in cases {
        let expected = legacy(&case).map_err(|e| e.to_string());
        let actual = map_reply("planner", &case)
            .map(|mapped| match mapped {
                MappedReply::Kernel(reply) => reply,
                _ => panic!("planner kernel reply"),
            })
            .map_err(|e| e.to_string());
        assert_eq!(actual, expected, "{case}");
    }
}

#[test]
fn planner_reply_mapping_preserves_nested_schema_acceptance() {
    let leaf = json!({"kind":"leaf","files":[],"checks":[{
        "kind":"demonstrating","node_ids":["tests/test_a.py::test_a"],"goal_ids":[0],
        "forcing":{"symptom":"deadline","environment":{},"patches":[]},
        "proof":{"subjects":[],"observer_support":[],"red_message":"deadline"},
        "declared_symptom":{"node":"tests/test_a.py::test_a","exception":"builtins.TimeoutError",
            "phase":"call","repeat":1,"parallel":0}}],
        "no_design_questions":true,"lands_on_its_own":true,
        "forecast":{"tokenizer":"o200k_base","brief_tokens":0,"file_tokens":{},
            "overhead_tokens":0,"tokens":0,"fit_tokens":100,"context_tokens":150,"fits":true},
        "violations":[]});
    assert!(matches!(legacy(&leaf).unwrap(), Reply::PlanLeaf { .. }));
    let mut cases = vec![leaf.clone()];
    for exception in ["module.Erreuré", "module.A\u{301}", "module.Error\n"] {
        let mut case = leaf.clone();
        case["checks"][0]["declared_symptom"]["exception"] = exception.into();
        cases.push(case);
    }
    for name in ["DEADLINE", "DEADLINE\n"] {
        let mut case = leaf.clone();
        case["checks"][0]["forcing"]["environment"] = json!({name:"1"});
        cases.push(case);
    }
    for target in ["module.deadline", "module.deadline\n"] {
        let mut case = leaf.clone();
        case["checks"][0]["forcing"]["patches"] = json!([{"target":target,"returns":null}]);
        cases.push(case);
    }
    for case in cases {
        let expected = legacy(&case).map_err(|e| e.to_string());
        let actual = map_reply("planner", &case)
            .map(|mapped| match mapped {
                MappedReply::Kernel(reply) => reply,
                _ => panic!("planner kernel reply"),
            })
            .map_err(|e| e.to_string());
        assert_eq!(actual, expected, "{case}");
    }
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config { cases: 64, ..Default::default() })]
    #[test]
    fn planner_reply_mapping_parity_on_generated_replies(
        kind in 0usize..3, tokens in 0u16..300, violates in proptest::bool::ANY,
        integral_float in proptest::bool::ANY
    ) {
        let files=serde_json::json!(["a.py"]);
        let mut value=match kind {
            0=>serde_json::json!({"kind":"leaf","files":files,"checks":[],"no_design_questions":true,"lands_on_its_own":true}),
            1=>serde_json::json!({"kind":"split","reason":"does_not_fit","pieces":[{"brief":"First", "goal_ids":[0], "files":files}]}),
            _=>serde_json::json!({"kind":"cant_meet","report":"Cannot fit.","files":files}),
        };
        value["forecast"]=serde_json::json!({"tokenizer":"o200k_base", "brief_tokens":tokens,
            "file_tokens":{"a.py":0},"overhead_tokens":0,"tokens":tokens,
            "fit_tokens":100,"context_tokens":150,"fits":tokens<100});
        value["violations"]=if violates {serde_json::json!(["rule break"])} else {serde_json::json!([])};
        if integral_float {value["estimated_tool_turns"]=serde_json::json!(f64::from(tokens));}
        let expected=legacy(&value).map_err(|e|e.to_string());
        let actual=map_reply("planner",&value).map(|reply|match reply {MappedReply::Kernel(reply)=>reply,_=>panic!("planner kernel reply")}).map_err(|e|e.to_string());
        proptest::prop_assert_eq!(actual,expected);
    }
}
