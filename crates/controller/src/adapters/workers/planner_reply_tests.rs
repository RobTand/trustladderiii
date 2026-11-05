use super::*;
use crate::adapters::workers::protocol::{MappedReply, map_reply};
use serde_json::json;

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
            let MappedReply::Kernel(expected) = map_reply("planner", &case).unwrap() else {
                panic!("planner must map to a kernel reply");
            };
            assert_eq!(map(&case).unwrap(), expected, "{case}");
        }
    }
}
