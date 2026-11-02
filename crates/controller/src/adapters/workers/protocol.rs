use crate::{Error, Result};
use serde_json::Value;
use std::{collections::BTreeMap, sync::OnceLock};
use tl3_kernel::{EarlyEnd, Reply, Role};

pub(crate) enum MappedReply {
    Kernel(Reply),
    Made(String),
    NeedsFiles { paths: Vec<String>, fits: bool },
}

pub(super) fn role(role: Role) -> Option<&'static str> {
    match role {
        Role::Ingest => Some("ingest"),
        Role::Audit => Some("audit"),
        Role::Make => Some("maker"),
        Role::Plan => Some("planner"),
        Role::Review => Some("reviewer"),
    }
}

pub(super) fn delivery_role(delivery: &crate::executor::Delivery) -> Option<&'static str> {
    use crate::executor::{CommentKind, Output};
    match delivery.output {
        Output::Comment {
            kind: CommentKind::Classify,
            ..
        } => Some("classifier"),
        Output::Effect {
            kind: tl3_kernel::EffectKind::StartRun(r),
            ..
        } => role(r),
        _ => None,
    }
}

pub(crate) fn map_reply(role: &str, reply: &Value) -> Result<MappedReply> {
    let validator = validator(role, "reply")?;
    if !validator.is_valid(reply) {
        return Err(Error::Invalid("worker reply breaks role schema"));
    }
    if role == "maker" {
        return Ok(match reply["kind"].as_str().unwrap() {
            "done"
                if reply["self_check"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|check| check["status"] != "met") =>
            {
                MappedReply::Kernel(Reply::MakerCantMeet)
            }
            "done" => MappedReply::Made(reply["head"].as_str().unwrap().into()),
            "needs_files" => {
                let paths = reply["files"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|item| item["path"].as_str().unwrap().to_owned())
                    .collect::<Vec<_>>();
                if paths.iter().any(|path| !relative_file(path)) {
                    return Err(Error::Invalid("maker requested an invalid path"));
                }
                MappedReply::NeedsFiles {
                    paths,
                    fits: reply["fits"].as_bool().unwrap_or(false),
                }
            }
            "cant_meet" => MappedReply::Kernel(Reply::MakerCantMeet),
            // Q50: an environment blocker is an infrastructure failure. The
            // worker checked its evidence; the incident text cites it.
            "blocked" => MappedReply::Kernel(Reply::RunFailed),
            "out_of_room" => MappedReply::Kernel(Reply::MakerOutOfRoom),
            _ => unreachable!(),
        });
    }
    Ok(MappedReply::Kernel(match role {
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
        "reviewer" if reply["verdict"] == "approve" => Reply::Approved {
            head: tl3_kernel::HeadId(0),
        },
        "reviewer" if reply["verdict"] == "out_of_room" => Reply::ReviewerOutOfRoom,
        "reviewer" => Reply::ChangesRequested,
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
        _ => return Err(Error::Invalid("unsupported worker role")),
    }))
}

pub(super) fn relative_file(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains(['\\', '\0'])
        && path
            .split('/')
            .all(|part| !matches!(part, "" | "." | ".." | ".git"))
}

include!(concat!(env!("OUT_DIR"), "/worker_schemas.rs"));

pub(super) fn initialize_schemas() -> Result<()> {
    validators().map(|_| ())
}

fn validators() -> Result<&'static BTreeMap<String, jsonschema::Validator>> {
    static VALIDATORS: OnceLock<
        std::result::Result<BTreeMap<String, jsonschema::Validator>, String>,
    > = OnceLock::new();
    VALIDATORS
        .get_or_init(|| {
            let schemas = PACKAGED_SCHEMAS
                .iter()
                .map(|(name, source)| {
                    serde_json::from_str(source)
                        .map(|schema| ((*name).to_owned(), schema))
                        .map_err(|error| format!("packaged worker schema {name}: {error}"))
                })
                .collect::<std::result::Result<Vec<_>, _>>()?;
            schemas_from(schemas).map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(|error| Error::Adapter(error.clone().into()))
}

fn validator(role: &str, direction: &str) -> Result<&'static jsonschema::Validator> {
    validators()?
        .get(&format!("{role}-{direction}.json"))
        .ok_or(Error::Invalid("unsupported worker role"))
}

pub(super) fn validate_inputs(role: &str, inputs: &Value) -> Result<()> {
    if validator(role, "input")?.is_valid(inputs) {
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
    let declared: std::collections::BTreeSet<_> = if reply["kind"] == "split" {
        reply["pieces"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|piece| piece["files"].as_array().into_iter().flatten())
            .filter_map(Value::as_str)
            .collect()
    } else {
        reply["files"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect()
    };
    let measured: std::collections::BTreeSet<_> = files.keys().map(String::as_str).collect();
    if declared != measured {
        return Err(Error::Invalid("forecast files differ from declared files"));
    }
    Ok((tokens, fit))
}

fn classifier_schema(value: &Value, reply: bool) -> Result<()> {
    let validator = validator("classifier", if reply { "reply" } else { "input" })?;
    if validator.is_valid(value) {
        Ok(())
    } else {
        Err(Error::Invalid("classifier value breaks role schema"))
    }
}

pub(crate) fn validate_classification(inputs: &Value, reply: &Value) -> Result<()> {
    classifier_schema(reply, true)?;
    let closed = inputs["state"]["closed"] == true;
    let kind = reply["kind"].as_str().unwrap();
    if (kind == "restart" && inputs["state"]["restartable"] != true)
        || (closed && !matches!(kind, "complaint" | "remark" | "unsure" | "restart"))
        || (!closed && kind == "complaint")
    {
        return Err(Error::Invalid(
            "classification does not apply to request state",
        ));
    }
    if kind == "answer" {
        let question = &inputs["state"]["open_question"];
        if question.is_null()
            || reply["question_id"] != question["id"]
            || (reply["answer"] != "unclear"
                && !question["answers"]
                    .as_array()
                    .is_some_and(|a| a.contains(&reply["answer"])))
            || (reply["answer"] == "add_credit" && reply["amount"].as_u64().is_none())
        {
            return Err(Error::Invalid(
                "classifier answer does not name its offered question",
            ));
        }
    }
    if (kind == "change_request" || reply["answer"] == "change_request")
        && reply["new_text"].as_str().unwrap().trim().is_empty()
    {
        return Err(Error::Invalid("changed request text is empty"));
    }
    Ok(())
}

pub(super) fn validate_forecast_settings(reply: &Value, settings: &Value) -> Result<()> {
    if let Some(forecast) = reply.get("forecast") {
        let overhead = settings["overhead_tokens"].as_object().and_then(|values| {
            values
                .values()
                .try_fold(0u64, |sum, value| sum.checked_add(value.as_u64()?))
        });
        if forecast["fit_tokens"] != settings["fit_tokens"]
            || forecast["context_tokens"] != settings["context_tokens"]
            || forecast["overhead_tokens"].as_u64() != overhead
        {
            return Err(Error::Invalid(
                "forecast differs from configured limits or overhead",
            ));
        }
    }
    Ok(())
}

fn schemas_from(schemas: Vec<(String, Value)>) -> Result<BTreeMap<String, jsonschema::Validator>> {
    let mut registry = jsonschema::Registry::new();
    for (name, schema) in &schemas {
        let uri = format!("https://trustladderiii.invalid/workers/protocol-1/{name}");
        registry = registry.add(uri, schema.clone()).map_err(|error| {
            Error::Adapter(format!("packaged worker schema {name}: {error}").into())
        })?;
    }
    let registry = registry.prepare().map_err(|error| {
        Error::Adapter(format!("packaged worker schema registry: {error}").into())
    })?;
    schemas
        .into_iter()
        .map(|(name, schema)| {
            let validator = jsonschema::options()
                .with_registry(&registry)
                .build(&schema)
                .map_err(|error| {
                    Error::Adapter(format!("packaged worker schema {name}: {error}").into())
                })?;
            Ok((name, validator))
        })
        .collect()
}

pub(crate) fn reply_field<T: serde::de::DeserializeOwned>(reply: &Value, field: &str) -> Result<T> {
    let value = reply
        .get(field)
        .ok_or(Error::Invalid("worker reply field missing"))?;
    serde_json::from_value(value.clone()).map_err(|_| Error::Invalid("invalid worker reply field"))
}

#[cfg(test)]
#[path = "protocol_tests.rs"]
mod tests;

#[cfg(test)]
mod classifier_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn classifier_rejects_unsafe_or_unoffered_answers() {
        let mut inputs = json!({"comment": {"author": "owner", "body": "continue"}, "state": {
            "phase": "AwaitingRob", "closed": false,
            "open_question": {"id": "17", "kind": "out_of_credit", "answers": ["add_credit", "change_request", "stop"]}
        }});
        validate_inputs("classifier", &inputs).unwrap();
        for amount in [1, 100] {
            validate_classification(&inputs, &json!({"kind": "answer", "question_id": "17", "answer": "add_credit", "amount": amount})).unwrap();
        }
        for reply in [
            json!({"kind": "answer", "question_id": "16", "answer": "stop"}),
            json!({"kind": "answer", "question_id": "17", "answer": "continue"}),
            json!({"kind": "answer", "question_id": "17", "answer": "add_credit", "amount": 0}),
            json!({"kind": "answer", "question_id": "17", "answer": "add_credit", "amount": 101}),
            json!({"kind": "answer", "question_id": "17", "answer": "add_credit", "amount": 1.0}),
            json!({"kind": "change_request", "new_text": " "}),
            json!({"kind": "pause", "extra": true}),
            json!({"kind": "complaint", "summary": "Broken", "revert": true}),
        ] {
            assert!(validate_classification(&inputs, &reply).is_err(), "{reply}");
        }
        inputs["state"]["closed"] = json!(true);
        inputs["state"]["open_question"] = Value::Null;
        for kind in ["pause", "resume", "stop"] {
            assert!(validate_classification(&inputs, &json!({"kind": kind})).is_err());
        }
        for revert in [false, true] {
            validate_classification(
                &inputs,
                &json!({"kind": "complaint", "summary": "Broken", "revert": revert}),
            )
            .unwrap();
        }
        assert!(
            validate_classification(&inputs, &json!({"kind": "complaint", "summary": "Broken"}))
                .is_err()
        );
    }

    #[test]
    fn restart_needs_a_restartable_request() {
        let restart = json!({"kind": "restart"});
        for (closed, restartable, valid) in [
            (false, Some(true), true),
            (true, Some(true), true),
            (true, Some(false), false),
            (false, Some(false), false),
            (true, None, false),
        ] {
            let mut inputs = json!({"comment": {"author": "owner", "body": "Restart this."},
                "state": {"phase": "Making", "closed": closed, "open_question": null}});
            if let Some(restartable) = restartable {
                inputs["state"]["restartable"] = json!(restartable);
            }
            validate_inputs("classifier", &inputs).unwrap();
            assert_eq!(
                validate_classification(&inputs, &restart).is_ok(),
                valid,
                "closed={closed} restartable={restartable:?}"
            );
        }
        let inputs = json!({"comment": {"author": "owner", "body": "Restart this."},
            "state": {"phase": "Making", "closed": false, "restartable": true}});
        assert!(
            validate_classification(
                &inputs,
                &json!({"kind": "restart", "new_text": "Return goodbye."})
            )
            .is_err()
        );
    }

    #[test]
    fn unsure_is_one_nonempty_line_of_at_most_300_characters() {
        let inputs = json!({"state": {"closed": false}});
        validate_classification(
            &inputs,
            &json!({"kind": "unsure", "question": "é".repeat(300)}),
        )
        .unwrap();
        for question in [
            "".into(),
            " ".into(),
            "é".repeat(301),
            "First?\nSecond?".into(),
            "First?\rSecond?".into(),
            "First?\u{2028}Second?".into(),
        ] {
            assert!(
                validate_classification(&inputs, &json!({"kind": "unsure", "question": question}))
                    .is_err()
            );
        }
    }
}

#[cfg(test)]
mod reply_defects {
    use super::*;
    use serde_json::json;

    #[test]
    fn registry_resolves_a_new_cross_schema_reference() {
        let schemas = vec![
            (
                "new-input.json".into(),
                json!({"$id":"https://trustladderiii.invalid/workers/protocol-1/new-input.json", "$ref":"maker-input.json#/properties/goals/items"}),
            ),
            (
                "maker-input.json".into(),
                serde_json::from_str(include_str!(
                    "../../../../../workers/src/tl3_workers/schemas/maker-input.json"
                ))
                .unwrap(),
            ),
        ];
        let validators = schemas_from(schemas).unwrap();
        assert!(
            validators["new-input.json"]
                .is_valid(&json!({"id":"g", "description":"Hello", "verification":"test"}))
        );
        assert!(!validators["new-input.json"].is_valid(&json!({})));
    }

    #[test]
    fn schema_load_errors_are_returned() {
        for schema in [
            json!({"type":"unknown"}),
            json!({"$ref":"https://trustladderiii.invalid/missing.json"}),
        ] {
            assert!(schemas_from(vec![("broken.json".into(), schema)]).is_err());
        }
    }
}

#[cfg(test)]
mod decoding_defects {
    use super::*;
    use serde_json::json;
    #[test]
    fn planner_arrays_reject_missing_and_wrong_item_types() {
        for (field, reply) in [
            ("violations", json!({})),
            ("violations", json!({"violations": [1]})),
            ("files", json!({"files":null})),
            ("files", json!({"files":["ok.py",false]})),
        ] {
            assert!(matches!(
                reply_field::<Vec<String>>(&reply, field),
                Err(Error::Invalid(_))
            ));
        }
        assert_eq!(
            reply_field::<Vec<String>>(&json!({"files":["ok.py"]}), "files").unwrap(),
            vec!["ok.py"]
        );
    }
    #[test]
    fn maker_self_check_must_be_an_array() {
        assert!(matches!(
            reply_field::<Vec<Value>>(&json!({"self_check":false}), "self_check"),
            Err(Error::Invalid(_))
        ));
    }
}
