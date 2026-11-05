use crate::{Error, Result};
use serde_json::Value;
use std::{collections::BTreeMap, sync::OnceLock};
use tl3_kernel::{EarlyEnd, Reply, Role};

#[derive(Debug, PartialEq, Eq)]
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

pub(crate) fn delivery_role(delivery: &crate::executor::Delivery) -> Option<&'static str> {
    use crate::executor::{CommentKind, Output};
    match delivery.output {
        Output::Comment {
            kind: CommentKind::Classify,
            ..
        } => Some("classifier"),
        Output::Comment {
            kind: CommentKind::Postmortem,
            ..
        } => Some("postmortem"),
        Output::Effect {
            kind: tl3_kernel::EffectKind::StartRun(r),
            ..
        } => role(r),
        _ => None,
    }
}

/// JSON boundary retained for controller observations stored as protocol v1 Values.
pub(crate) fn map_reply(role: &str, reply: &Value) -> Result<MappedReply> {
    map_typed_reply(&crate::contracts::replies::RoleReply::parse(role, reply)?)
}

/// The decision consumes only typed variants; parsing and schema errors occur above it.
pub(crate) fn map_typed_reply(reply: &crate::contracts::replies::RoleReply) -> Result<MappedReply> {
    use crate::contracts::replies::decision::{self, MappedReply as Mapping, RoleReply as Input};
    use crate::contracts::replies::{
        RoleReply,
        audit::AuditReply,
        ingest::{IngestReply, IngestReplyEarlyEndOutcome},
        maker::{MakerReply, MakerReplyDoneSelfCheckItem},
        reviewer::ReviewerReply,
    };
    let input = match reply {
        RoleReply::Maker(MakerReply::Done(done)) => Input::MakerDone {
            head: done.head.as_str(),
            rejected: done
                .self_check
                .iter()
                .any(|check| matches!(check, MakerReplyDoneSelfCheckItem::NotMet(_)))
                || crate::budget::decision::reject_done(
                    true,
                    done.budget_exhaustion
                        .as_ref()
                        .is_some_and(|budget| !budget.0.unfinished_obligations.is_empty()),
                ),
        },
        RoleReply::Maker(MakerReply::NeedsFiles(files)) => Input::MakerNeedsFiles(files),
        RoleReply::Maker(MakerReply::CantMeet(_)) => Input::MakerCantMeet,
        RoleReply::Maker(MakerReply::Blocked(_)) => Input::MakerBlocked,
        RoleReply::Maker(MakerReply::OutOfRoom(_)) => Input::MakerOutOfRoom,
        RoleReply::Ingest(IngestReply::Brief(brief)) => Input::IngestBrief(brief),
        RoleReply::Ingest(IngestReply::EarlyEnd(early)) => match early.outcome {
            IngestReplyEarlyEndOutcome::Duplicate => Input::IngestDuplicate,
            IngestReplyEarlyEndOutcome::AlreadyDone => Input::IngestAlreadyDone,
            IngestReplyEarlyEndOutcome::NotActionable => Input::IngestNotActionable,
        },
        RoleReply::Audit(AuditReply::Agree(_)) => Input::AuditAgree,
        RoleReply::Audit(AuditReply::Disagree(_)) => Input::AuditDisagree,
        RoleReply::Reviewer(ReviewerReply::Approve(_)) => Input::ReviewApprove,
        RoleReply::Reviewer(ReviewerReply::Changes(_)) => Input::ReviewChanges,
        RoleReply::Reviewer(ReviewerReply::OutOfRoom(_)) => Input::ReviewOutOfRoom,
        RoleReply::Planner(reply) => {
            forecast(reply)?;
            let violations = reply["violations"]
                .as_array()
                .ok_or(Error::Invalid("planner violations missing"))?;
            Input::Planner {
                violations: !violations.is_empty(),
                cant_meet: reply["kind"] == "cant_meet",
                split: reply["kind"] == "split",
                fits: reply["forecast"]["fits"] == true,
            }
        }
        RoleReply::Classifier(_) => Input::Classifier,
        RoleReply::Writer(_) => Input::Writer,
        RoleReply::Postmortem(_) => Input::Postmortem,
    };
    Ok(match decision::map_reply(input) {
        Mapping::Made(head) => MappedReply::Made(head.into()),
        Mapping::NeedsFiles(request) => {
            let paths = request
                .files
                .iter()
                .map(|file| file.path.0.as_str().to_owned())
                .collect::<Vec<_>>();
            if paths.iter().any(|path| !relative_file(path)) {
                return Err(Error::Invalid("maker requested an invalid path"));
            }
            MappedReply::NeedsFiles {
                paths,
                fits: request.fits.unwrap_or(false),
            }
        }
        Mapping::Ingest(brief) => {
            let (tokens, fit) = typed_forecast(brief)?;
            let size = tokens
                .div_ceil(fit)
                .max(1)
                .try_into()
                .map_err(|_| Error::Invalid("request size exceeds kernel range"))?;
            MappedReply::Kernel(Reply::IngestDone { size })
        }
        Mapping::MakerCantMeet => MappedReply::Kernel(Reply::MakerCantMeet),
        Mapping::RunFailed => MappedReply::Kernel(Reply::RunFailed),
        Mapping::MakerOutOfRoom => MappedReply::Kernel(Reply::MakerOutOfRoom),
        Mapping::IngestDuplicate => MappedReply::Kernel(Reply::IngestEarlyEnd(EarlyEnd::Duplicate)),
        Mapping::IngestAlreadyDone => {
            MappedReply::Kernel(Reply::IngestEarlyEnd(EarlyEnd::AlreadyDone))
        }
        Mapping::IngestNotActionable => {
            MappedReply::Kernel(Reply::IngestEarlyEnd(EarlyEnd::NotActionable))
        }
        Mapping::AuditAgrees => MappedReply::Kernel(Reply::AuditAgrees),
        Mapping::AuditDisagrees => MappedReply::Kernel(Reply::AuditDisagrees),
        Mapping::Approved => MappedReply::Kernel(Reply::Approved {
            head: tl3_kernel::HeadId(0),
        }),
        Mapping::ChangesRequested => MappedReply::Kernel(Reply::ChangesRequested),
        Mapping::ReviewerOutOfRoom => MappedReply::Kernel(Reply::ReviewerOutOfRoom),
        Mapping::PlanCantMeet => MappedReply::Kernel(Reply::PlanCantMeet),
        Mapping::PlanSplit => MappedReply::Kernel(Reply::PlanSplit),
        Mapping::PlanLeaf { fits } => MappedReply::Kernel(Reply::PlanLeaf {
            footprint: tl3_kernel::Footprint::default(),
            fits,
        }),
        Mapping::Unsupported => return Err(Error::Invalid("unsupported worker role")),
    })
}

fn typed_forecast(
    brief: &crate::contracts::replies::ingest::IngestReplyBrief,
) -> Result<(u64, u64)> {
    let f = &brief
        .forecast
        .as_ref()
        .ok_or(Error::Invalid("forecast missing"))?
        .0;
    let tokens = f
        .tokens
        .as_u64()
        .ok_or(Error::Invalid("forecast missing"))?;
    let fit = f
        .fit_tokens
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or(Error::Invalid("forecast limit missing"))?;
    let context = f
        .context_tokens
        .as_u64()
        .ok_or(Error::Invalid("forecast context missing"))?;
    let files = f
        .file_tokens
        .iter()
        .map(|(path, value)| {
            Ok((
                path.clone(),
                value
                    .as_u64()
                    .ok_or(Error::Invalid("inconsistent forecast evidence"))?,
            ))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let expected = crate::budget::decide(crate::budget::Request::Forecast {
        brief_tokens: f
            .brief_tokens
            .as_u64()
            .ok_or(Error::Invalid("inconsistent forecast evidence"))?,
        file_tokens: files,
        overhead_tokens: f
            .overhead_tokens
            .as_u64()
            .ok_or(Error::Invalid("inconsistent forecast evidence"))?,
        fit_tokens: fit,
        context_tokens: context,
        problems: 0,
    })
    .map_err(|_| Error::Invalid("inconsistent forecast evidence"))?;
    if expected["tokens"] != tokens || fit >= context || expected["fits"] != f.fits {
        return Err(Error::Invalid("inconsistent forecast evidence"));
    }
    let declared: std::collections::BTreeSet<_> = brief
        .files
        .iter()
        .flat_map(|files| files.iter().map(|file| file.as_str()))
        .collect();
    let measured: std::collections::BTreeSet<_> =
        f.file_tokens.keys().map(String::as_str).collect();
    if declared != measured {
        return Err(Error::Invalid("forecast files differ from declared files"));
    }
    Ok((tokens, fit))
}

pub(crate) fn relative_file(path: &str) -> bool {
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

/// A reply checked against its role's schema alone, for replies no kernel role maps.
pub(crate) fn validate_reply_schema(role: &str, reply: &Value) -> Result<()> {
    if validator(role, "reply")?.is_valid(reply) {
        Ok(())
    } else {
        Err(Error::Invalid("worker reply breaks role schema"))
    }
}

pub(crate) fn validate_reply(role: &str, reply: &Value) -> Result<()> {
    crate::contracts::replies::RoleReply::parse(role, reply).map(|_| ())
}

pub(super) fn validate_inputs(role: &str, inputs: &Value) -> Result<()> {
    match validator(role, "input")?.validate(inputs) {
        Ok(()) => Ok(()),
        Err(error) => Err(crate::worker_inputs::Failure::capture(role, &error).into()),
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
    let expected = crate::budget::decide(crate::budget::Request::Forecast {
        brief_tokens: f["brief_tokens"]
            .as_u64()
            .ok_or(Error::Invalid("inconsistent forecast evidence"))?,
        file_tokens: files
            .iter()
            .map(|(path, value)| {
                Ok((
                    path.clone(),
                    value
                        .as_u64()
                        .ok_or(Error::Invalid("inconsistent forecast evidence"))?,
                ))
            })
            .collect::<Result<_>>()?,
        overhead_tokens: f["overhead_tokens"]
            .as_u64()
            .ok_or(Error::Invalid("inconsistent forecast evidence"))?,
        fit_tokens: fit,
        context_tokens: context,
        problems: 0,
    })
    .map_err(|_| Error::Invalid("inconsistent forecast evidence"))?;
    if expected["tokens"] != tokens || fit >= context || f["fits"] != expected["fits"] {
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

pub(crate) fn classifier_schema(value: &Value, reply: bool) -> Result<()> {
    let validator = validator("classifier", if reply { "reply" } else { "input" })?;
    if validator.is_valid(value) {
        Ok(())
    } else {
        Err(Error::Invalid("classifier value breaks role schema"))
    }
}

pub(crate) fn reviewer_schema(value: &Value, reply: bool) -> Result<()> {
    if validator("reviewer", if reply { "reply" } else { "input" })?.is_valid(value) {
        Ok(())
    } else {
        Err(Error::Invalid("reviewer value breaks role schema"))
    }
}

pub(crate) use crate::classifier::validate as validate_classification;

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

/// One input property's packaged schema, for contract tests of a projection (#734).
#[cfg(test)]
pub(super) fn input_property_validator(role: &str, property: &str) -> jsonschema::Validator {
    let mut schemas = PACKAGED_SCHEMAS
        .iter()
        .map(|(name, source)| ((*name).to_owned(), serde_json::from_str(source).unwrap()))
        .collect::<Vec<_>>();
    let name = format!("{role}-input-{property}.json");
    schemas.push((
        name.clone(),
        serde_json::json!({"$id": format!("https://trustladderiii.invalid/workers/protocol-1/{name}"),
            "$ref": format!("{role}-input.json#/properties/{property}")}),
    ));
    schemas_from(schemas).unwrap().remove(&name).unwrap()
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
        let inputs = json!({"comment": {"author":"owner", "body":"unsure"}, "state": {"phase":"Making", "closed": false}});
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
        let mut schemas = PACKAGED_SCHEMAS
            .iter()
            .map(|(name, source)| ((*name).to_owned(), serde_json::from_str(source).unwrap()))
            .collect::<Vec<_>>();
        schemas.extend([
            (
                "decision-context.json".into(),
                serde_json::from_str(include_str!(
                    "../../../../../workers/src/tl3_workers/schemas/decision-context.json"
                ))
                .unwrap(),
            ),
            (
                "new-input.json".into(),
                json!({"$id":"https://trustladderiii.invalid/workers/protocol-1/new-input.json", "$ref":"maker-input.json#/properties/goals/items"}),
            ),
            (
                "new-forecast.json".into(),
                json!({"$id":"https://trustladderiii.invalid/workers/protocol-1/new-forecast.json", "$ref":"maker-input.json#/properties/forecast"}),
            ),
        ]);
        let validators = schemas_from(schemas).unwrap();
        assert!(
            validators["new-input.json"]
                .is_valid(&json!({"id":"g", "description":"Hello", "verification":"test"}))
        );
        assert!(!validators["new-input.json"].is_valid(&json!({})));
        let mut forecast = json!({"fit_tokens":100000, "context_tokens":150000,
            "overhead_tokens":{"prompt":5000, "making":20000}});
        assert!(validators["new-forecast.json"].is_valid(&forecast));
        forecast["overhead_tokens"]["making"] = json!(-1);
        assert!(!validators["new-forecast.json"].is_valid(&forecast));
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

#[cfg(test)]
#[path = "../../contracts/replies/reference_map.rs"]
mod reference_map;
