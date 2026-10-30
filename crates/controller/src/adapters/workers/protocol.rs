use crate::{Error, Result};
use serde_json::Value;
use std::sync::OnceLock;
use tl3_kernel::{EarlyEnd, Reply, Role};

pub(super) fn role(role: Role) -> Option<&'static str> {
    match role {
        Role::Ingest => Some("ingest"),
        Role::Audit => Some("audit"),
        _ => None,
    }
}

pub(crate) fn map_reply(role: &str, reply: &Value) -> Result<Reply> {
    static INGEST: OnceLock<jsonschema::Validator> = OnceLock::new();
    static AUDIT: OnceLock<jsonschema::Validator> = OnceLock::new();
    let (cell, schema) = match role {
        "ingest" => (
            &INGEST,
            include_str!("../../../../../workers/src/tl3_workers/schemas/ingest-reply.json"),
        ),
        "audit" => (
            &AUDIT,
            include_str!("../../../../../workers/src/tl3_workers/schemas/audit-reply.json"),
        ),
        _ => return Err(Error::Invalid("unsupported worker role")),
    };
    let validator = cell.get_or_init(|| {
        let schema: Value = serde_json::from_str(schema).expect("packaged worker schema");
        jsonschema::validator_for(&schema).expect("valid offline worker schema")
    });
    if !validator.is_valid(reply) {
        return Err(Error::Invalid("worker reply breaks role schema"));
    }
    Ok(match role {
        // The forecaster owns sizing in #11. Protocol v1 has no size field.
        "ingest" if reply["kind"] == "brief" => Reply::IngestDone { size: 1 },
        "ingest" => Reply::IngestEarlyEnd(match reply["outcome"].as_str() {
            Some("duplicate") => EarlyEnd::Duplicate,
            Some("already_done") => EarlyEnd::AlreadyDone,
            Some("not_actionable") => EarlyEnd::NotActionable,
            _ => unreachable!("validated early outcome"),
        }),
        "audit" if reply["verdict"] == "agree" => Reply::AuditAgrees,
        "audit" => Reply::AuditDisagrees,
        _ => unreachable!(),
    })
}

pub(super) fn validate_inputs(role: &str, inputs: &Value) -> Result<()> {
    static VALIDATORS: OnceLock<(jsonschema::Validator, jsonschema::Validator)> = OnceLock::new();
    let validators = VALIDATORS.get_or_init(|| {
        let ingest: Value = serde_json::from_str(include_str!(
            "../../../../../workers/src/tl3_workers/schemas/ingest-input.json"
        ))
        .unwrap();
        let audit: Value = serde_json::from_str(include_str!(
            "../../../../../workers/src/tl3_workers/schemas/audit-input.json"
        ))
        .unwrap();
        let reply: Value = serde_json::from_str(include_str!(
            "../../../../../workers/src/tl3_workers/schemas/ingest-reply.json"
        ))
        .unwrap();
        let mut registry = jsonschema::Registry::new();
        for schema in [&ingest, &audit, &reply] {
            registry = registry
                .add(schema["$id"].as_str().unwrap(), schema.clone())
                .unwrap();
        }
        let registry = registry.prepare().unwrap();
        (
            jsonschema::options()
                .with_registry(&registry)
                .build(&ingest)
                .unwrap(),
            jsonschema::options()
                .with_registry(&registry)
                .build(&audit)
                .unwrap(),
        )
    });
    let validator = if role == "ingest" {
        &validators.0
    } else {
        &validators.1
    };
    if validator.is_valid(inputs) {
        Ok(())
    } else {
        Err(Error::Invalid("worker inputs break role schema"))
    }
}
