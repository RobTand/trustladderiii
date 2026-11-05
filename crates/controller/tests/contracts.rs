//! #741: the Rust contract types are the one source of the harness check report's schema.
//!
//! - The committed schema files are exactly what the types render.
//! - Every shared harness vector (#734) parses into the typed report and serializes back
//!   unchanged, and the generated schema accepts it.
//! - The plan declarations the report echoes are the planner's own `$defs`.
//! - proptest: schema-valid JSON parses, round-trips byte for byte, and the typed value
//!   validates against the generated schema.

use proptest::{
    collection::{btree_map, btree_set, vec},
    prelude::*,
    sample::select,
    test_runner::RngSeed,
};
use serde_json::{Map, Value, json};
use std::path::Path;
use tl3_controller::contracts::{self, Check, CheckReport, CheckResult};

const FIX: &str = "bazel run --config=remote //crates/controller:tl3-contracts-schemas";

// The committed generated files, compiled in so the check stays hermetic.
const COMMITTED: &[(&str, &str)] = &[
    (
        "workers/src/tl3_workers/schemas/check-report.json",
        include_str!("../../../workers/src/tl3_workers/schemas/check-report.json"),
    ),
    (
        "workers/src/tl3_workers/schemas/ingest-reply.json",
        include_str!("../../../workers/src/tl3_workers/schemas/ingest-reply.json"),
    ),
    (
        "workers/src/tl3_workers/schemas/classifier-reply.json",
        include_str!("../../../workers/src/tl3_workers/schemas/classifier-reply.json"),
    ),
    (
        "workers/src/tl3_workers/schemas/maker-reply.json",
        include_str!("../../../workers/src/tl3_workers/schemas/maker-reply.json"),
    ),
    (
        "workers/src/tl3_workers/schemas/writer-reply.json",
        include_str!("../../../workers/src/tl3_workers/schemas/writer-reply.json"),
    ),
];

#[test]
fn generated_schemas_match_committed() {
    let generated = contracts::generated();
    assert_eq!(
        generated
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        COMMITTED.iter().map(|(path, _)| *path).collect::<Vec<_>>(),
        "every generated file is committed and compiled into this test"
    );
    for (file, (path, committed)) in generated.iter().zip(COMMITTED) {
        if file.text == *committed {
            continue;
        }
        let line = file
            .text
            .lines()
            .zip(committed.lines())
            .position(|(rendered, written)| rendered != written)
            .unwrap_or_else(|| file.text.lines().count().min(committed.lines().count()));
        panic!(
            "{path} no longer matches the Rust contract types (first difference at line {}: \
             rendered {:?}, committed {:?}). Never edit it by hand; regenerate with: {FIX}",
            line + 1,
            file.text.lines().nth(line),
            committed.lines().nth(line),
        );
    }
}

fn check_report_validator(definition: Option<&str>) -> jsonschema::Validator {
    let mut schema = contracts::check_report_schema();
    if let Some(definition) = definition {
        schema = json!({
            "$schema": schema["$schema"],
            "$defs": schema["$defs"],
            "$ref": format!("#/$defs/{definition}"),
        });
    }
    jsonschema::validator_for(&schema).expect("the generated schema compiles")
}

fn vectors() -> Vec<(String, Value)> {
    let directory =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../harness/test-vectors/contracts");
    let mut paths: Vec<_> = std::fs::read_dir(&directory)
        .unwrap_or_else(|error| panic!("{}: {error}", directory.display()))
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect();
    paths.sort();
    assert!(paths.len() >= 7, "harness contract vectors are missing");
    paths
        .into_iter()
        .map(|path| {
            let name = path.file_stem().unwrap().to_string_lossy().into_owned();
            (
                name,
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap(),
            )
        })
        .collect()
}

#[test]
fn every_harness_vector_parses_into_the_typed_report_and_back() {
    let validator = check_report_validator(None);
    for (name, vector) in vectors() {
        let request = &vector["request"];
        // The envelope the trusted action prints around result_json (action.py).
        let envelope = json!({"schema": "tl3.checks.v1", "effect_key": request["effect_key"],
            "base": request["base"], "head": request["head"], "reviewed": request["reviewed"],
            "results": vector["results"]});
        let report: CheckReport = serde_json::from_value(envelope.clone())
            .unwrap_or_else(|error| panic!("{name}: the typed report refuses the vector: {error}"));
        assert_eq!(
            serde_json::to_value(&report).unwrap(),
            envelope,
            "{name}: the typed report must keep every key and value the harness reports"
        );
        let errors: Vec<_> = validator
            .iter_errors(&envelope)
            .map(|error| format!("{}: {error}", error.instance_path()))
            .collect();
        assert!(
            errors.is_empty(),
            "{name}: check-report.json rejects the vector: {errors:?}"
        );
        let plan: Vec<Check> = serde_json::from_value(request["plan"].clone())
            .unwrap_or_else(|error| panic!("{name}: the typed plan refuses the request: {error}"));
        assert_eq!(
            serde_json::to_value(&plan).unwrap(),
            request["plan"],
            "{name}"
        );
    }
}

// Local references inlined, annotations and no-op keywords removed, so equal schemas
// compare equal however they were written. Each rule keeps the accepted set unchanged.
fn normalized(value: &Value, defs: &Value) -> Value {
    match value {
        // A local reference stands for its definition.
        Value::Object(object)
            if object
                .get("$ref")
                .and_then(Value::as_str)
                .is_some_and(|reference| reference.starts_with("#/$defs/")) =>
        {
            assert_eq!(object.len(), 1, "a reference with siblings: {value}");
            let name = object["$ref"]
                .as_str()
                .unwrap()
                .trim_start_matches("#/$defs/");
            normalized(&defs[name], defs)
        }
        Value::Object(object) => {
            let mut result = Map::new();
            for (key, item) in object {
                let no_op = matches!(key.as_str(), "description" | "title")
                    || (matches!(key.as_str(), "minItems" | "minLength" | "minProperties")
                        && item == &json!(0))
                    || (key == "required" && item == &json!([]));
                // `type` beside an enum or const whose every value already has that type.
                let implied_type = key == "type"
                    && object
                        .get("enum")
                        .and_then(Value::as_array)
                        .cloned()
                        .or_else(|| object.get("const").map(|value| vec![value.clone()]))
                        .is_some_and(|values| {
                            values.iter().all(|value| match item.as_str() {
                                Some("string") => value.is_string(),
                                Some("integer") => value.is_u64() || value.is_i64(),
                                Some("boolean") => value.is_boolean(),
                                _ => false,
                            })
                        });
                if no_op || implied_type {
                    continue;
                }
                let mut item = normalized(item, defs);
                if key == "required"
                    && let Value::Array(names) = &mut item
                {
                    names.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
                }
                result.insert(key.clone(), item);
            }
            Value::Object(result)
        }
        Value::Array(items) => {
            Value::Array(items.iter().map(|item| normalized(item, defs)).collect())
        }
        other => other.clone(),
    }
}

#[test]
fn plan_declarations_are_the_planners_definitions() {
    let planner: Value = serde_json::from_str(include_str!(
        "../../../workers/src/tl3_workers/schemas/planner-reply.json"
    ))
    .unwrap();
    let generated = contracts::check_report_schema();
    for (ours, planners) in [
        ("Forcing", "forcing"),
        ("ProofDeclaration", "proof"),
        ("DeclaredSymptom", "declared_symptom"),
    ] {
        assert_eq!(
            normalized(&generated["$defs"][ours], &generated["$defs"]),
            normalized(&planner["$defs"][planners], &planner["$defs"]),
            "check-report.json#/$defs/{ours} must accept exactly what \
             planner-reply.json#/$defs/{planners} accepts: the harness echoes the plan's declaration"
        );
    }
}

// JSON-level strategies for schema-valid documents.

fn text(min: usize, max: usize) -> BoxedStrategy<Value> {
    let longest = max.min(min + 24);
    let short = proptest::string::string_regex(&format!("[a-zé _.:]{{{min},{longest}}}")).unwrap();
    if max <= 1000 && max > longest {
        // Sometimes exactly at the bound, counted in characters.
        prop_oneof![3 => short, 1 => Just("é".repeat(max))]
            .prop_map(Value::from)
            .boxed()
    } else {
        short.prop_map(Value::from).boxed()
    }
}

fn pattern(regex: &str) -> BoxedStrategy<Value> {
    proptest::string::string_regex(regex)
        .unwrap()
        .prop_map(Value::from)
        .boxed()
}

fn sha256() -> BoxedStrategy<Value> {
    pattern("[0-9a-f]{64}")
}

fn commit() -> BoxedStrategy<Value> {
    prop_oneof![pattern("[0-9a-f]{40}"), pattern("[0-9a-f]{64}")].boxed()
}

fn nullable(strategy: BoxedStrategy<Value>) -> BoxedStrategy<Value> {
    prop_oneof![1 => Just(Value::Null), 3 => strategy].boxed()
}

fn one_of(values: &[&'static str]) -> BoxedStrategy<Value> {
    select(values.to_vec()).prop_map(Value::from).boxed()
}

// Integral numbers in every form the schema's `integer` accepts: small, the largest u64,
// and integral floats such as `3.0` (#750 review finding 3).
fn integer(range: std::ops::RangeInclusive<u64>) -> BoxedStrategy<Value> {
    let (low, high) = (*range.start(), *range.end());
    prop_oneof![
        4 => range.prop_map(Value::from),
        1 => Just(Value::from(high)),
        1 => (low..=high.min(1 << 53)).prop_map(|value| json!(value as f64)),
    ]
    .boxed()
}

fn count() -> BoxedStrategy<Value> {
    prop_oneof![3 => integer(0..=999), 1 => integer(0..=u64::MAX)].boxed()
}

fn list(item: BoxedStrategy<Value>, size: std::ops::RangeInclusive<usize>) -> BoxedStrategy<Value> {
    vec(item, size).prop_map(Value::Array).boxed()
}

// Mostly short lists, sometimes exactly the contract's maximum (#750 review finding 4).
fn list_up_to(item: BoxedStrategy<Value>, min: usize, max: usize) -> BoxedStrategy<Value> {
    prop_oneof![
        4 => list(item.clone(), min..=max.min(min + 3)),
        1 => list(item, max..=max),
    ]
    .boxed()
}

fn map(
    key: BoxedStrategy<Value>,
    item: BoxedStrategy<Value>,
    size: std::ops::RangeInclusive<usize>,
) -> BoxedStrategy<Value> {
    btree_map(
        key.prop_map(|key| key.as_str().unwrap().to_owned()),
        item,
        size,
    )
    .prop_map(|entries| Value::Object(entries.into_iter().collect()))
    .boxed()
}

type Fields = Vec<(&'static str, BoxedStrategy<Value>)>;

fn object(required: Fields, optional: Fields) -> BoxedStrategy<Value> {
    let (required_keys, required): (Vec<_>, Vec<_>) = required.into_iter().unzip();
    let (optional_keys, optional): (Vec<_>, Vec<_>) = optional.into_iter().unzip();
    let optional: Vec<_> = optional
        .into_iter()
        .map(|strategy| proptest::option::of(strategy).boxed())
        .collect();
    (required, optional)
        .prop_map(move |(required, optional)| {
            let mut object = Map::new();
            for (key, value) in required_keys.iter().zip(required) {
                object.insert((*key).into(), value);
            }
            for (key, value) in optional_keys.iter().zip(optional) {
                if let Some(value) = value {
                    object.insert((*key).into(), value);
                }
            }
            Value::Object(object)
        })
        .boxed()
}

fn node() -> BoxedStrategy<Value> {
    pattern("tests/test_[a-z]{1,5}\\.py::test_[a-z]{1,5}")
}

fn unit_path() -> BoxedStrategy<Value> {
    pattern("[a-z]{1,6}(/[a-z]{1,6}){0,2}\\.py(::[A-Za-z_][A-Za-z0-9_]{0,5}){0,2}")
}

fn dotted() -> BoxedStrategy<Value> {
    pattern("[A-Za-z_][A-Za-z0-9_]{0,5}(\\.[A-Za-z_][A-Za-z0-9_]{0,5}){1,2}")
}

fn outcome() -> BoxedStrategy<Value> {
    one_of(&[
        "passed",
        "assertion",
        "added-name",
        "error",
        "skipped",
        "missing",
        "uncollected",
        "no-report",
    ])
}

fn seconds(max: u64) -> BoxedStrategy<Value> {
    prop_oneof![
        (1..=max).prop_map(Value::from),
        (0.001f64..max as f64).prop_map(|seconds| json!(seconds)),
    ]
    .boxed()
}

// An applied delay may be zero, as an integer or a float, and may reach the 10 s total.
fn applied_seconds() -> BoxedStrategy<Value> {
    prop_oneof![
        3 => seconds(10),
        1 => select(vec![json!(0), json!(0.0), json!(10), json!(10.0)]),
    ]
    .boxed()
}

fn delay_budget() -> BoxedStrategy<Value> {
    object(
        vec![
            ("applied_seconds", applied_seconds()),
            ("exhausted", any::<bool>().prop_map(Value::from).boxed()),
        ],
        vec![],
    )
}

fn scalar() -> BoxedStrategy<Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::from),
        any::<i64>().prop_map(Value::from),
        text(0, 12),
        vec(any::<i32>().prop_map(Value::from), 0..3).prop_map(Value::Array),
    ]
    .boxed()
}

fn forcing() -> BoxedStrategy<Value> {
    let mode = prop_oneof![
        scalar().prop_map(|value| ("value", value)),
        scalar().prop_map(|value| ("returns", value)),
        text(1, 20).prop_map(|value| ("raises", value)),
        seconds(5).prop_map(|value| ("delay_seconds", value)),
    ];
    let patch =
        (dotted(), mode).prop_map(|(target, (key, value))| json!({"target": target, key: value}));
    object(
        vec![
            ("symptom", text(1, 1000)),
            (
                "environment",
                map(pattern("[A-Z_][A-Z0-9_]{0,10}"), text(0, 1000), 0..=8),
            ),
            ("patches", list_up_to(patch.boxed(), 0, 8)),
        ],
        vec![],
    )
}

// Distinct proof units, sometimes exactly the 16 a declaration allows.
fn units() -> BoxedStrategy<Value> {
    let unit = || unit_path().prop_map(|unit| unit.as_str().unwrap().to_owned());
    prop_oneof![4 => btree_set(unit(), 0..=4), 1 => btree_set(unit(), 16..=16)]
        .prop_map(|units| units.into_iter().map(Value::from).collect())
        .boxed()
}

fn proof_declaration() -> BoxedStrategy<Value> {
    object(
        vec![
            ("subjects", units()),
            ("observer_support", units()),
            ("red_message", text(1, 160)),
        ],
        vec![],
    )
}

fn declared_symptom() -> BoxedStrategy<Value> {
    object(
        vec![
            ("node", node()),
            ("exception", dotted()),
            ("phase", one_of(&["call", "setup"])),
            ("repeat", integer(1..=10)),
            (
                "parallel",
                select(vec![
                    json!(0),
                    json!(2),
                    json!(3),
                    json!(4),
                    json!(0.0),
                    json!(4.0),
                ])
                .boxed(),
            ),
        ],
        vec![],
    )
}

fn phase_fact() -> BoxedStrategy<Value> {
    object(
        vec![
            ("outcome", one_of(&["passed", "failed", "skipped"])),
            ("exception", text(0, 200)),
            ("assertion", any::<bool>().prop_map(Value::from).boxed()),
        ],
        vec![],
    )
}

fn phases() -> BoxedStrategy<Value> {
    let run = object(
        vec![
            ("test", node()),
            ("setup", phase_fact()),
            ("teardown", phase_fact()),
        ],
        vec![("call", phase_fact())],
    );
    object(
        vec![],
        vec![
            ("base", run.clone()),
            ("head", run.clone()),
            ("head_forced", run),
        ],
    )
    .prop_filter("phase evidence needs a side", |phases| {
        !phases.as_object().unwrap().is_empty()
    })
    .boxed()
}

fn test_result() -> BoxedStrategy<Value> {
    object(
        vec![
            ("test", node()),
            ("base", outcome()),
            ("head", outcome()),
            ("pre_existing", any::<bool>().prop_map(Value::from).boxed()),
            ("passed", any::<bool>().prop_map(Value::from).boxed()),
            ("message", text(0, contracts::MESSAGE_CHARS)),
        ],
        vec![("head_forced", outcome()), ("phases", phases())],
    )
}

fn proof_receipt() -> BoxedStrategy<Value> {
    let digests = object(
        vec![("base", nullable(sha256())), ("head", nullable(sha256()))],
        vec![],
    );
    object(
        vec![
            ("rule", Just(json!("subject-observer-v1")).boxed()),
            ("declaration", nullable(proof_declaration())),
            ("mode", one_of(&["head-observer", "pytest-outcome"])),
            ("subjects", map(unit_path(), digests, 0..=3)),
            ("observer_sha256", map(node(), nullable(sha256()), 0..=3)),
            ("support_sha256", map(unit_path(), sha256(), 0..=3)),
            ("source_base_tree_sha256", sha256()),
            ("base_tree_sha256", sha256()),
            ("head_tree_sha256", sha256()),
        ],
        vec![
            (
                "red_message_matches",
                map(node(), one_of(&["message", "source-assertion"]), 0..=3),
            ),
            (
                "delay_budget",
                map(
                    one_of(&["base", "head", "head_forced"]),
                    map(node(), delay_budget(), 0..=2),
                    0..=3,
                ),
            ),
        ],
    )
}

fn selection() -> BoxedStrategy<Value> {
    let names = || list(text(1, 20), 0..=3);
    let unit = object(
        vec![
            ("members", names()),
            ("cases", count()),
            ("outcomes", map(text(1, 20), outcome(), 0..=3)),
        ],
        vec![],
    );
    let saturation = object(
        vec![
            ("unit", names()),
            ("cases", nullable(count())),
            ("remaining", count()),
            ("reason", text(0, 80)),
        ],
        vec![],
    );
    object(
        vec![
            ("policy", text(0, 80)),
            ("limit", count()),
            ("charged_cases", count()),
            ("base_units", list(unit, 0..=2)),
            ("head_units", list(names(), 0..=2)),
            ("skipped_units", list(names(), 0..=2)),
            (
                "skipped_checked",
                any::<bool>().prop_map(Value::from).boxed(),
            ),
            ("saturation", saturation),
        ],
        vec![],
    )
}

fn event(name: &'static str, required: Fields, optional: Fields) -> BoxedStrategy<Value> {
    let mut required = required;
    required.insert(0, ("event", Just(Value::from(name)).boxed()));
    object(required, optional)
}

fn symptom_record() -> BoxedStrategy<Value> {
    let missing = || {
        nullable(list(
            prop_oneof![
                text(0, 12),
                (text(0, 12), text(0, 12)).prop_map(|(module, name)| json!([module, name])),
            ]
            .boxed(),
            0..=2,
        ))
    };
    let refusal = object(
        vec![
            ("phase", Just(json!("forcing_validation")).boxed()),
            ("target", text(0, 40)),
            ("resolved_module", text(0, 40)),
            ("owner_category", text(0, 40)),
            ("reason", text(0, 40)),
        ],
        vec![],
    );
    let failure = object(
        vec![
            ("exception", text(1, 300)),
            ("phase", one_of(&["setup", "call", "teardown"])),
            ("source", any::<bool>().prop_map(Value::from).boxed()),
        ],
        vec![],
    );
    let saturation = object(
        vec![
            ("unit", list(text(1, 20), 1..=2)),
            ("cases", count()),
            ("remaining", count()),
        ],
        vec![],
    );
    prop_oneof![
        // JSON Schema's `const: 1` is met by 1.0 too (#750 re-review finding 5).
        event(
            "start",
            vec![("schema", select(vec![json!(1), json!(1.0)]).boxed())],
            vec![]
        ),
        event(
            "finish",
            vec![(
                "exit_code",
                prop_oneof![
                    any::<i64>().prop_map(Value::from),
                    (-1000i32..1000).prop_map(|code| json!(f64::from(code))),
                ]
                .boxed(),
            )],
            vec![]
        ),
        event(
            "collection",
            vec![("tests", list(node(), 0..=3))],
            vec![("saturation", saturation)]
        ),
        event(
            "result",
            vec![
                ("test", node()),
                ("outcome", one_of(&["passed", "failed", "error", "skipped"])),
                ("message", text(0, contracts::MESSAGE_CHARS)),
                ("truncated", any::<bool>().prop_map(Value::from).boxed()),
            ],
            vec![
                ("failures", list(failure, 0..=3)),
                ("target_hits", map(dotted(), count(), 0..=2)),
                ("delay_budget", delay_budget()),
                ("forcing_tampered", Just(json!(true)).boxed()),
                ("forcing_refusal", refusal),
            ],
        ),
        event(
            "phase",
            vec![
                ("test", node()),
                ("when", one_of(&["setup", "call", "teardown"])),
                ("outcome", one_of(&["passed", "failed", "skipped"])),
                ("assertion", any::<bool>().prop_map(Value::from).boxed()),
                ("missing", missing()),
            ],
            vec![
                ("exception", text(0, 300)),
                ("exception_type", text(0, 200)),
                ("message", text(0, contracts::MESSAGE_CHARS)),
            ],
        ),
        event(
            "collection-error",
            vec![],
            vec![
                ("test", text(0, 20)),
                ("missing", missing()),
                ("exception", text(0, 300)),
                ("message", text(0, contracts::MESSAGE_CHARS)),
                ("truncated", any::<bool>().prop_map(Value::from).boxed()),
            ],
        ),
        event(
            "unavailable",
            vec![("message", text(0, contracts::MESSAGE_CHARS))],
            vec![]
        ),
        event(
            "plan-error",
            vec![("message", text(0, contracts::MESSAGE_CHARS))],
            vec![]
        ),
    ]
    .boxed()
}

fn symptom_evidence() -> BoxedStrategy<Value> {
    let run = object(
        vec![
            ("side", one_of(&["base", "head", "ordinary-head"])),
            ("run", integer(1..=10)),
            (
                "outcome",
                one_of(&["passed", "symptom", "unrelated", "unavailable"]),
            ),
            ("sha256", sha256()),
            ("records", list(symptom_record(), 0..=4)),
        ],
        vec![("target_hits", map(dotted(), count(), 0..=2))],
    );
    object(
        vec![
            ("base", commit()),
            ("head", commit()),
            ("forcing", nullable(forcing())),
            ("declaration", declared_symptom()),
            ("runs", list_up_to(run, 0, 30)),
        ],
        vec![],
    )
}

fn check_result() -> BoxedStrategy<Value> {
    object(
        vec![
            ("name", text(1, 40)),
            (
                "kind",
                one_of(&[
                    "demonstrating",
                    "regression",
                    "behavior-preserving",
                    "reproducing",
                ]),
            ),
            (
                "verdict",
                one_of(&["pass", "fail", "infrastructure-failure"]),
            ),
            ("custody", Just(json!("in-process")).boxed()),
            ("tests", list(test_result(), 1..=3)),
        ],
        vec![
            ("selection", selection()),
            ("forcing", forcing()),
            ("proof", proof_receipt()),
            ("declared_symptom", declared_symptom()),
            ("symptom_evidence", symptom_evidence()),
        ],
    )
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        // Bazel's source tree is read-only, and a fixed seed keeps the Gate repeatable.
        failure_persistence: None,
        rng_seed: RngSeed::Fixed(741),
        ..ProptestConfig::default()
    })]

    #[test]
    fn typed_results_round_trip_and_satisfy_the_generated_schema(document in check_result()) {
        static VALIDATOR: std::sync::OnceLock<jsonschema::Validator> = std::sync::OnceLock::new();
        let validator = VALIDATOR.get_or_init(|| check_report_validator(Some("CheckResult")));
        let errors: Vec<_> = validator
            .iter_errors(&document)
            .map(|error| format!("{}: {error}", error.instance_path()))
            .collect();
        prop_assert!(errors.is_empty(), "the strategy produced a document the schema rejects: {errors:?}");
        let typed: CheckResult = serde_json::from_value(document.clone())
            .map_err(|error| TestCaseError::fail(format!("schema-valid document refused: {error}")))?;
        let serialized = serde_json::to_value(&typed).unwrap();
        prop_assert_eq!(&serialized, &document);
        prop_assert!(validator.is_valid(&serialized));
        let reparsed: CheckResult = serde_json::from_value(serialized).unwrap();
        prop_assert_eq!(reparsed, typed);
    }
}

/// Typed values built only through the contract constructors (#750 review finding 4):
/// whatever a caller can construct must serialize to a document the schema accepts.
mod typed {
    use super::*;
    use serde::de::DeserializeOwned;
    use serde_json::Number;
    use std::collections::BTreeMap;
    use tl3_controller::contracts::{bounded::*, checks::*, report::*};

    fn string(min: usize, max: usize) -> BoxedStrategy<String> {
        let longest = max.min(min + 24);
        let short =
            proptest::string::string_regex(&format!("[a-zé _.:]{{{min},{longest}}}")).unwrap();
        if max <= 1000 && max > longest {
            prop_oneof![3 => short, 1 => Just("é".repeat(max))].boxed()
        } else {
            short.boxed()
        }
    }

    fn regex(pattern: &str) -> BoxedStrategy<String> {
        proptest::string::string_regex(pattern).unwrap().boxed()
    }

    // Only for fields whose Rust type this change replaces: build them from the wire form,
    // through the field type's own Deserialize.
    fn wire<T: DeserializeOwned + std::fmt::Debug>(json: BoxedStrategy<Value>) -> BoxedStrategy<T> {
        json.prop_map(|value| serde_json::from_value(value).unwrap())
            .boxed()
    }

    fn text<const MIN: usize, const MAX: usize>() -> BoxedStrategy<Text<MIN, MAX>> {
        string(MIN, MAX)
            .prop_map(|text| Text::new(text).unwrap())
            .boxed()
    }

    fn sha256() -> BoxedStrategy<Sha256Hex> {
        regex("[0-9a-f]{64}")
            .prop_map(|sha| Sha256Hex::new(sha).unwrap())
            .boxed()
    }

    fn commit() -> BoxedStrategy<CommitSha> {
        prop_oneof![regex("[0-9a-f]{40}"), regex("[0-9a-f]{64}")]
            .prop_map(|sha| CommitSha::new(sha).unwrap())
            .boxed()
    }

    fn node() -> BoxedStrategy<String> {
        regex("tests/test_[a-z]{1,5}\\.py::test_[a-z]{1,5}")
    }

    fn unit() -> BoxedStrategy<UnitPath> {
        regex("[a-z]{1,6}(/[a-z]{1,6}){0,2}\\.py(::[A-Za-z_][A-Za-z0-9_]{0,5}){0,2}")
            .prop_map(|unit| UnitPath::new(unit).unwrap())
            .boxed()
    }

    fn dotted() -> BoxedStrategy<String> {
        regex("[A-Za-z_][A-Za-z0-9_]{0,5}(\\.[A-Za-z_][A-Za-z0-9_]{0,5}){1,2}")
    }

    fn integer<const MIN: u64, const MAX: u64>() -> BoxedStrategy<Integer<MIN, MAX>> {
        prop_oneof![
            4 => (MIN..=MAX).prop_map(|value| Integer::new(value).unwrap()),
            1 => Just(Integer::new(MAX).unwrap()),
            1 => (MIN..=MAX.min(1 << 53)).prop_map(|value| {
                Integer::from_number(Number::from_f64(value as f64).unwrap()).unwrap()
            }),
        ]
        .boxed()
    }

    fn list<T, const MIN: usize, const MAX: usize>(
        item: BoxedStrategy<T>,
    ) -> BoxedStrategy<List<T, MIN, MAX>>
    where
        T: Clone + PartialEq + std::fmt::Debug + 'static,
    {
        let edge = if MAX <= 30 { MAX } else { MIN };
        prop_oneof![
            4 => vec(item.clone(), MIN..=MAX.min(MIN + 3)),
            1 => vec(item, edge..=edge),
        ]
        .prop_map(|items| List::new(items).unwrap())
        .boxed()
    }

    fn units() -> BoxedStrategy<List<UnitPath, 0, 16, true>> {
        prop_oneof![4 => btree_set(unit(), 0..=4), 1 => btree_set(unit(), 16..=16)]
            .prop_map(|units| List::new(units.into_iter().collect()).unwrap())
            .boxed()
    }

    fn outcome() -> BoxedStrategy<Outcome> {
        select(vec![
            Outcome::Passed,
            Outcome::Assertion,
            Outcome::AddedName,
            Outcome::Error,
            Outcome::Skipped,
            Outcome::Missing,
            Outcome::Uncollected,
            Outcome::NoReport,
        ])
        .boxed()
    }

    fn number(value: f64) -> Number {
        Number::from_f64(value).unwrap()
    }

    fn patch() -> BoxedStrategy<Patch> {
        let target = dotted().prop_map(|target| PatchTarget::new(target).unwrap());
        let mode = prop_oneof![
            super::scalar().prop_map(PatchMode::Value),
            super::scalar().prop_map(PatchMode::Returns),
            text::<1, UNBOUNDED>().prop_map(PatchMode::Raises),
            prop_oneof![
                (1u64..=5).prop_map(Number::from),
                (0.001f64..5.0).prop_map(number),
            ]
            .prop_map(|seconds| PatchMode::DelaySeconds(DelaySeconds::new(seconds).unwrap())),
        ];
        (target, mode)
            .prop_map(|(target, mode)| Patch { target, mode })
            .boxed()
    }

    fn forcing() -> BoxedStrategy<Forcing> {
        let name =
            regex("[A-Z_][A-Z0-9_]{0,10}").prop_map(|name| EnvironmentName::new(name).unwrap());
        let environment = prop_oneof![
            4 => btree_map(name.clone(), text::<0, 1000>(), 0..=3),
            1 => btree_map(name, text::<0, 1000>(), 8..=8),
        ]
        .prop_map(|entries| Object::new(entries).unwrap());
        (text::<1, 1000>(), environment, list::<Patch, 0, 8>(patch()))
            .prop_map(|(symptom, environment, patches)| Forcing {
                symptom,
                environment,
                patches,
            })
            .boxed()
    }

    fn proof_declaration() -> BoxedStrategy<ProofDeclaration> {
        (units(), units(), text::<1, 160>())
            .prop_map(
                |(subjects, observer_support, red_message)| ProofDeclaration {
                    subjects,
                    observer_support,
                    red_message,
                },
            )
            .boxed()
    }

    fn declared_symptom() -> BoxedStrategy<DeclaredSymptom> {
        let parallel = select(vec![0.0, 2.0, 3.0, 4.0])
            .prop_flat_map(|value| {
                prop_oneof![Just(Number::from(value as u64)), Just(number(value))]
            })
            .prop_map(|value| Parallel::from_number(value).unwrap());
        (
            node(),
            dotted(),
            select(vec![SymptomPhase::Call, SymptomPhase::Setup]),
            integer::<1, 10>(),
            parallel,
        )
            .prop_map(
                |(node, exception, phase, repeat, parallel)| DeclaredSymptom {
                    node: Text::new(node).unwrap(),
                    exception: ExceptionName::new(exception).unwrap(),
                    phase,
                    repeat,
                    parallel,
                },
            )
            .boxed()
    }

    fn phase_fact() -> BoxedStrategy<PhaseFact> {
        (
            select(vec![
                PhaseOutcome::Passed,
                PhaseOutcome::Failed,
                PhaseOutcome::Skipped,
            ]),
            text::<0, 200>(),
            any::<bool>(),
        )
            .prop_map(|(outcome, exception, assertion)| PhaseFact {
                outcome,
                exception,
                assertion,
            })
            .boxed()
    }

    fn phases() -> BoxedStrategy<PhaseEvidence> {
        let run = (
            node(),
            phase_fact(),
            proptest::option::of(phase_fact()),
            phase_fact(),
        )
            .prop_map(|(test, setup, call, teardown)| PhaseRun {
                test: Text::new(test).unwrap(),
                setup,
                call,
                teardown,
            });
        let side = || proptest::option::of(run.clone());
        (side(), side(), side())
            .prop_filter_map("phase evidence needs a side", |(base, head, forced)| {
                PhaseEvidence::new(base, head, forced).ok()
            })
            .boxed()
    }

    fn test_result() -> BoxedStrategy<TestResult> {
        (
            node(),
            (outcome(), outcome(), any::<bool>(), any::<bool>()),
            string(0, MESSAGE_CHARS),
            proptest::option::of(outcome()),
            proptest::option::of(phases()),
        )
            .prop_map(
                |(test, (base, head, pre_existing, passed), message, head_forced, phases)| {
                    TestResult {
                        test: Text::new(test).unwrap(),
                        base,
                        head,
                        pre_existing,
                        passed,
                        message: ClippedText::clipped(&message),
                        head_forced,
                        phases,
                    }
                },
            )
            .boxed()
    }

    fn digests() -> BoxedStrategy<SubjectDigests> {
        (
            proptest::option::of(sha256()),
            proptest::option::of(sha256()),
        )
            .prop_map(|(base, head)| SubjectDigests { base, head })
            .boxed()
    }

    fn delay_budget() -> BoxedStrategy<DelayBudget> {
        let applied = prop_oneof![
            3 => (0.001f64..10.0).prop_map(number),
            1 => select(vec![Number::from(0u64), number(0.0), Number::from(10u64), number(10.0)]),
        ];
        (applied, any::<bool>())
            .prop_map(|(applied, exhausted)| DelayBudget {
                applied_seconds: AppliedSeconds::new(applied).unwrap(),
                exhausted,
            })
            .boxed()
    }

    fn proof_receipt() -> BoxedStrategy<ProofReceipt> {
        let units = || unit().prop_map(|unit| unit.as_str().to_owned());
        (
            (
                select(vec![ProofRule::SubjectObserverV1]),
                proptest::option::of(proof_declaration()),
                select(vec![ProofMode::HeadObserver, ProofMode::PytestOutcome]),
                btree_map(units(), digests(), 0..=3),
                btree_map(node(), proptest::option::of(sha256()), 0..=3),
                btree_map(units(), sha256(), 0..=3),
            ),
            (sha256(), sha256(), sha256()),
            proptest::option::of(btree_map(
                node(),
                select(vec![
                    RedMessageMatch::Message,
                    RedMessageMatch::SourceAssertion,
                ]),
                0..=3,
            )),
            proptest::option::of(btree_map(
                select(vec![
                    "base".to_owned(),
                    "head".to_owned(),
                    "head_forced".to_owned(),
                ]),
                btree_map(node(), delay_budget(), 0..=2),
                0..=3,
            )),
        )
            .prop_map(
                |(
                    (rule, declaration, mode, subjects, observer_sha256, support_sha256),
                    (source_base_tree_sha256, base_tree_sha256, head_tree_sha256),
                    red_message_matches,
                    delay_budget,
                )| ProofReceipt {
                    rule,
                    declaration,
                    mode,
                    subjects,
                    observer_sha256,
                    support_sha256,
                    source_base_tree_sha256,
                    base_tree_sha256,
                    head_tree_sha256,
                    red_message_matches,
                    delay_budget,
                },
            )
            .boxed()
    }

    fn selection() -> BoxedStrategy<Selection> {
        let names = || vec(string(1, 20), 0..=3);
        let unit = (
            names(),
            integer::<0, { u64::MAX }>(),
            btree_map(string(1, 20), outcome(), 0..=3),
        )
            .prop_map(|(members, cases, outcomes)| SelectedUnit {
                members,
                cases,
                outcomes,
            });
        let saturation = (
            names(),
            proptest::option::of(integer::<0, { u64::MAX }>()),
            integer::<0, { u64::MAX }>(),
            string(0, 80),
        )
            .prop_map(|(unit, cases, remaining, reason)| SelectionSaturation {
                unit,
                cases,
                remaining,
                reason,
            });
        (
            string(0, 80),
            (integer::<0, { u64::MAX }>(), integer::<0, { u64::MAX }>()),
            vec(unit, 0..=2),
            vec(names(), 0..=2),
            vec(names(), 0..=2),
            any::<bool>(),
            saturation,
        )
            .prop_map(
                |(
                    policy,
                    (limit, charged_cases),
                    base_units,
                    head_units,
                    skipped_units,
                    skipped_checked,
                    saturation,
                )| Selection {
                    policy,
                    limit,
                    charged_cases,
                    base_units,
                    head_units,
                    skipped_units,
                    skipped_checked,
                    saturation,
                },
            )
            .boxed()
    }

    fn missing() -> BoxedStrategy<Vec<MissingName>> {
        vec(
            prop_oneof![
                string(0, 12).prop_map(MissingName::Name),
                (string(0, 12), string(0, 12)).prop_map(|(a, b)| MissingName::Pair([a, b])),
            ],
            0..=2,
        )
        .boxed()
    }

    fn hits() -> BoxedStrategy<BTreeMap<String, Count>> {
        btree_map(dotted(), integer::<0, { u64::MAX }>(), 0..=2).boxed()
    }

    fn record() -> BoxedStrategy<SymptomRecord> {
        let phase_name = || select(vec![PhaseName::Setup, PhaseName::Call, PhaseName::Teardown]);
        let failure = (text::<1, UNBOUNDED>(), phase_name(), any::<bool>()).prop_map(
            |(exception, phase, source)| RecordFailure {
                exception: serde_json::from_value(json!(exception.as_str())).unwrap(),
                phase,
                source,
            },
        );
        let refusal_text = || wire(super::text(0, 40));
        let refusal = (
            refusal_text(),
            refusal_text(),
            refusal_text(),
            refusal_text(),
        )
            .prop_map(
                |(target, resolved_module, owner_category, reason)| ForcingRefusal {
                    phase: ForcingValidation::ForcingValidation,
                    target,
                    resolved_module,
                    owner_category,
                    reason,
                },
            );
        let saturation = (
            list::<Text<1, UNBOUNDED>, 1, UNBOUNDED>(text()),
            integer::<0, { u64::MAX }>(),
            integer::<0, { u64::MAX }>(),
        )
            .prop_map(|(unit, cases, remaining)| CollectionSaturation {
                unit,
                cases,
                remaining,
            });
        let exit_code = wire(
            prop_oneof![
                any::<i64>().prop_map(Value::from),
                (-1000i32..1000).prop_map(|code| json!(f64::from(code))),
            ]
            .boxed(),
        );
        prop_oneof![
            wire(select(vec![json!(1), json!(1.0)]).boxed())
                .prop_map(|schema| SymptomRecord::Start { schema }),
            exit_code.prop_map(|exit_code| SymptomRecord::Finish { exit_code }),
            (
                vec(text::<1, UNBOUNDED>(), 0..=3),
                proptest::option::of(saturation)
            )
                .prop_map(|(tests, saturation)| SymptomRecord::Collection { tests, saturation }),
            (
                (
                    node(),
                    select(vec![
                        RecordOutcome::Passed,
                        RecordOutcome::Failed,
                        RecordOutcome::Error,
                        RecordOutcome::Skipped,
                    ]),
                    text::<0, MESSAGE_CHARS>(),
                    any::<bool>(),
                ),
                proptest::option::of(vec(failure, 0..=3)),
                proptest::option::of(hits()),
                proptest::option::of(delay_budget()),
                proptest::option::of(Just(True)),
                proptest::option::of(refusal),
            )
                .prop_map(
                    |(
                        (test, outcome, message, truncated),
                        failures,
                        target_hits,
                        delay_budget,
                        forcing_tampered,
                        forcing_refusal,
                    )| SymptomRecord::Result {
                        test: Text::new(test).unwrap(),
                        outcome,
                        message,
                        truncated,
                        failures,
                        target_hits,
                        delay_budget,
                        forcing_tampered,
                        forcing_refusal,
                    },
                ),
            (
                (
                    node(),
                    phase_name(),
                    select(vec![
                        PhaseOutcome::Passed,
                        PhaseOutcome::Failed,
                        PhaseOutcome::Skipped,
                    ]),
                    any::<bool>(),
                    proptest::option::of(missing()),
                ),
                proptest::option::of(text::<0, 300>()),
                proptest::option::of(text::<0, 200>()),
                proptest::option::of(text::<0, MESSAGE_CHARS>()),
            )
                .prop_map(
                    |(
                        (test, when, outcome, assertion, missing),
                        exception,
                        exception_type,
                        message,
                    )| {
                        SymptomRecord::Phase {
                            test: Text::new(test).unwrap(),
                            when,
                            outcome,
                            assertion,
                            missing,
                            exception,
                            exception_type,
                            message,
                        }
                    },
                ),
            (
                proptest::option::of(string(0, 20)),
                proptest::option::of(proptest::option::of(missing())),
                proptest::option::of(text::<0, 300>()),
                proptest::option::of(text::<0, MESSAGE_CHARS>()),
                proptest::option::of(any::<bool>()),
            )
                .prop_map(|(test, missing, exception, message, truncated)| {
                    SymptomRecord::CollectionError {
                        test,
                        missing,
                        exception,
                        message,
                        truncated,
                    }
                }),
            text::<0, MESSAGE_CHARS>().prop_map(|message| SymptomRecord::Unavailable { message }),
            text::<0, MESSAGE_CHARS>().prop_map(|message| SymptomRecord::PlanError { message }),
        ]
        .boxed()
    }

    fn symptom_evidence() -> BoxedStrategy<SymptomEvidence> {
        let run = (
            select(vec![
                SymptomSide::Base,
                SymptomSide::Head,
                SymptomSide::OrdinaryHead,
            ]),
            integer::<1, 10>(),
            select(vec![
                RunOutcome::Passed,
                RunOutcome::Symptom,
                RunOutcome::Unrelated,
                RunOutcome::Unavailable,
            ]),
            sha256(),
            vec(record(), 0..=3),
            proptest::option::of(hits()),
        )
            .prop_map(
                |(side, run, outcome, sha256, records, target_hits)| SymptomRun {
                    side,
                    run,
                    outcome,
                    sha256,
                    records,
                    target_hits,
                },
            )
            .boxed();
        (
            commit(),
            commit(),
            proptest::option::of(forcing()),
            declared_symptom(),
            list::<SymptomRun, 0, 30>(run),
        )
            .prop_map(|(base, head, forcing, declaration, runs)| SymptomEvidence {
                base,
                head,
                forcing,
                declaration,
                runs,
            })
            .boxed()
    }

    pub(super) fn check_result() -> BoxedStrategy<CheckResult> {
        (
            (
                text::<1, UNBOUNDED>(),
                select(vec![
                    CheckKind::Demonstrating,
                    CheckKind::Regression,
                    CheckKind::BehaviorPreserving,
                    CheckKind::Reproducing,
                ]),
                select(vec![
                    Verdict::Pass,
                    Verdict::Fail,
                    Verdict::InfrastructureFailure,
                ]),
                list::<TestResult, 1, UNBOUNDED>(test_result()),
            ),
            proptest::option::of(selection()),
            proptest::option::of(forcing()),
            proptest::option::of(proof_receipt()),
            proptest::option::of(declared_symptom()),
            proptest::option::of(symptom_evidence()),
        )
            .prop_map(
                |(
                    (name, kind, verdict, tests),
                    selection,
                    forcing,
                    proof,
                    declared_symptom,
                    symptom_evidence,
                )| CheckResult {
                    name,
                    kind,
                    verdict,
                    custody: Custody::InProcess,
                    tests,
                    selection,
                    forcing,
                    proof,
                    declared_symptom,
                    symptom_evidence,
                },
            )
            .boxed()
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        failure_persistence: None,
        rng_seed: RngSeed::Fixed(750),
        ..ProptestConfig::default()
    })]

    /// Encodes: #750 review finding 4. Every value the constructors allow serializes to a
    /// document the generated schema accepts, and parses back to the same value.
    #[test]
    fn constructed_results_satisfy_the_schema_and_round_trip(result in typed::check_result()) {
        static VALIDATOR: std::sync::OnceLock<jsonschema::Validator> = std::sync::OnceLock::new();
        let validator = VALIDATOR.get_or_init(|| check_report_validator(Some("CheckResult")));
        let document = serde_json::to_value(&result).unwrap();
        let errors: Vec<_> = validator
            .iter_errors(&document)
            .map(|error| format!("{}: {error}", error.instance_path()))
            .collect();
        prop_assert!(errors.is_empty(), "a constructed value breaks the schema: {errors:?}");
        let parsed: CheckResult = serde_json::from_value(document.clone())
            .map_err(|error| TestCaseError::fail(format!("a constructed value does not parse: {error}")))?;
        prop_assert_eq!(serde_json::to_value(&parsed).unwrap(), document);
        prop_assert_eq!(parsed, result);
    }
}

/// Encodes: #750 review finding 1. runner.py records the full qualified exception class name
/// and the module a forcing resolved to, unclipped, and test_tool.py keeps (and hashes) the
/// record before it refuses it. The controller must keep that evidence byte for byte.
#[test]
fn symptom_records_keep_the_runners_unclipped_strings() {
    let record = json!({"event": "result", "test": "tests/test_long.py::test_long",
        "outcome": "error", "message": "", "truncated": false,
        "failures": [{"exception": format!("test_long.{}", "Deadline".repeat(40)),
            "phase": "call", "source": true}],
        "forcing_refusal": {"phase": "forcing_validation", "target": "product.target",
            "resolved_module": "m".repeat(5000), "owner_category": "external_module",
            "reason": "outside_served_repository"}});
    let typed: tl3_controller::contracts::report::SymptomRecord =
        serde_json::from_value(record.clone()).expect("the producer's record parses");
    assert_eq!(serde_json::to_value(&typed).unwrap(), record);
    let errors: Vec<_> = check_report_validator(Some("SymptomRecord"))
        .iter_errors(&record)
        .map(|error| format!("{}: {error}", error.instance_path()))
        .collect();
    assert!(
        errors.is_empty(),
        "check-report.json rejects the producer's record: {errors:?}"
    );
}

/// Encodes: #750 review finding 3. JSON Schema's `integer` accepts integral floats and states
/// its bounds; serde must accept exactly the same numbers and keep them as written.
#[test]
fn integral_numbers_parse_as_the_schema_allows() {
    let declaration = json!({"node": "tests/test_a.py::test_a", "exception": "builtins.TimeoutError",
        "phase": "call", "repeat": 1.0, "parallel": 0.0});
    let typed: tl3_controller::contracts::DeclaredSymptom =
        serde_json::from_value(declaration.clone()).expect("integral floats are integers");
    assert_eq!(
        serde_json::to_value(&typed).unwrap(),
        declaration,
        "kept as written"
    );
    for record in [
        json!({"event": "finish", "exit_code": 1.0}),
        json!({"event": "collection", "tests": [], "saturation": {"unit": ["a"], "cases": 2.0,
            "remaining": u64::MAX}}),
    ] {
        let typed: tl3_controller::contracts::report::SymptomRecord =
            serde_json::from_value(record.clone()).expect("schema-valid record parses");
        assert_eq!(serde_json::to_value(&typed).unwrap(), record);
    }
    let schema = contracts::check_report_schema();
    assert_eq!(
        schema["$defs"]["CollectionSaturation"]["properties"]["cases"]["maximum"],
        json!(u64::MAX),
        "a count states the largest value serde accepts"
    );
    let validator = check_report_validator(Some("CollectionSaturation"));
    let too_large = json!({"unit": ["a"], "cases": 18446744073709551616.0, "remaining": 0});
    assert!(!validator.is_valid(&too_large), "the schema refuses 2^64");
    assert!(
        serde_json::from_value::<tl3_controller::contracts::report::CollectionSaturation>(
            too_large
        )
        .is_err(),
        "serde refuses 2^64"
    );
}

// One released set per contract the harness has shipped: the reports one release's
// harness wrote, kept so the next controller is held to them. released-contracts.json
// records each set's report version, the commit whose harness produced it, and the
// sha256 of check-report.json at the time (#750 review finding 2, re-review finding 3).
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleasedSet {
    set: String,
    version: String,
    harness_commit: String,
    harness_src_tree: String,
    check_report_sha256: String,
}

fn released_sets() -> Vec<ReleasedSet> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Manifest {
        sets: Vec<ReleasedSet>,
    }
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../harness/test-vectors/released-contracts.json");
    let manifest: Manifest = serde_json::from_slice(
        &std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display())),
    )
    .unwrap();
    manifest.sets
}

fn released_vectors() -> Vec<(String, String, Value)> {
    let root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../harness/test-vectors/contracts/released");
    let mut found = vec![];
    for set in released_sets() {
        let directory = root.join(&set.set);
        for vector in std::fs::read_dir(&directory)
            .unwrap_or_else(|error| panic!("{}: {error}", directory.display()))
        {
            let path = vector.unwrap().path();
            found.push((
                set.version.clone(),
                format!(
                    "{}/{}",
                    set.set,
                    path.file_stem().unwrap().to_string_lossy()
                ),
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap(),
            ));
        }
    }
    found.sort_by(|a, b| a.1.cmp(&b.1));
    found
}

/// Encodes: #750 re-review finding 3. Every released set is listed with its provenance,
/// and every listed set is present.
#[test]
fn released_sets_are_listed_with_their_provenance() {
    let sets = released_sets();
    assert!(!sets.is_empty(), "no released contract set");
    let root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../harness/test-vectors/contracts/released");
    let mut present: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    present.sort();
    let mut listed: Vec<_> = sets.iter().map(|set| set.set.clone()).collect();
    listed.sort();
    assert_eq!(
        present, listed,
        "released/ and released-contracts.json disagree"
    );
    let hex = |text: &str, length: usize| {
        text.len() == length
            && text
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    };
    for set in &sets {
        assert!(hex(&set.harness_commit, 40), "{}: harness_commit", set.set);
        assert!(
            hex(&set.harness_src_tree, 40),
            "{}: harness_src_tree",
            set.set
        );
        assert!(
            hex(&set.check_report_sha256, 64),
            "{}: check_report_sha256",
            set.set
        );
        serde_json::from_value::<tl3_controller::contracts::report::ReportSchema>(json!(
            set.version
        ))
        .unwrap_or_else(|error| {
            panic!(
                "{}: {} is not a report version: {error}",
                set.set, set.version
            )
        });
    }
}

/// Encodes: #750 re-review finding 3. A contract change must capture compatibility fixtures
/// from the harness that produces the new shape: the newest released set was captured
/// against exactly the contract this tree generates.
#[test]
fn the_newest_released_set_captures_the_current_contract() {
    use sha2::{Digest, Sha256};
    let generated = contracts::generated();
    let rendered = format!("{:x}", Sha256::digest(generated[0].text.as_bytes()));
    let newest = released_sets().pop().unwrap();
    assert_eq!(
        newest.check_report_sha256, rendered,
        "check-report.json changed since released set {}. Capture fixtures from the harness \
         that produces the new shape: copy harness/test-vectors/contracts/*.json into a new \
         contracts/released/<set>/, and append that set to released-contracts.json with this \
         sha256, the producing commit and its harness/src tree.",
        newest.set
    );
}

/// Encodes: #750 re-review findings 4 and 5. Integer bounds and numeric constants decide
/// exactly as Python's validator does, on the raw JSON text, at both ends of every integer
/// type. workers/tests/test_check_report_vectors.py runs the same cases.
#[test]
fn raw_numbers_parse_as_pythons_validator_decides() {
    use tl3_controller::contracts::report::{CollectionSaturation, SymptomRecord, SymptomRun};
    #[derive(serde::Deserialize)]
    struct Case {
        definition: String,
        document: String,
        valid: bool,
    }
    #[derive(serde::Deserialize)]
    struct Cases {
        cases: Vec<Case>,
    }
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../harness/test-vectors/contract-numbers.json");
    let cases: Cases = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(cases.cases.len() >= 30);
    for case in cases.cases {
        let parsed = match case.definition.as_str() {
            "CollectionSaturation" => {
                serde_json::from_str::<CollectionSaturation>(&case.document).is_ok()
            }
            "DeclaredSymptom" => {
                serde_json::from_str::<tl3_controller::contracts::DeclaredSymptom>(&case.document)
                    .is_ok()
            }
            // Records reach the controller inside a symptom run, which reads each one's raw
            // text, so an exit code is checked exactly (#750 round 3 finding 5).
            // A record is checked exactly on its own and inside its symptom run (#750 round 4).
            "SymptomRecord" => {
                let direct = serde_json::from_str::<SymptomRecord>(&case.document).is_ok();
                let wrapped = serde_json::from_str::<SymptomRun>(&format!(
                    r#"{{"side":"base","run":1,"outcome":"passed","sha256":"{}","records":[{}]}}"#,
                    "0".repeat(64),
                    case.document
                ))
                .is_ok();
                assert_eq!(direct, wrapped, "direct and wrapped: {}", case.document);
                direct
            }
            other => panic!("no Rust type for {other}"),
        };
        assert_eq!(parsed, case.valid, "{} {}", case.definition, case.document);
    }
}

/// Encodes: #750 review finding 2, a new controller resuming a check the previous release's
/// harness staged: every released report version still parses, unchanged.
#[test]
fn every_released_report_version_still_parses() {
    let released = released_vectors();
    assert!(
        released.len() >= 8,
        "the previous release's vectors are missing"
    );
    for (version, name, vector) in released {
        let request = &vector["request"];
        let envelope = json!({"schema": version, "effect_key": request["effect_key"],
            "base": request["base"], "head": request["head"], "reviewed": request["reviewed"],
            "results": vector["results"]});
        let report: CheckReport = serde_json::from_value(envelope.clone())
            .unwrap_or_else(|error| panic!("{version}/{name}: no longer parses: {error}"));
        assert_eq!(
            serde_json::to_value(&report).unwrap(),
            envelope,
            "{version}/{name}"
        );
    }
}

/// Encodes: #750 review finding 2, the other direction. The trusted action writes the newest
/// version this controller knows, and a controller refuses a version it does not know,
/// instead of guessing at a newer harness's shape (a rollback's case).
#[test]
fn report_versions_are_written_by_the_action_and_checked_by_the_controller() {
    let action = include_str!("../src/adapters/prismabuild/action.py");
    assert!(
        action.contains(r#""schema": "tl3.checks.v1""#),
        "action.py writes the report version"
    );
    let vector = vectors().remove(0).1;
    let request = &vector["request"];
    let future = json!({"schema": "tl3.checks.v2", "effect_key": request["effect_key"],
        "base": request["base"], "head": request["head"], "reviewed": request["reviewed"],
        "results": vector["results"]});
    assert!(serde_json::from_value::<CheckReport>(future).is_err());
}

// #819 P1b-0: every shared role type must serialize into the unchanged wire schema.
fn shared_reply_validator(role: &str, definition: Option<&str>) -> &'static jsonschema::Validator {
    static VALIDATORS: std::sync::OnceLock<
        std::collections::BTreeMap<String, jsonschema::Validator>,
    > = std::sync::OnceLock::new();
    let validators = VALIDATORS.get_or_init(|| {
        let schemas: &[(&str, &str)] = &[
            (
                "audit-input.json",
                include_str!("../../../workers/src/tl3_workers/schemas/audit-input.json"),
            ),
            (
                "audit-reply.json",
                include_str!("../../../workers/src/tl3_workers/schemas/audit-reply.json"),
            ),
            (
                "budget-wire.json",
                include_str!("../../../workers/src/tl3_workers/schemas/budget-wire.json"),
            ),
            (
                "check-report.json",
                include_str!("../../../workers/src/tl3_workers/schemas/check-report.json"),
            ),
            (
                "classifier-input.json",
                include_str!("../../../workers/src/tl3_workers/schemas/classifier-input.json"),
            ),
            (
                "classifier-reply.json",
                include_str!("../../../workers/src/tl3_workers/schemas/classifier-reply.json"),
            ),
            (
                "decision-context.json",
                include_str!("../../../workers/src/tl3_workers/schemas/decision-context.json"),
            ),
            (
                "ingest-input.json",
                include_str!("../../../workers/src/tl3_workers/schemas/ingest-input.json"),
            ),
            (
                "ingest-reply.json",
                include_str!("../../../workers/src/tl3_workers/schemas/ingest-reply.json"),
            ),
            (
                "maker-input.json",
                include_str!("../../../workers/src/tl3_workers/schemas/maker-input.json"),
            ),
            (
                "maker-reply.json",
                include_str!("../../../workers/src/tl3_workers/schemas/maker-reply.json"),
            ),
            (
                "planner-input.json",
                include_str!("../../../workers/src/tl3_workers/schemas/planner-input.json"),
            ),
            (
                "planner-reply.json",
                include_str!("../../../workers/src/tl3_workers/schemas/planner-reply.json"),
            ),
            (
                "postmortem-input.json",
                include_str!("../../../workers/src/tl3_workers/schemas/postmortem-input.json"),
            ),
            (
                "postmortem-reply.json",
                include_str!("../../../workers/src/tl3_workers/schemas/postmortem-reply.json"),
            ),
            (
                "request.json",
                include_str!("../../../workers/src/tl3_workers/schemas/request.json"),
            ),
            (
                "reviewer-input.json",
                include_str!("../../../workers/src/tl3_workers/schemas/reviewer-input.json"),
            ),
            (
                "reviewer-reply.json",
                include_str!("../../../workers/src/tl3_workers/schemas/reviewer-reply.json"),
            ),
            (
                "writer-input.json",
                include_str!("../../../workers/src/tl3_workers/schemas/writer-input.json"),
            ),
            (
                "writer-reply.json",
                include_str!("../../../workers/src/tl3_workers/schemas/writer-reply.json"),
            ),
        ];
        let mut registry = jsonschema::Registry::new();
        for (name, text) in schemas {
            registry = registry
                .add(
                    format!("{}{name}", contracts::SCHEMA_BASE),
                    serde_json::from_str::<Value>(text).unwrap(),
                )
                .unwrap();
        }
        let registry = registry.prepare().unwrap();
        [
            ("ingest", None),
            ("ingest", Some("tool_capability")),
            ("ingest", Some("evidence_contract")),
            ("ingest", Some("proof_obligation")),
            ("classifier", None),
            ("maker", None),
            ("writer", None),
            ("maker", Some("generation_receipt")),
            ("maker", Some("test_run")),
            ("writer", Some("text")),
            ("writer", Some("options")),
        ]
        .into_iter()
        .map(|(role, definition)| {
            let suffix = definition
                .map(|name| format!("#/$defs/{name}"))
                .unwrap_or_default();
            let key = format!("{role}{suffix}");
            let validator = jsonschema::options()
                .with_registry(&registry)
                .build(
                    &json!({"$ref":format!("{}{role}-reply.json{suffix}",contracts::SCHEMA_BASE)}),
                )
                .unwrap();
            (key, validator)
        })
        .collect()
    });
    let suffix = definition
        .map(|name| format!("#/$defs/{name}"))
        .unwrap_or_default();
    &validators[&format!("{role}{suffix}")]
}

fn shared_sound<T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug>(
    value: Value,
    validator: &jsonschema::Validator,
) {
    assert!(validator.is_valid(&value), "invalid test input: {value}");
    let typed: T = serde_json::from_value(value.clone()).unwrap_or_else(|error| {
        panic!("schema-valid shared type failed to parse: {error}: {value}")
    });
    let encoded = serde_json::to_value(&typed).unwrap();
    assert!(
        validator.is_valid(&encoded),
        "typed serialization breaks the schema: {encoded}"
    );
    assert_eq!(encoded, value);
    assert_eq!(serde_json::from_value::<T>(encoded).unwrap(), typed);
}

fn receipt_budget() -> Value {
    json!({"cpus":1,"mem_gb":1,"timeout_s":60,"outer_timeout_s":120})
}
fn shared_generation_receipt(seconds: f64, count: u64, cache: bool) -> Value {
    let phase = json!({"seconds":seconds,"outcome":"completed","scope":"startup_and_command"});
    json!({"action_key":"a".repeat(64),"artifact_sha256":"b".repeat(64),"receipt_sha256":"c".repeat(64),
        "output_manifest":"d".repeat(64),"input_tree":"a".repeat(40),"output_tree":"b".repeat(40),
        "status":"completed","exit_code":0.0,"command":"python3 generate.py","argv":["python3","generate.py"],
        "identity":{"implementation":"tl3-generator-v4","image":"image","budget":receipt_budget()},
        "output_count":count,"budget":receipt_budget(),"phase":"completed",
        "timings":{"preparation":phase,"container":phase,"validation":phase,"cleanup":phase,"total_seconds":seconds},
        "inner":{"status":"completed","exit_code":0},
        "outer":{"status":if cache {"cache_hit"} else {"executed"},"exit_code":0,"elapsed_s":seconds}})
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 96, rng_seed: RngSeed::Fixed(819), .. ProptestConfig::default() })]
    #[test]
    fn p_schema_sound_ingest_and_shared_evidence(suffix in ".{0,60}", early in any::<bool>(), publishing in any::<bool>(), source in any::<bool>()) {
        use contracts::roles::ingest_reply::{IngestReply, ToolCapability, EvidenceContract, ProofObligation};
        let text=format!("e{suffix}");
        let capability=json!({"tool":"run_tests","executor":"prismabuild","runner_features":["tests"],"artifacts":["receipt"],"methods":[],"protected_files":[]});
        shared_sound::<ToolCapability>(capability.clone(),shared_reply_validator("ingest",Some("tool_capability")));
        let mut requirement=capability; requirement["requirement"]=json!(text);
        requirement["provenance"]=if source {json!({"kind":"source","source":"issue","quote":text})} else {json!({"kind":"method"})};
        let evidence=json!([requirement]);
        shared_sound::<EvidenceContract>(evidence.clone(),shared_reply_validator("ingest",Some("evidence_contract")));
        let proof=json!({"kind":"assertion","observer":"new","subjects":["tests/test_a.py::test_a"],"source":"issue","quote":text});
        shared_sound::<ProofObligation>(proof.clone(),shared_reply_validator("ingest",Some("proof_obligation")));
        let mut goal=json!({"description":text,"verification":"test","proof_obligation":proof});
        if publishing { goal["stage"]=json!("publishing");goal["publication"]=json!({"non_draft":true,"body_lines":[],"proof":false}); }
        let reply=if early {json!({"kind":"early_end","outcome":"duplicate","reason":text,"evidence_contract":evidence})}
            else {json!({"kind":"brief","brief":text,"goals":[goal],"memory_ids":[],"evidence_contract":evidence})};
        shared_sound::<IngestReply>(reply,shared_reply_validator("ingest",None));
    }
    #[test]
    fn p_schema_sound_classifier(suffix in ".{0,60}", choice in 0u8..13, credit in 1u64..=100) {
        use contracts::roles::classifier_reply::ClassifierReply;
        let text=format!("c{suffix}");
        let reply=match choice {
            0 => json!({"kind":"change_request","new_text":text,"supersedes":["id"]}),
            1..=5 => json!({"kind":(["stop","restart","pause","resume","remark"][(choice-1) as usize])}),
            6 => json!({"kind":"unsure","question":"One line?"}),
            7 => json!({"kind":"complaint","summary":"One line","revert":true}),
            8 => json!({"kind":"answer","question_id":text,"answer":"stop","decision":{"allowed_scope":[],"restrictions":[text]}}),
            9 => json!({"kind":"answer","question_id":text,"answer":"change_request","new_text":text}),
            10 => json!({"kind":"answer","question_id":text,"answer":"try_again","guidance":text}),
            11 => json!({"kind":"answer","question_id":text,"answer":"add_credit","amount":credit}),
            _ => json!({"kind":"reconcile_decisions","assessments":[{"id":text,"contradicted":true,"reason":text}]}),
        };
        shared_sound::<ClassifierReply>(reply,shared_reply_validator("classifier",None));
    }
    #[test]
    fn p_schema_sound_generation_receipt_and_test_run(seconds in 0.0f64..1000.0, count in 0u64..=128, cache in any::<bool>(), repeat in 1u64..=10, parallel in select(vec![0,2,3,4]), optional in any::<bool>(), proof in proof_receipt(), exit in any::<i64>()) {
        use contracts::roles::maker_reply::{GenerationReceipt, TestRun};
        let mut receipt=shared_generation_receipt(seconds,count,cache);
        if optional {
            receipt["identity"]["implementation"]=json!("tl3-generator-v3");
            receipt["budget"]=json!({"cpus":16.0,"mem_gb":64.0,"timeout_s":600.0,"outer_timeout_s":660.0});
            receipt["identity"]["budget"]=json!({"cpus":1.0,"mem_gb":1.0,"timeout_s":1.0,"outer_timeout_s":61.0});
            receipt["outer"]["elapsed_s"]=Value::Null;
            for (phase,outcome) in [("preparation","failed"),("container","timeout"),("validation","skipped"),("cleanup","completed")] {
                receipt["timings"][phase]["outcome"]=json!(outcome);
                receipt["timings"][phase].as_object_mut().unwrap().remove("scope");
            }
        }
        shared_sound::<GenerationReceipt>(receipt,shared_reply_validator("maker",Some("generation_receipt")));
        let mut tests=json!({"tests":["tests/test_a.py::test_a"],"base":false,"repeat":repeat,"parallel":parallel,"status":"completed",
            "tree":"a".repeat(40),"fresh":true,"totals":{"passed":1,"failed":0,"error":0,"skipped":0},
            "receipts":[{"action_key":"a".repeat(64),"status":"executed","exit_code":exit,"receipt_sha256":"b".repeat(64),"log_sha256":"c".repeat(64)}]});
        if optional {
            tests["tests_identity"]=json!({"schema":"tl3.selectors.v1","count":256.0,"sha256":"a".repeat(64),"prefix":["é".repeat(120),"b","c"]});
            tests["tests_omitted"]=json!(256.0);
            tests["base_semantics"]=json!(if cache {"untouched_base"} else {"base_subject_head_observer"});
            tests["proof"]=proof;
            tests["execution"]=json!({"outcome":"test_timeout","phase":if cache {"test_run"} else {"cleanup"},"termination_reason":"no_progress","test_started":true,"test":"é".repeat(2048)});
            tests["runs"]=json!([{"run":repeat,"status":"completed","totals":{"passed":u64::MAX,"failed":0.0,"error":0,"skipped":0}}]);
            tests["receipts"][0]["attempt"]=json!(1.0);
            tests["receipts"][0]["generation"]=json!("a".repeat(64));
            tests["receipts"][0]["status"]=json!("cache_hit");
        }
        shared_sound::<TestRun>(tests,shared_reply_validator("maker",Some("test_run")));
    }
    #[test]
    fn p_schema_sound_maker_root_and_writer_root(choice in 0u8..7, suffix in ".{0,60}", fits in any::<bool>()) {
        use contracts::roles::{maker_reply::MakerReply,writer_reply::WriterReply};
        let text=format!("r{suffix}");
        let reply=match choice {
            0 => json!({"kind":"done","head":"a".repeat(40),"self_check":[{"goal_id":text,"status":"met","evidence":text}]}),
            1 => json!({"kind":"done","head":"b".repeat(64),"self_check":[{"goal_id":text,"status":"not_met","why":text}]}),
            2 => json!({"kind":"done","head":"a".repeat(40),"self_check":[{"goal_id":text,"status":"pending","why":text}]}),
            3 => json!({"kind":"needs_files","files":[{"path":"file.py","why":text}],"fits":fits}),
            4 => json!({"kind":"cant_meet","report":text,"unfinished_obligations":[text]}),
            5 => json!({"kind":"blocked","report":text,"evidence":{"action_key":"a".repeat(64),"class":"test_infrastructure"}}),
            _ => json!({"kind":"out_of_room"}),
        };
        shared_sound::<MakerReply>(reply,shared_reply_validator("maker",None));
        let writer=if fits {json!({"text":text})} else {json!({"request_quote":text,"obstacle":text,
            "options":[{"action":"retry","outcome":text},{"action":"stop","outcome":text}],
            "recommended_option":0.0,"recommendation_reason":text,"how_to_answer":text,"decision":text})};
        shared_sound::<WriterReply>(writer,shared_reply_validator("writer",None));
    }
    #[test]
    fn p_schema_sound_writer_text_and_options(suffix in ".{0,60}", count in 2usize..=3) {
        use contracts::roles::writer_reply::{Text,Options};
        let text=format!("w{suffix}");
        shared_sound::<Text>(json!(text),shared_reply_validator("writer",Some("text")));
        let options=Value::Array((0..count).map(|i|json!({"action":format!("{text}{i}"),"outcome":text})).collect());
        shared_sound::<Options>(options,shared_reply_validator("writer",Some("options")));
    }
}

#[test]
fn shared_reply_types_reject_unsupported_shapes_and_enforce_conditionals() {
    use contracts::roles::{
        classifier_reply::ClassifierReply,
        ingest_reply::IngestReply,
        writer_reply::{Options, Text},
    };
    for value in [
        json!({"kind":"brief","brief":"b","goals":[{"description":"g","verification":"test","stage":"publishing"}],"memory_ids":[]}),
        json!({"kind":"brief","brief":"b","goals":[{"description":"g","verification":"test","publication":{"non_draft":true,"body_lines":[],"proof":false}}],"memory_ids":[]}),
    ] {
        assert!(serde_json::from_value::<IngestReply>(value).is_err());
    }
    for value in [
        json!({"kind":"complaint","summary":"two\nlines","revert":false}),
        json!({"kind":"answer","question_id":"q","answer":"add_credit","amount":101}),
        json!({"kind":"stop","extra":true}),
        json!({"kind":"change_request","new_text":"change","supersedes":null}),
    ] {
        assert!(serde_json::from_value::<ClassifierReply>(value).is_err());
    }
    assert!(serde_json::from_value::<Text>(json!(" ")).is_err());
    assert!(serde_json::from_value::<Options>(json!([{"action":"one","outcome":"one"}])).is_err());

    use contracts::roles::maker_reply::{GenerationReceipt, TestRun};
    let mut receipt = shared_generation_receipt(0.5, 128, true);
    receipt["outer"]["elapsed_s"] = Value::Null;
    shared_sound::<GenerationReceipt>(
        receipt.clone(),
        shared_reply_validator("maker", Some("generation_receipt")),
    );
    receipt["outer"]
        .as_object_mut()
        .unwrap()
        .remove("elapsed_s");
    assert!(serde_json::from_value::<GenerationReceipt>(receipt).is_err());
    let run = json!({"tests":[],"base":false,"repeat":1.0,"parallel":0.0,"status":"completed",
        "tree":"a".repeat(40),"fresh":false,"totals":{"passed":0,"failed":0,"error":0,"skipped":0},
        "receipts":[{"action_key":"a".repeat(64),"status":"cache_hit","exit_code":-1.0,
            "receipt_sha256":"b".repeat(64),"log_sha256":"c".repeat(64),"attempt":null}]});
    assert!(serde_json::from_value::<TestRun>(run).is_err());
}
