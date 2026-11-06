use super::super::super::*;
use std::sync::OnceLock;
use tl3_kernel::EarlyEnd;
pub(crate) enum MappedReply { Kernel(Reply), Made(String), NeedsFiles {paths:Vec<String>, fits:bool} }
include!(concat!(env!("OUT_DIR"), "/worker_schemas.rs"));
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
pub(crate) fn relative_file(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains(['\\', '\0'])
        && path
            .split('/')
            .all(|part| !matches!(part, "" | "." | ".." | ".git"))
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
