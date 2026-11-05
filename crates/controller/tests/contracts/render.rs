//! #819 P1a-0: ordering metadata must never change values or hide a schema change.
use proptest::prelude::*;
use serde_json::{Value, json};
use tl3_controller::contracts::render::{self, ErrorKind, Layout, TypeOrder, TypeUse};

fn kind(value: Value, inline: &[(&str, &[&str])]) -> render::RenderError {
    render::render(
        "example.json",
        &value,
        &Layout {
            inline,
            shared: &[],
        },
    )
    .unwrap_err()
}

#[test]
fn a_multikey_object_requires_an_order_entry() {
    let error = kind(json!({"wrapper":{"a":1,"b":2}}), &[]);
    assert_eq!(error.kind, ErrorKind::MissingOrder);
    assert_eq!(error.file, "example.json");
    assert_eq!(error.pointer, "/wrapper");
}

#[test]
fn a_missing_key_is_an_error() {
    let error = kind(json!({"a":1,"b":2}), &[("", &["a"])]);
    assert!(matches!(error.kind, ErrorKind::KeysDiffer { .. }));
    assert_eq!(error.pointer, "");
}

#[test]
fn an_extra_key_is_an_error() {
    let error = kind(json!({"a":1}), &[("", &["a", "b"])]);
    assert!(matches!(error.kind, ErrorKind::KeysDiffer { .. }));
}

#[test]
fn an_order_entry_must_match_an_object() {
    let error = kind(json!({"a":1}), &[("/absent", &["a"])]);
    assert_eq!(error.kind, ErrorKind::UnusedOrder);
    assert_eq!(error.pointer, "/absent");
}

#[test]
fn an_order_entry_cannot_target_a_scalar() {
    let error = kind(json!({"a":1}), &[("/a", &[])]);
    assert_eq!(error.kind, ErrorKind::UnusedOrder);
    assert_eq!(error.pointer, "/a");
}

#[test]
fn repeated_keys_in_an_order_are_an_error() {
    let error = kind(json!({"a":1}), &[("", &["a", "a"])]);
    assert!(matches!(error.kind, ErrorKind::KeysDiffer { .. }));
}

#[test]
fn a_pointer_cannot_have_two_order_entries() {
    let error = kind(json!({"a":1}), &[("", &["a"]), ("", &["a"])]);
    assert_eq!(error.kind, ErrorKind::DuplicateOrder);
}

#[test]
fn shared_type_order_is_reused_and_paths_are_escaped() {
    let shared = TypeOrder::of::<Vec<String>>(&[("", &["z", "a"])]);
    let layout = Layout {
        inline: &[("", &["second", "first/~"])],
        shared: &[
            TypeUse {
                pointer: "/first~1~0",
                order: &shared,
            },
            TypeUse {
                pointer: "/second",
                order: &shared,
            },
        ],
    };
    let value = json!({"first/~":{"a":1,"z":2},"second":{"a":3,"z":4}});
    let rendered = render::render("shared.json", &value, &layout).unwrap();
    assert_eq!(
        rendered,
        "{\n  \"second\": {\n    \"z\": 4,\n    \"a\": 3\n  },\n  \"first/~\": {\n    \"z\": 2,\n    \"a\": 1\n  }\n}\n"
    );
}

#[test]
fn a_named_type_cannot_declare_conflicting_orders() {
    let first = TypeOrder::of::<Vec<String>>(&[("", &["a", "b"])]);
    let second = TypeOrder::of::<Vec<String>>(&[("", &["b", "a"])]);
    let layout = Layout {
        inline: &[("", &["first", "second"])],
        shared: &[
            TypeUse {
                pointer: "/first",
                order: &first,
            },
            TypeUse {
                pointer: "/second",
                order: &second,
            },
        ],
    };
    let value = json!({"first":{"a":1,"b":2},"second":{"a":3,"b":4}});
    let error = render::render("shared.json", &value, &layout).unwrap_err();
    assert!(matches!(error.kind, ErrorKind::ConflictingTypeOrder { .. }));
    assert_eq!(error.pointer, "/second");
}

fn json_values() -> BoxedStrategy<Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::from),
        any::<i64>().prop_map(Value::from),
        ".{0,24}".prop_map(Value::from)
    ]
    .prop_recursive(3, 32, 5, |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..5).prop_map(Value::Array),
            proptest::collection::btree_map("[a-z/~]{1,5}", inner, 0..5)
                .prop_map(|map| Value::Object(map.into_iter().collect())),
        ]
    })
    .boxed()
}

fn orders(value: &Value, path: &str, result: &mut Vec<(String, Vec<String>)>) {
    match value {
        Value::Object(map) => {
            result.push((path.into(), map.keys().rev().cloned().collect()));
            for (key, value) in map {
                orders(
                    value,
                    &format!("{path}/{}", key.replace('~', "~0").replace('/', "~1")),
                    result,
                );
            }
        }
        Value::Array(values) => {
            for (i, value) in values.iter().enumerate() {
                orders(value, &format!("{path}/{i}"), result);
            }
        }
        _ => {}
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    /// P-schema-render (#819): key ordering preserves every serializable JSON value.
    #[test]
    fn rendering_then_parsing_preserves_serde_values(value in json_values()) {
        let original = serde_json::to_value(&value).unwrap();
        let mut owned = Vec::new();
        orders(&original,"",&mut owned);
        let keys: Vec<Vec<&str>> = owned.iter().map(|(_,keys)|keys.iter().map(String::as_str).collect()).collect();
        let inline: Vec<_> = owned.iter().zip(&keys).map(|((path,_),keys)|(path.as_str(),keys.as_slice())).collect();
        let rendered = render::render("property.json",&original,&Layout {inline:&inline,shared:&[]}).unwrap();
        prop_assert_eq!(serde_json::from_str::<Value>(&rendered).unwrap(), original);
    }
}

fn contexts() -> BoxedStrategy<tl3_controller::contracts::decision_context::DecisionContext> {
    use tl3_controller::contracts::{
        bounded::{List, Natural, Text},
        decision_context::{Decision, DecisionContext, SchemaVersion},
    };
    let nonempty = ".{1,20}".prop_map(|s| Text::new(s).unwrap());
    let scope = proptest::collection::vec(nonempty.clone(), 0..4)
        .prop_map(|values| List::new(values).unwrap());
    let question = proptest::collection::btree_map("[a-z]{1,8}", json_values(), 0..4)
        .prop_map(|map| map.into_iter().collect());
    let decision = (
        nonempty.clone(),
        proptest::option::of(".{0,20}"),
        question,
        nonempty.clone(),
        scope.clone(),
        scope,
        any::<u64>(),
        proptest::option::of(".{0,20}"),
        proptest::option::of(nonempty),
    )
        .prop_map(
            |(
                id,
                source_comment,
                question,
                answer,
                allowed_scope,
                restrictions,
                version,
                superseded_by,
                supersession_reason,
            )| Decision {
                id,
                source_comment,
                question,
                answer,
                allowed_scope,
                restrictions,
                origin_request_version: Natural::new(version).unwrap(),
                superseded_by,
                supersession_reason,
            },
        );
    (any::<u64>(), proptest::collection::vec(decision, 0..5))
        .prop_map(|(version, decisions)| DecisionContext {
            schema_version: SchemaVersion::default(),
            request_version: Natural::new(version).unwrap(),
            decisions: List::new(decisions).unwrap(),
        })
        .boxed()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    /// P-schema-sound (#819, #717): constructed contexts validate and round-trip.
    #[test]
    fn typed_decision_contexts_satisfy_the_schema_and_round_trip(context in contexts()) {
        use tl3_controller::contracts::decision_context::{self,DecisionContext};
        static VALIDATOR: std::sync::OnceLock<jsonschema::Validator> = std::sync::OnceLock::new();
        let validator = VALIDATOR.get_or_init(|| jsonschema::validator_for(&decision_context::schema()).unwrap());
        let json = serde_json::to_value(&context).unwrap();
        prop_assert!(validator.is_valid(&json), "{:?}", validator.iter_errors(&json).map(|e|e.to_string()).collect::<Vec<_>>());
        prop_assert_eq!(serde_json::from_value::<DecisionContext>(json).unwrap(), context);
    }
}

#[test]
fn decision_context_presence_and_numeric_spelling_match_the_schema() {
    use tl3_controller::contracts::decision_context::DecisionContext;
    let value = json!({"schema_version":1.0,"request_version":0.0,"decisions":[{
        "id":"decision", "source_comment":null,"question":{},"answer":"yes",
        "allowed_scope":[],"restrictions":[],"origin_request_version":2.0,"superseded_by":null
    }]});
    let parsed: DecisionContext = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(parsed).unwrap(), value);
    for field in ["source_comment", "superseded_by"] {
        let mut invalid = value.clone();
        invalid["decisions"][0]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(
            serde_json::from_value::<DecisionContext>(invalid).is_err(),
            "missing {field}"
        );
    }
    let mut invalid = value.clone();
    invalid["decisions"][0]["supersession_reason"] = Value::Null;
    assert!(serde_json::from_value::<DecisionContext>(invalid).is_err());
    let mut extra = value;
    extra["unexpected"] = true.into();
    assert!(serde_json::from_value::<DecisionContext>(extra).is_err());
}
