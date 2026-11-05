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
