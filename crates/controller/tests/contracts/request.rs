use proptest::prelude::*;
use serde_json::{Value, json};
use tl3_controller::contracts::{bounded::*, request::*};

#[test]
fn request_generation_matches_committed() {
    assert_eq!(
        generated().unwrap().text,
        include_str!("../../../../workers/src/tl3_workers/schemas/request.json")
    );
}

#[test]
fn nullable_presence_and_numeric_spelling_are_preserved() {
    let value = json!({"protocol":1.0,"operation":{"node":18446744073709551616.0,"generation":0.0,"kind":"making"},"role":"maker",
        "model":{"base_url":"https://model.example/v1","name":"model","effort":null,"budget_tokens":1.0},"key_file":"/key","inputs":{},"worktree":"/work","mlflow":null,"hindsight_key_file":null});
    let typed: WorkerRequest = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(typed).unwrap(), value);
    let mut invalid = value;
    invalid["model"].as_object_mut().unwrap().remove("effort");
    assert!(serde_json::from_value::<WorkerRequest>(invalid).is_err());
}

#[test]
fn request_patterns_agree_with_the_controller_schemas() {
    let url = jsonschema::validator_for(&schemars::schema_for!(ModelUrl).to_value()).unwrap();
    for s in [
        "http://x",
        "https://x/a/b?c#d",
        "http://x/",
        "http://",
        "ftp://x",
        "http://x y",
        "http://x\n",
        "http://x\u{85}",
        "http://x\u{1c}",
        "http://x\u{feff}",
        "http://x\u{a0}",
        "http://x\u{2005}",
    ] {
        assert_eq!(
            ModelUrl::new(s).is_ok(),
            url.is_valid(&json!(s)),
            "URL {s:?}"
        );
    }
    let role = jsonschema::validator_for(&schemars::schema_for!(WireRole).to_value()).unwrap();
    for s in ["maker", "a1_", "A", "1a", "a\n", "", "aé"] {
        assert_eq!(
            WireRole::new(s).is_ok(),
            role.is_valid(&json!(s)),
            "role {s:?}"
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn typed_requests_satisfy_the_schema_and_round_trip(n in any::<u64>(), budget in 1u64..=u64::MAX, text in ".{1,20}", variant in 0..3, has_effort in any::<bool>()) {
        let nonempty=Text::new(text.clone()).unwrap();
        let path=AbsolutePath::new(format!("/{text}")).unwrap();
        let object=JsonObject(serde_json::Map::from_iter([("value".into(),Value::from(text))]));
        let request=WorkerRequest {
            mlflow: match variant {0=>None,1=>Some(None),_=>Some(Some(object.clone()))},trace:Some(object.clone()),protocol:Protocol(RecordSchemaVersion::default()),
            operation:Operation {node:Natural::new(n).unwrap(),generation:Natural::new(n).unwrap(),kind:nonempty.clone()},role:WireRole::new(format!("role{n}")).unwrap(),
            model:Model {base_url:ModelUrl::new("https://example.test/v1").unwrap(),name:nonempty.clone(),effort:has_effort.then_some(nonempty.clone()),budget_tokens:Natural::new(budget).unwrap()},
            hindsight_key_file:match variant {0=>None,1=>Some(None),_=>Some(Some(path.clone()))},key_file:path.clone(),test_cpus:Some(Natural::new(budget).unwrap()),silence_secs:Some(Natural::new(budget).unwrap()),no_progress_secs:Some(Natural::new(budget).unwrap()),inputs:object,worktree:path.clone(),session_file:Some(path.clone()),last_accepted_generation:Some(Natural::new(n).unwrap()),resume_session:Some(has_effort),
            prompt:Some(Prompt {version:Natural::new(budget).unwrap(),text:nonempty}),memory:Some(Memory {summary_file:path,repo:String::new(),issue:Natural::new(budget).unwrap(),request:String::new(),epoch:Natural::new(n).unwrap()})
        };
        static VALIDATOR:std::sync::OnceLock<jsonschema::Validator>=std::sync::OnceLock::new();
        let validator=VALIDATOR.get_or_init(||jsonschema::validator_for(&schema()).unwrap());
        let value=serde_json::to_value(&request).unwrap();
        prop_assert!(validator.is_valid(&value),"{:?}",validator.iter_errors(&value).map(|e|e.to_string()).collect::<Vec<_>>());
        prop_assert_eq!(serde_json::from_value::<WorkerRequest>(value).unwrap(),request);
    }
}
