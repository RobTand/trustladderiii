//! #717/#730 C3: schemas, serde and the model must share one reply contract.
use serde_json::{Value, json};
use tl3_controller::contracts::{self, replies::RoleReply};

const ROLES: &[&str] = &[
    "audit",
    "classifier",
    "ingest",
    "maker",
    "reviewer",
    "writer",
    "postmortem",
];

fn reference(role: &str) -> Value {
    let text = match role {
        "audit" => include_str!("role_replies/reference/audit-reply.json"),
        "classifier" => include_str!("role_replies/reference/classifier-reply.json"),
        "ingest" => include_str!("role_replies/reference/ingest-reply.json"),
        "maker" => include_str!("role_replies/reference/maker-reply.json"),
        "reviewer" => include_str!("role_replies/reference/reviewer-reply.json"),
        "writer" => include_str!("role_replies/reference/writer-reply.json"),
        "postmortem" => include_str!("role_replies/reference/postmortem-reply.json"),
        _ => panic!("unknown reference role"),
    };
    serde_json::from_str(text).unwrap()
}

fn validator(role: &str) -> jsonschema::Validator {
    let mut registry = jsonschema::Registry::new();
    // Frozen refs include planner and inputs: WR1b and WR3 own their replacements.
    for (name, text) in [
        (
            "audit-input.json",
            include_str!("role_replies/reference/audit-input.json"),
        ),
        (
            "audit-reply.json",
            include_str!("role_replies/reference/audit-reply.json"),
        ),
        (
            "budget-wire.json",
            include_str!("role_replies/reference/budget-wire.json"),
        ),
        (
            "check-report.json",
            include_str!("role_replies/reference/check-report.json"),
        ),
        (
            "classifier-input.json",
            include_str!("role_replies/reference/classifier-input.json"),
        ),
        (
            "classifier-reply.json",
            include_str!("role_replies/reference/classifier-reply.json"),
        ),
        (
            "decision-context.json",
            include_str!("role_replies/reference/decision-context.json"),
        ),
        (
            "ingest-input.json",
            include_str!("role_replies/reference/ingest-input.json"),
        ),
        (
            "ingest-reply.json",
            include_str!("role_replies/reference/ingest-reply.json"),
        ),
        (
            "maker-input.json",
            include_str!("role_replies/reference/maker-input.json"),
        ),
        (
            "maker-reply.json",
            include_str!("role_replies/reference/maker-reply.json"),
        ),
        (
            "planner-input.json",
            include_str!("role_replies/reference/planner-input.json"),
        ),
        (
            "planner-reply.json",
            include_str!("role_replies/reference/planner-reply.json"),
        ),
        (
            "postmortem-input.json",
            include_str!("role_replies/reference/postmortem-input.json"),
        ),
        (
            "postmortem-reply.json",
            include_str!("role_replies/reference/postmortem-reply.json"),
        ),
        (
            "request.json",
            include_str!("role_replies/reference/request.json"),
        ),
        (
            "reviewer-input.json",
            include_str!("role_replies/reference/reviewer-input.json"),
        ),
        (
            "reviewer-reply.json",
            include_str!("role_replies/reference/reviewer-reply.json"),
        ),
        (
            "writer-input.json",
            include_str!("role_replies/reference/writer-input.json"),
        ),
        (
            "writer-reply.json",
            include_str!("role_replies/reference/writer-reply.json"),
        ),
    ] {
        let schema: Value = serde_json::from_str(text).unwrap();
        registry = registry
            .add(format!("{}{name}", contracts::SCHEMA_BASE), schema)
            .unwrap();
    }
    jsonschema::options()
        .with_registry(&registry.prepare().unwrap())
        .build(&reference(role))
        .unwrap()
}

/// #648: preserving the full schema also preserves model_facing's input.
#[test]
fn generated_reply_schemas_preserve_the_model_contract() {
    let generated = contracts::replies::generated();
    for role in ROLES {
        let file = generated
            .iter()
            .find(|file| file.path.ends_with(&format!("/{role}-reply.json")))
            .unwrap_or_else(|| panic!("{role} schema is not generated from its Rust type"));
        assert_eq!(
            serde_json::from_str::<Value>(&file.text).unwrap(),
            reference(role),
            "{role}"
        );
    }
}

/// #717: released reply fixtures must have the same schema and serde verdict.
#[test]
fn every_committed_reply_fixture_agrees_with_serde() {
    let corpus: Vec<Value> =
        serde_json::from_str(include_str!("role_replies/corpus.json")).unwrap();
    let mut accepted = 0;
    for role in ROLES {
        let validator = validator(role);
        for case in &corpus {
            let valid = validator.is_valid(&case["value"]);
            accepted += usize::from(valid);
            assert_eq!(
                RoleReply::decode(role, &case["value"]).is_ok(),
                valid,
                "{role}: {}: {}",
                case["source"],
                case["value"]
            );
        }
    }
    assert!(
        accepted >= 20,
        "the corpus must exercise accepted replies too"
    );
}

#[test]
fn unknown_roles_are_refused() {
    assert!(RoleReply::parse("unknown", &json!({})).is_err());
}

use proptest::{collection::vec, prelude::*, sample::select};

fn schema_strategy(
    schema: &Value,
    root: &Value,
    references: &std::collections::BTreeMap<String, Value>,
) -> BoxedStrategy<Value> {
    if let Some(reference) = schema["$ref"].as_str() {
        let (source, pointer) = reference.split_once('#').unwrap();
        let document = if source.is_empty() {
            root
        } else {
            &references[source]
        };
        let resolved = document.pointer(pointer).unwrap();
        // Sibling constraints, such as the writer's obstacle bound, apply too.
        let mut resolved = resolved.clone();
        for field in ["minLength", "maxLength"] {
            if let Some(bound) = schema.get(field) {
                resolved[field] = bound.clone();
            }
        }
        return schema_strategy(&resolved, document, references);
    }
    if let Some(value) = schema.get("const") {
        return Just(value.clone()).boxed();
    }
    if let Some(values) = schema["enum"].as_array() {
        return select(values.clone()).boxed();
    }
    if let Some(variants) = schema
        .get("oneOf")
        .or_else(|| schema.get("anyOf"))
        .and_then(Value::as_array)
    {
        return proptest::strategy::Union::new(
            variants
                .iter()
                .map(|s| schema_strategy(s, root, references)),
        )
        .boxed();
    }
    if let Some(types) = schema["type"].as_array() {
        let mut scalar = schema.clone();
        scalar["type"] = types.iter().find(|t| **t != "null").unwrap().clone();
        return prop_oneof![
            Just(Value::Null),
            schema_strategy(&scalar, root, references)
        ]
        .boxed();
    }
    if schema.as_object().is_some_and(|object| object.is_empty()) {
        return Just(json!("opaque patch value")).boxed();
    }
    match schema["type"].as_str().unwrap() {
        "null" => Just(Value::Null).boxed(),
        "boolean" => any::<bool>().prop_map(Value::from).boxed(),
        "string" => {
            let min = schema["minLength"].as_u64().unwrap_or(0) as usize;
            let max =
                (schema["maxLength"].as_u64().unwrap_or((min + 12) as u64) as usize).min(min + 12);
            let pattern = schema["pattern"].as_str().unwrap_or("");
            if pattern.contains("{40}") {
                return "[0-9a-f]{40}".prop_map(Value::from).boxed();
            }
            if pattern.contains("{64}") {
                return "[0-9a-f]{64}".prop_map(Value::from).boxed();
            }
            if pattern.contains("tool-error-") {
                return (1u32..1000)
                    .prop_map(|n| json!(format!("tool-error-{n}")))
                    .boxed();
            }
            if pattern.contains(".py") {
                return Just(json!("tests/a.py")).boxed();
            }
            // Nonblank scalar text and line restrictions all accept these letters.
            let min = if pattern.contains("\\S") {
                min.max(1)
            } else {
                min
            };
            proptest::string::string_regex(&format!("[a-zé]{{{min},{}}}", max.max(min)))
                .unwrap()
                .prop_map(Value::from)
                .boxed()
        }
        "integer" => {
            let min = schema["minimum"].as_i64().unwrap_or(-1000);
            let max = schema["maximum"]
                .as_i64()
                .unwrap_or(min.saturating_add(1000));
            (min..=max).prop_map(Value::from).boxed()
        }
        "number" => (0u32..1000)
            .prop_map(|n| json!(f64::from(n) / 10.0))
            .boxed(),
        "array" => {
            let min = schema["minItems"].as_u64().unwrap_or(0) as usize;
            let max =
                (schema["maxItems"].as_u64().unwrap_or((min + 2) as u64) as usize).min(min + 2);
            let unique = schema["uniqueItems"] == true;
            vec(
                schema_strategy(&schema["items"], root, references),
                min..=max,
            )
            .prop_filter("distinct wire items", move |items| {
                !unique
                    || items
                        .iter()
                        .enumerate()
                        .all(|(i, x)| !items[..i].contains(x))
            })
            .prop_map(Value::Array)
            .boxed()
        }
        "object" => {
            if let Some(properties) = schema["properties"].as_object() {
                let required = schema["required"].as_array().unwrap();
                let fields = properties
                    .iter()
                    .map(|(name, s)| {
                        let value = schema_strategy(s, root, references);
                        let field = if required.contains(&json!(name)) {
                            value.prop_map(Some).boxed()
                        } else {
                            proptest::option::of(value).boxed()
                        };
                        (name.clone(), field)
                    })
                    .collect::<Vec<_>>();
                let (keys, values): (Vec<_>, Vec<_>) = fields.into_iter().unzip();
                values
                    .prop_map(move |values| {
                        Value::Object(
                            keys.iter()
                                .cloned()
                                .zip(values)
                                .filter_map(|(k, v)| v.map(|v| (k, v)))
                                .collect(),
                        )
                    })
                    .boxed()
            } else {
                let item = schema_strategy(&schema["additionalProperties"], root, references);
                proptest::collection::btree_map("[a-z]{1,5}", item, 0..3)
                    .prop_map(|map| json!(map))
                    .boxed()
            }
        }
        kind => panic!("unsupported schema strategy: {kind}"),
    }
}

/// #717: serialized typed replies satisfy their generated schemas, and decoding
/// serialized values preserves every typed variant and bounded field.
#[test]
fn arbitrary_typed_replies_round_trip_and_validate() {
    use proptest::test_runner::{Config, RngSeed, TestRunner};
    let references: std::collections::BTreeMap<String, Value> = reference_documents();
    for role in ROLES {
        let schema = reference(role);
        let validator = validator(role);
        let strategy = schema_strategy(&schema, &schema, &references)
            .prop_filter("cross-field wire conditions", move |v| {
                validator.is_valid(v)
            });
        let validator = validator_for_generated(role, &references);
        let mut runner = TestRunner::new(Config {
            cases: 128,
            rng_seed: RngSeed::Fixed(730),
            failure_persistence: None,
            ..Config::default()
        });
        runner
            .run(&strategy, |value| {
                let typed = RoleReply::decode(role, &value)
                    .map_err(|error| TestCaseError::fail(format!("{role}: {error}: {value}")))?;
                let serialized = serde_json::to_value(&typed).unwrap();
                prop_assert!(validator.is_valid(&serialized), "{role}: {serialized}");
                prop_assert_eq!(RoleReply::decode(role, &serialized).unwrap(), typed);
                prop_assert_eq!(serialized, value);
                Ok(())
            })
            .unwrap_or_else(|error| panic!("{role}: {error}"));
    }
}

fn reference_documents() -> std::collections::BTreeMap<String, Value> {
    [
        (
            "audit-input.json",
            include_str!("role_replies/reference/audit-input.json"),
        ),
        (
            "audit-reply.json",
            include_str!("role_replies/reference/audit-reply.json"),
        ),
        (
            "budget-wire.json",
            include_str!("role_replies/reference/budget-wire.json"),
        ),
        (
            "check-report.json",
            include_str!("role_replies/reference/check-report.json"),
        ),
        (
            "classifier-input.json",
            include_str!("role_replies/reference/classifier-input.json"),
        ),
        (
            "classifier-reply.json",
            include_str!("role_replies/reference/classifier-reply.json"),
        ),
        (
            "decision-context.json",
            include_str!("role_replies/reference/decision-context.json"),
        ),
        (
            "ingest-input.json",
            include_str!("role_replies/reference/ingest-input.json"),
        ),
        (
            "ingest-reply.json",
            include_str!("role_replies/reference/ingest-reply.json"),
        ),
        (
            "maker-input.json",
            include_str!("role_replies/reference/maker-input.json"),
        ),
        (
            "maker-reply.json",
            include_str!("role_replies/reference/maker-reply.json"),
        ),
        (
            "planner-input.json",
            include_str!("role_replies/reference/planner-input.json"),
        ),
        (
            "planner-reply.json",
            include_str!("role_replies/reference/planner-reply.json"),
        ),
        (
            "postmortem-input.json",
            include_str!("role_replies/reference/postmortem-input.json"),
        ),
        (
            "postmortem-reply.json",
            include_str!("role_replies/reference/postmortem-reply.json"),
        ),
        (
            "request.json",
            include_str!("role_replies/reference/request.json"),
        ),
        (
            "reviewer-input.json",
            include_str!("role_replies/reference/reviewer-input.json"),
        ),
        (
            "reviewer-reply.json",
            include_str!("role_replies/reference/reviewer-reply.json"),
        ),
        (
            "writer-input.json",
            include_str!("role_replies/reference/writer-input.json"),
        ),
        (
            "writer-reply.json",
            include_str!("role_replies/reference/writer-reply.json"),
        ),
    ]
    .into_iter()
    .map(|(name, text)| (name.into(), serde_json::from_str(text).unwrap()))
    .collect()
}

fn validator_for_generated(
    role: &str,
    references: &std::collections::BTreeMap<String, Value>,
) -> jsonschema::Validator {
    let mut registry = jsonschema::Registry::new();
    for (name, schema) in references {
        registry = registry
            .add(format!("{}{name}", contracts::SCHEMA_BASE), schema.clone())
            .unwrap();
    }
    for file in contracts::replies::generated() {
        let schema: Value = serde_json::from_str(&file.text).unwrap();
        registry = registry
            .add(schema["$id"].as_str().unwrap().to_owned(), schema)
            .unwrap();
    }
    let file = contracts::replies::generated()
        .into_iter()
        .find(|file| file.path.ends_with(&format!("/{role}-reply.json")))
        .unwrap();
    jsonschema::options()
        .with_registry(&registry.prepare().unwrap())
        .build(&serde_json::from_str::<Value>(&file.text).unwrap())
        .unwrap()
}
