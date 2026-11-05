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
                    .any(|check| check["status"] == "not_met")
                    || crate::budget::decision::reject_done(
                        true,
                        reply["budget_exhaustion"]["unfinished_obligations"]
                            .as_array()
                            .is_some_and(|items| !items.is_empty()),
                    ) =>
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
        "planner" => planner_reply::map(reply)?,
        _ => return Err(Error::Invalid("unsupported worker role")),
    }))
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
pub(crate) fn validate_reply(role: &str, reply: &Value) -> Result<()> {
    if validator(role, "reply")?.is_valid(reply) {
        Ok(())
    } else {
        Err(Error::Invalid("worker reply breaks role schema"))
    }
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

mod planner_reply {
    use crate::{Error, Result, contracts::roles::planner_reply::PlannerReply};
    use serde_json::Value;
    use tl3_kernel::Reply;

    pub(super) fn map(reply: &Value) -> Result<Reply> {
        // Preserve Q54's error precedence: forecast evidence precedes violations and
        // planner decisions. The schema boundary has already checked the wire shape.
        super::forecast(reply)?;
        let violations = reply["violations"]
            .as_array()
            .ok_or(Error::Invalid("planner violations missing"))?;
        if !violations.is_empty() {
            return Ok(Reply::PlanCantMeet);
        }
        let typed: PlannerReply = serde_json::from_value(reply.clone())
            .map_err(|_| Error::Invalid("worker reply breaks role schema"))?;
        Ok(match typed {
            PlannerReply::CantMeet(_) => Reply::PlanCantMeet,
            PlannerReply::Split(_) => Reply::PlanSplit,
            PlannerReply::Leaf(leaf) => Reply::PlanLeaf {
                footprint: tl3_kernel::Footprint::default(),
                fits: leaf.forecast.is_some_and(|forecast| forecast.fits),
            },
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::adapters::workers::protocol::{MappedReply, map_reply};
        use serde_json::json;

        // Verbatim planner arm from cdacce8f, with the original schema boundary. Kept as
        // the test-only migration oracle until P2b ships; no sampled I/O occurs here.
        fn legacy(reply: &Value) -> Result<Reply> {
            let validator = super::super::validator("planner", "reply")?;
            if !validator.is_valid(reply) {
                return Err(Error::Invalid("worker reply breaks role schema"));
            }
            super::super::forecast(reply)?;
            let violations = reply["violations"]
                .as_array()
                .ok_or(Error::Invalid("planner violations missing"))?;
            if !violations.is_empty() || reply["kind"] == "cant_meet" {
                Ok(Reply::PlanCantMeet)
            } else if reply["kind"] == "split" {
                Ok(Reply::PlanSplit)
            } else {
                Ok(Reply::PlanLeaf {
                    footprint: tl3_kernel::Footprint::default(),
                    fits: reply["forecast"]["fits"] == true,
                })
            }
        }

        #[test]
        fn planner_reply_mapping_matches_pre_extraction_arm() {
            let forecast = json!({"tokenizer":"o200k_base", "brief_tokens":1,
                "file_tokens":{"a.py":0}, "overhead_tokens":0,"tokens":1,
                "fit_tokens":100,"context_tokens":150,"fits":true});
            let cases = [
                json!({"kind":"leaf", "files":["a.py"], "checks":[],
                    "no_design_questions":true,"lands_on_its_own":true}),
                json!({"kind":"split","reason":"out_of_room","pieces":[
                    {"brief":"First", "goal_ids":[0],"files":["a.py"]}]}),
                json!({"kind":"cant_meet","report":"Cannot fit.","files":["a.py"]}),
            ];
            for mut case in cases {
                case["forecast"] = forecast.clone();
                for violations in [json!([]), json!(["Cannot proceed."])] {
                    case["violations"] = violations;
                    let expected = legacy(&case).unwrap();
                    assert_eq!(map(&case).unwrap(), expected, "{case}");
                }
            }
        }

        #[test]
        fn planner_reply_preserves_errors_and_forecast_precedence() {
            let leaf = serde_json::json!({"kind":"leaf","files":[],"checks":[],
                "no_design_questions":true,"lands_on_its_own":true,
                "forecast":{"tokenizer":"o200k_base","brief_tokens":0,"file_tokens":{},
                    "overhead_tokens":0,"tokens":0,"fit_tokens":100,"context_tokens":150,"fits":true},
                "violations":[]});
            let mut cases = vec![leaf.clone()];
            for key in ["forecast", "violations", "files", "kind"] {
                let mut case = leaf.clone();
                case.as_object_mut().unwrap().remove(key);
                cases.push(case);
            }
            for (path, value) in [
                ("/forecast/tokens", serde_json::json!(0.0)),
                ("/forecast/tokens", serde_json::json!(1)),
                ("/violations", serde_json::json!(["blocked"])),
                ("/files", serde_json::json!(["a.py", "a.py"])),
                (
                    "/checks",
                    serde_json::json!([{"kind":"reproducing","node_ids":[],"goal_ids":[]}]),
                ),
            ] {
                let mut case = leaf.clone();
                *case.pointer_mut(path).unwrap() = value;
                cases.push(case);
            }
            for case in cases {
                let expected = legacy(&case).map_err(|e| e.to_string());
                let actual = map_reply("planner", &case)
                    .map(|mapped| match mapped {
                        MappedReply::Kernel(reply) => reply,
                        _ => panic!("planner kernel reply"),
                    })
                    .map_err(|e| e.to_string());
                assert_eq!(actual, expected, "{case}");
            }
        }

        #[test]
        fn planner_reply_mapping_preserves_nested_schema_acceptance() {
            let leaf = json!({"kind":"leaf","files":[],"checks":[{
                "kind":"demonstrating","node_ids":["tests/test_a.py::test_a"],"goal_ids":[0],
                "forcing":{"symptom":"deadline","environment":{},"patches":[]},
                "proof":{"subjects":[],"observer_support":[],"red_message":"deadline"},
                "declared_symptom":{"node":"tests/test_a.py::test_a","exception":"builtins.TimeoutError",
                    "phase":"call","repeat":1,"parallel":0}}],
                "no_design_questions":true,"lands_on_its_own":true,
                "forecast":{"tokenizer":"o200k_base","brief_tokens":0,"file_tokens":{},
                    "overhead_tokens":0,"tokens":0,"fit_tokens":100,"context_tokens":150,"fits":true},
                "violations":[]});
            assert!(matches!(legacy(&leaf).unwrap(), Reply::PlanLeaf { .. }));
            let mut cases = vec![leaf.clone()];
            for version in [json!(1), json!(1.0)] {
                let mut case = leaf.clone();
                case["decision_context"] = json!({"schema_version":version,"request_version":0.0,
                    "decisions":[{"id":"approval","source_comment":null,"question":{},
                        "answer":"Approved.","allowed_scope":[],"restrictions":[],
                        "origin_request_version":0.0,"superseded_by":null}]});
                cases.push(case);
            }
            for provenance in [
                json!({"kind":"method"}),
                json!({"kind":"source",
                "source":"issue","quote":"Run the declared check."}),
            ] {
                let mut case = leaf.clone();
                case["evidence_contract"] = json!([{"tool":"run_tests","executor":"prismabuild",
                    "runner_features":[],"artifacts":[],"methods":[],"protected_files":[],
                    "requirement":"Run the declared check.","provenance":provenance}]);
                cases.push(case);
            }
            for exception in ["module.Erreuré", "module.A\u{301}", "module.Error\n"] {
                let mut case = leaf.clone();
                case["checks"][0]["declared_symptom"]["exception"] = exception.into();
                cases.push(case);
            }
            for name in ["DEADLINE", "DEADLINE\n"] {
                let mut case = leaf.clone();
                case["checks"][0]["forcing"]["environment"] = json!({name:"1"});
                cases.push(case);
            }
            for target in ["module.deadline", "module.deadline\n"] {
                let mut case = leaf.clone();
                case["checks"][0]["forcing"]["patches"] = json!([{"target":target,"returns":null}]);
                cases.push(case);
            }
            for case in cases {
                let expected = legacy(&case).map_err(|e| e.to_string());
                let actual = map_reply("planner", &case)
                    .map(|mapped| match mapped {
                        MappedReply::Kernel(reply) => reply,
                        _ => panic!("planner kernel reply"),
                    })
                    .map_err(|e| e.to_string());
                assert_eq!(actual, expected, "{case}");
            }
        }

        proptest::proptest! {
            #![proptest_config(proptest::test_runner::Config { cases: 64, ..Default::default() })]
            #[test]
            fn planner_reply_mapping_parity_on_generated_replies(
                kind in 0usize..3, tokens in 0u16..300, violates in proptest::bool::ANY,
                integral_float in proptest::bool::ANY
            ) {
                let files=serde_json::json!(["a.py"]);
                let mut value=match kind {
                    0=>serde_json::json!({"kind":"leaf","files":files,"checks":[],"no_design_questions":true,"lands_on_its_own":true}),
                    1=>serde_json::json!({"kind":"split","reason":"does_not_fit","pieces":[{"brief":"First", "goal_ids":[0], "files":files}]}),
                    _=>serde_json::json!({"kind":"cant_meet","report":"Cannot fit.","files":files}),
                };
                value["forecast"]=serde_json::json!({"tokenizer":"o200k_base", "brief_tokens":tokens,
                    "file_tokens":{"a.py":0},"overhead_tokens":0,"tokens":tokens,
                    "fit_tokens":100,"context_tokens":150,"fits":tokens<100});
                value["violations"]=if violates {serde_json::json!(["rule break"])} else {serde_json::json!([])};
                if integral_float {value["estimated_tool_turns"]=serde_json::json!(f64::from(tokens));}
                let expected=legacy(&value).map_err(|e|e.to_string());
                let actual=map_reply("planner",&value).map(|reply|match reply {MappedReply::Kernel(reply)=>reply,_=>panic!("planner kernel reply")}).map_err(|e|e.to_string());
                proptest::prop_assert_eq!(actual,expected);
            }
        }
    }
}
