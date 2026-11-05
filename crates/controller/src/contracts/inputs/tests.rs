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
