//! #820: prompt/schema identity and proof omission parity at the planner boundary.
use serde_json::{Value, json};
use tl3_controller::contracts::roles::planner_reply::{self, PlannerReply, Proof};

#[test]
fn planner_schema_is_byte_identical_and_registered_for_drift() {
    let committed = include_str!("../../../workers/src/tl3_workers/schemas/planner-reply.json");
    assert_eq!(planner_reply::schema_text(), committed);
    let generated = tl3_controller::contracts::generated();
    assert!(
        generated
            .iter()
            .any(|file| file.path.ends_with("/planner-reply.json") && file.text == committed)
    );
}

#[test]
fn planner_proof_omissions_are_explicit_and_round_trip() {
    for partial in [
        json!({}),
        json!({"subjects":[]}),
        json!({"observer_support":[]}),
        json!({"red_message":"assert status"}),
    ] {
        let proof: Proof = serde_json::from_value(partial.clone()).unwrap();
        assert!(proof.complete().is_none());
        assert_eq!(serde_json::to_value(proof).unwrap(), partial);
    }
    let complete = json!({"subjects":[],"observer_support":[],"red_message":"assert status"});
    let proof: Proof = serde_json::from_value(complete.clone()).unwrap();
    assert_eq!(
        serde_json::to_value(proof.complete().expect("all fields declared")).unwrap(),
        complete
    );
}

#[test]
fn planner_variants_round_trip_and_reject_unknown_fields() {
    let cases = [
        json!({"kind":"leaf","files":[],"checks":[],"no_design_questions":true,"lands_on_its_own":false}),
        json!({"kind":"split","reason":"multiple_repos","pieces":[]}),
        json!({"kind":"cant_meet","report":"No useful change.","files":[]}),
    ];
    for value in cases {
        let typed: PlannerReply = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(typed).unwrap(), value);
        let mut extra = value;
        extra["unknown"] = json!(true);
        assert!(serde_json::from_value::<PlannerReply>(extra).is_err());
    }
    for invalid in [
        json!({"kind":"leaf"}),
        json!({"kind":"split","reason":"invented","pieces":[]}),
        json!({"kind":"cant_meet","report":"","files":[]}),
        Value::Null,
    ] {
        assert!(serde_json::from_value::<PlannerReply>(invalid).is_err());
    }
}
