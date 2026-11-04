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
const COMMITTED: &[(&str, &str)] = &[(
    "workers/src/tl3_workers/schemas/check-report.json",
    include_str!("../../../workers/src/tl3_workers/schemas/check-report.json"),
)];

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

fn count() -> BoxedStrategy<Value> {
    (0u64..1000).prop_map(Value::from).boxed()
}

fn list(item: BoxedStrategy<Value>, size: std::ops::RangeInclusive<usize>) -> BoxedStrategy<Value> {
    vec(item, size).prop_map(Value::Array).boxed()
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

fn delay_budget() -> BoxedStrategy<Value> {
    object(
        vec![
            ("applied_seconds", seconds(10)),
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
                map(pattern("[A-Z_][A-Z0-9_]{0,10}"), text(0, 1000), 0..=3),
            ),
            ("patches", list(patch.boxed(), 0..=3)),
        ],
        vec![],
    )
}

fn units() -> BoxedStrategy<Value> {
    btree_set(
        unit_path().prop_map(|unit| unit.as_str().unwrap().to_owned()),
        0..=4,
    )
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
            ("repeat", (1u64..=10).prop_map(Value::from).boxed()),
            (
                "parallel",
                select(vec![0u64, 2, 3, 4]).prop_map(Value::from).boxed(),
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
        event("start", vec![("schema", Just(json!(1)).boxed())], vec![]),
        event(
            "finish",
            vec![("exit_code", any::<i32>().prop_map(Value::from).boxed())],
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
            ("run", (1u64..=10).prop_map(Value::from).boxed()),
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
            ("runs", list(run, 0..=3)),
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
