use proptest::prelude::*;
use serde_json::{Value, json};
use tl3_controller::contracts::{bounded::*, roles::postmortem_input::*};

#[test]
fn postmortem_generation_matches_committed() {
    assert_eq!(
        generated().unwrap().text,
        include_str!("../../../../workers/src/tl3_workers/schemas/postmortem-input.json")
    );
}

#[test]
fn rubric_extensions_and_optional_nulls_round_trip() {
    let value = json!({"record":{"id":1,"trigger":"failed","reason":"why"},
        "case":{"request":{},"outcome":"stopped","grades":[],"phases":[],"questions":[],"incidents":[],"release":"r","plan":null},
        "rubric":{"version":1,"dimensions":{},"extension":{"value":null}},"release":"r","grade_model":"model","deadline_secs":1,
        "trace_tags":{"node":"1","request":"1","repo":"owner/repo","issue":"1"}});
    let typed: PostmortemInput = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(typed).unwrap(), value);
}

#[test]
fn patterned_text_agrees_with_its_schema() {
    let digits = jsonschema::validator_for(&schemars::schema_for!(DecimalText).to_value()).unwrap();
    for s in ["0", "01", "123\n", "", "x", "1\n\n"] {
        assert_eq!(
            DecimalText::new(s).is_ok(),
            digits.is_valid(&json!(s)),
            "digits {s:?}"
        );
    }
    let repo =
        jsonschema::validator_for(&schemars::schema_for!(RepositoryName).to_value()).unwrap();
    for s in ["owner/repo", "o/r\n", "/r", "o/", "o/r/z", "o/é"] {
        assert_eq!(
            RepositoryName::new(s).is_ok(),
            repo.is_valid(&json!(s)),
            "repo {s:?}"
        );
    }
}

fn any_json() -> BoxedStrategy<Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::from),
        any::<i64>().prop_map(Value::from),
        ".{0,12}".prop_map(Value::from)
    ]
    .prop_recursive(2, 16, 4, |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
            proptest::collection::btree_map("[a-z]{1,4}", inner, 0..4)
                .prop_map(|m| Value::Object(m.into_iter().collect()))
        ]
    })
    .boxed()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn typed_postmortems_satisfy_the_schema_and_round_trip(id in 1u64..=u64::MAX, n in any::<u64>(), text in ".{1,12}", contents in any_json(), entries in proptest::collection::vec(any_json(),0..4)) {
        let nonempty=Text::new(text).unwrap();
        let map=JsonObject(serde_json::Map::from_iter([("anything".into(),contents.clone())]));
        let context=PostmortemInput {
            record:Record {id:Natural::new(id).unwrap(),trigger:nonempty.clone(),reason:nonempty.clone()},
            case:Case {request:map.clone(),outcome:nonempty.clone(),grades:JsonArray(entries.clone()),phases:JsonArray(entries.clone()),questions:JsonArray(entries.clone()),incidents:JsonArray(entries),plan:Some(contents.clone()),brief:Some(contents.clone()),audit:None,findings:Some(contents),release:String::new()},
            rubric:Rubric::new(Natural::new(id).unwrap(),map),release:String::new(),grade_model:nonempty,deadline_secs:Natural::new(id).unwrap(),
            trace_tags:TraceTags {node:DecimalText::new(n.to_string()).unwrap(),request:DecimalText::new(n.to_string()).unwrap(),issue:DecimalText::new(n.to_string()).unwrap(),repo:RepositoryName::new("owner/repo").unwrap()}
        };
        static VALIDATOR:std::sync::OnceLock<jsonschema::Validator>=std::sync::OnceLock::new();
        let validator=VALIDATOR.get_or_init(||jsonschema::validator_for(&schema()).unwrap());
        let value=serde_json::to_value(&context).unwrap();
        prop_assert!(validator.is_valid(&value),"{:?}",validator.iter_errors(&value).map(|e|e.to_string()).collect::<Vec<_>>());
        prop_assert_eq!(serde_json::from_value::<PostmortemInput>(value).unwrap(),context);
    }
}
