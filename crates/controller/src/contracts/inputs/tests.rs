use super::*;
use serde_json::json;

/// #730 C1/C2: typed builders preserve request/session identities, including key order.
#[test]
fn captured_requests_keep_their_wire_bytes() {
    let value = json!({"comment": {"author": "requester", "body": "restart"},
        "state": {"phase": "Closed(Done)", "closed": true, "restartable": false,
            "open_question": null}});
    let typed = role_wire("classifier", value.clone()).unwrap();
    assert_eq!(
        serde_json::to_vec(&typed).unwrap(),
        serde_json::to_vec(&value).unwrap()
    );
}

/// #658/#663/#717: every role input has a generated schema, rather than a hand copy.
#[test]
fn every_role_input_schema_is_generated() {
    let generated = generated();
    for name in [
        "request",
        "ingest-input",
        "audit-input",
        "planner-input",
        "maker-input",
        "reviewer-input",
        "writer-input",
        "classifier-input",
        "postmortem-input",
        "decision-context",
    ] {
        assert!(
            generated
                .iter()
                .any(|file| file.path.ends_with(&format!("/{name}.json"))),
            "missing generated schema: {name}"
        );
    }
}

/// #730 C1: optional null values remain distinct from absent fields.
#[test]
fn envelope_round_trip_preserves_presence_and_sorted_keys() {
    for mlflow in [None, Some(Value::Null), Some(json!({}))] {
        let inputs = json!({"comment":{"author":"requester","body":"restart"},
            "state":{"phase":"Closed(Done)","closed":true,"restartable":false,"open_question":null}});
        let mut old = json!({"protocol":1,"operation":{"node":0,"generation":2,"kind":"classifier"},
            "role":"classifier","model":{"base_url":"https://model.invalid/v1","name":"fixture",
                "effort":null,"budget_tokens":100},"key_file":"/fixture/key", "inputs":inputs,
            "worktree":"/fixture/worktree", "hindsight_key_file":null});
        if let Some(mlflow) = mlflow {
            old["mlflow"] = mlflow;
        }
        let typed: WorkerRequest = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(
            value(typed).unwrap().to_string().as_bytes(),
            old.to_string().as_bytes()
        );
    }
}

/// #717: parsing a report cannot add its optional default message to worker input.
#[test]
fn reviewer_evidence_is_typed_without_normalizing_the_wire() {
    let old = json!({"name":"regressions","kind":"regression","verdict":"pass","custody":"in-process",
        "tests":[{"test":"tests/test_a.py","base":"passed","head":"passed","pre_existing":false,"passed":true}]});
    let parsed = Lossless::<crate::contracts::report::CheckResult>::parse(old.clone()).unwrap();
    assert_eq!(value(&parsed).unwrap(), old);
    assert_eq!(parsed.typed().name.as_str(), "regressions");
}

proptest::proptest! {
    /// #730 C2: arbitrary Unicode comments round-trip through the typed role contract.
    #[test]
    fn typed_classifier_round_trip(author in "[a-z]{1,32}", body in ".{0,512}", closed in proptest::bool::ANY) {
        let input = ClassifierInput::new(Comment::new(author, body), ClassifierState {
            phase: "Planning".into(), closed, restartable: Some(true), open_question: Some(None),
        });
        let document = value(&input).unwrap();
        let parsed: ClassifierInput = serde_json::from_value(document.clone()).unwrap();
        proptest::prop_assert_eq!(parsed, input);
        proptest::prop_assert!(crate::adapters::workers::protocol::classifier_schema(&document, false).is_ok());
    }
}

include!(concat!(env!("OUT_DIR"), "/worker_schemas.rs"));

fn input_validator(role: &str) -> &'static jsonschema::Validator {
    static VALIDATORS: std::sync::OnceLock<
        std::collections::BTreeMap<String, jsonschema::Validator>,
    > = std::sync::OnceLock::new();
    &VALIDATORS.get_or_init(|| {
        let mut documents = PACKAGED_SCHEMAS
            .iter()
            .map(|(name, text)| {
                (
                    (*name).to_owned(),
                    serde_json::from_str::<Value>(text).unwrap(),
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        for file in crate::contracts::generated() {
            documents.insert(
                file.path.rsplit('/').next().unwrap().to_owned(),
                serde_json::from_str(&file.text).unwrap(),
            );
        }
        let mut registry = jsonschema::Registry::new();
        for (name, document) in &documents {
            registry = registry
                .add(
                    format!("{}{name}", crate::contracts::SCHEMA_BASE),
                    document.clone(),
                )
                .unwrap();
        }
        let registry = registry.prepare().unwrap();
        documents
            .into_iter()
            .filter(|(name, _)| name.ends_with("-input.json") || name == "request.json")
            .map(|(name, document)| {
                (
                    name,
                    jsonschema::options()
                        .with_registry(&registry)
                        .build(&document)
                        .unwrap(),
                )
            })
            .collect()
    })[&if role == "request" {
        "request.json".to_owned()
    } else {
        format!("{role}-input.json")
    }]
}

fn request_input_corpus(text: &str, count: u64) -> Vec<(&'static str, Value)> {
    let issue = json!({"number":count,"title":text,"body":text,"author":"requester","comments":[]});
    let repo = json!({"readme":text,"file_list":["README.md"]});
    let brief = json!({"kind":"brief","brief":text,"goals":[{"description":text,"verification":"review"}],"memory_ids":[]});
    let goals = json!([{"id":"g1","description":text}]);
    let forecast =
        json!({"fit_tokens":count,"context_tokens":count+1000,"overhead_tokens":{"fixture":0}});
    let sha = "a".repeat(40);
    vec![
        ("ingest", json!({"issue":issue,"repo":repo})),
        ("audit", json!({"issue":issue,"repo":repo,"ingest":brief})),
        (
            "planner",
            json!({"must_split":false,"findings":[text],"protected_paths":[],"ingest":brief,"forecast":forecast}),
        ),
        (
            "maker",
            json!({"brief":text,"goals":goals,"plan":{"files":["README.md"],"checks":[{
            "name":text,"kind":"regression","tests":["test_a.py"],"goal_ids":["g1"]}]},
            "feedback":[{"kind":"failed_check","report":text,"goal_ids":["g1"],"ci_checks":[{"name":text,"summary":text,"url":null}]}],
            "attempt":1,"round":0,"tests":{"image":format!("sha256:{}","a".repeat(64)),"tools_path":"/fixture"},"context_size":count}),
        ),
        (
            "reviewer",
            json!({"goals":goals,"base_sha":sha,"head_sha":sha,"diff":text,"checks":[],"standing_rules":[]}),
        ),
        (
            "writer",
            json!({"kind":"note","facts":{"issue_title":text,"request":text,"change":text}}),
        ),
        (
            "classifier",
            json!({"comment":{"author":"requester","body":text},"state":{"phase":"Planning","closed":false}}),
        ),
        (
            "postmortem",
            json!({"record":{"id":count,"trigger":"close","reason":"sampled"},
            "case":{"request":{},"outcome":text,"grades":[],"phases":[],"questions":[],"incidents":[],"release":text},
            "rubric":{"version":1,"dimensions":{}},"release":text,"grade_model":text,"deadline_secs":count,
            "trace_tags":{"node":count.to_string(),"request":count.to_string(),"repo":"owner/repo","issue":count.to_string()}}),
        ),
    ]
}

proptest::proptest! {
    /// #730 C1/C2, #658/#663: every role and complete request preserves old json!
    /// wire bytes, validates against Rust-generated schemas and round-trips.
    #[test]
    fn all_typed_role_inputs_round_trip(text in "[a-zA-Z0-9é]{1,64}", count in 1u64..1000) {
        for (role, old_input) in request_input_corpus(&text, count) {
            let typed = RoleInput::parse(role, old_input.clone()).unwrap();
            let document = value(&typed).unwrap();
            proptest::prop_assert_eq!(serde_json::to_vec(&document).unwrap(), serde_json::to_vec(&old_input).unwrap(), "{}", role);
            let validator = input_validator(role);
            let errors = validator.iter_errors(&document).map(|e| e.to_string()).collect::<Vec<_>>();
            proptest::prop_assert!(errors.is_empty(), "{}: {:?}", role, errors);
            let parsed = RoleInput::parse(role, document.clone()).unwrap();
            proptest::prop_assert_eq!(parsed, typed);
            let old_request = json!({"protocol":1,"operation":{"node":0,"generation":count,"kind":role},
                "role":role,"model":{"base_url":"https://model.invalid/v1","name":"fixture","effort":null,"budget_tokens":count},
                "key_file":"/fixture/key","inputs":old_input,"worktree":"/fixture/worktree"});
            let errors = input_validator("request").iter_errors(&old_request).map(|e| e.to_string()).collect::<Vec<_>>();
            proptest::prop_assert!(errors.is_empty(), "request: {:?}", errors);
            let request: WorkerRequest = serde_json::from_value(old_request.clone()).unwrap();
            proptest::prop_assert_eq!(serde_json::to_vec(&value(request).unwrap()).unwrap(), serde_json::to_vec(&old_request).unwrap());
        }
    }
}

/// #730 C1/C2: a required nullable field is present even when it has no value.
#[test]
fn required_nullable_fields_accept_null_and_remain_required() {
    let context = Context {
        schema_version: 1,
        request_version: 0,
        decisions: vec![Decision {
            id: "d1".into(),
            source_comment: None,
            question: json!({}),
            answer: "yes".into(),
            allowed_scope: vec![],
            restrictions: vec![],
            origin_request_version: 0,
            superseded_by: None,
            supersession_reason: None,
        }],
    };
    let mut document = json!({"comment":{"author":"requester","body":"restart"},
        "state":{"phase":"Planning","closed":false},
        "reconcile_issue":{"number":1,"title":"title","body":null,"updated_at":"now"},
        "decision_context":context});
    assert!(input_validator("classifier").is_valid(&document));
    assert_eq!(role_wire("classifier", document.clone()).unwrap(), document);
    document["reconcile_issue"]
        .as_object_mut()
        .unwrap()
        .remove("body");
    assert!(!input_validator("classifier").is_valid(&document));
}

/// #730 C2: generation preserves the old item bounds, including nested lists.
#[test]
fn generated_inputs_keep_string_item_constraints() {
    for (role, mut document) in request_input_corpus("fixture", 1) {
        let changed = match role {
            "ingest" | "audit" | "planner" => {
                document["protected_paths"] = json!([""]);
                true
            }
            "maker" => {
                document["plan"]["files"] = json!(["  "]);
                true
            }
            "reviewer" => {
                document["edited_selected_tests"] = json!([""]);
                true
            }
            _ => false,
        };
        if changed {
            assert!(!input_validator(role).is_valid(&document), "{role}");
        }
    }
}
