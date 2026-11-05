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
        for case in corpus.iter().filter(|case| case["role"] == *role) {
            let valid = validator.is_valid(&case["value"]);
            accepted += usize::from(valid);
            assert_eq!(
                RoleReply::parse(role, &case["value"]).is_ok(),
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
