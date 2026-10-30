use crate::{Error, Result};
use serde_json::Value;
use std::sync::OnceLock;
use tl3_kernel::{EarlyEnd, Reply, Role};

pub(super) fn role(role: Role) -> Option<&'static str> {
    match role {
        Role::Ingest => Some("ingest"),
        Role::Audit => Some("audit"),
        Role::Plan => Some("planner"),
        Role::Classify => Some("classifier"),
        _ => None,
    }
}

pub(crate) fn map_reply(role: &str, reply: &Value) -> Result<Reply> {
    static INGEST: OnceLock<jsonschema::Validator> = OnceLock::new();
    static PLANNER: OnceLock<jsonschema::Validator> = OnceLock::new();
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
        "planner" => (
            &PLANNER,
            include_str!("../../../../../workers/src/tl3_workers/schemas/planner-reply.json"),
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
        "ingest" if reply["kind"] == "brief" => {
            let forecast = forecast(reply)?;
            let size = forecast
                .0
                .div_ceil(forecast.1)
                .max(1)
                .try_into()
                .map_err(|_| Error::Invalid("request size exceeds kernel range"))?;
            Reply::IngestDone { size }
        }
        "ingest" => Reply::IngestEarlyEnd(match reply["outcome"].as_str() {
            Some("duplicate") => EarlyEnd::Duplicate,
            Some("already_done") => EarlyEnd::AlreadyDone,
            Some("not_actionable") => EarlyEnd::NotActionable,
            _ => unreachable!("validated early outcome"),
        }),
        "audit" if reply["verdict"] == "agree" => Reply::AuditAgrees,
        "audit" => Reply::AuditDisagrees,
        "planner" => {
            forecast(reply)?;
            let violations = reply["violations"]
                .as_array()
                .ok_or(Error::Invalid("planner violations missing"))?;
            if !violations.is_empty() || reply["kind"] == "cant_meet" {
                Reply::PlanCantMeet
            } else if reply["kind"] == "split" {
                Reply::PlanSplit
            } else {
                Reply::PlanLeaf {
                    footprint: tl3_kernel::Footprint::default(),
                    fits: reply["forecast"]["fits"] == true,
                }
            }
        }
        _ => unreachable!(),
    })
}

pub(super) fn validate_inputs(role: &str, inputs: &Value) -> Result<()> {
    if role == "classifier" {
        return classifier_schema(inputs, false);
    }
    static VALIDATORS: OnceLock<(
        jsonschema::Validator,
        jsonschema::Validator,
        jsonschema::Validator,
    )> = OnceLock::new();
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
        let planner: Value = serde_json::from_str(include_str!(
            "../../../../../workers/src/tl3_workers/schemas/planner-input.json"
        ))
        .unwrap();
        let mut registry = jsonschema::Registry::new();
        for schema in [&ingest, &audit, &reply, &planner] {
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
            jsonschema::options()
                .with_registry(&registry)
                .build(&planner)
                .unwrap(),
        )
    });
    let validator = if role == "ingest" {
        &validators.0
    } else if role == "audit" {
        &validators.1
    } else if role == "planner" {
        &validators.2
    } else {
        return Err(Error::Invalid("unsupported worker role"));
    };
    if validator.is_valid(inputs) {
        Ok(())
    } else {
        Err(Error::Invalid("worker inputs break role schema"))
    }
}

fn forecast(reply: &Value) -> Result<(u64, u64)> {
    let f = &reply["forecast"];
    let tokens = f["tokens"]
        .as_u64()
        .ok_or(Error::Invalid("forecast missing"))?;
    let fit = f["fit_tokens"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or(Error::Invalid("forecast limit missing"))?;
    let context = f["context_tokens"]
        .as_u64()
        .ok_or(Error::Invalid("forecast context missing"))?;
    let files = f["file_tokens"]
        .as_object()
        .ok_or(Error::Invalid("forecast files missing"))?;
    let sum = files
        .values()
        .try_fold(0u64, |sum, v| sum.checked_add(v.as_u64()?))
        .and_then(|n| n.checked_add(f["brief_tokens"].as_u64()?))
        .and_then(|n| n.checked_add(f["overhead_tokens"].as_u64()?));
    if sum != Some(tokens) || fit >= context || f["fits"].as_bool() != Some(tokens < fit) {
        return Err(Error::Invalid("inconsistent forecast evidence"));
    }
    Ok((tokens, fit))
}

fn classifier_schema(value: &Value, reply: bool) -> Result<()> {
    static INPUT: OnceLock<jsonschema::Validator> = OnceLock::new();
    static REPLY: OnceLock<jsonschema::Validator> = OnceLock::new();
    let (cell, schema) = if reply {
        (&REPLY, include_str!("../../../../../workers/src/tl3_workers/schemas/classifier-reply.json"))
    } else {
        (&INPUT, include_str!("../../../../../workers/src/tl3_workers/schemas/classifier-input.json"))
    };
    let validator = cell.get_or_init(|| jsonschema::validator_for(&serde_json::from_str::<Value>(schema).unwrap()).unwrap());
    if validator.is_valid(value) { Ok(()) } else { Err(Error::Invalid("classifier value breaks role schema")) }
}

pub(crate) fn validate_classification(inputs: &Value, reply: &Value) -> Result<()> {
    classifier_schema(reply, true)?;
    let closed = inputs["state"]["closed"] == true;
    let kind = reply["kind"].as_str().unwrap();
    if (closed && !matches!(kind, "complaint" | "remark" | "unsure")) || (!closed && kind == "complaint") {
        return Err(Error::Invalid("classification does not apply to request state"));
    }
    if kind == "answer" {
        let question = &inputs["state"]["open_question"];
        if question.is_null() || reply["question_id"] != question["id"]
            || !question["answers"].as_array().is_some_and(|a| a.contains(&reply["answer"]))
            || (reply["answer"] == "add_credit" && reply["amount"].as_u64().is_none()) {
            return Err(Error::Invalid("classifier answer does not name its offered question"));
        }
    }
    if (kind == "change_request" || reply["answer"] == "change_request") && reply["new_text"].as_str().unwrap().trim().is_empty() {
        return Err(Error::Invalid("changed request text is empty"));
    }
    Ok(())
}
