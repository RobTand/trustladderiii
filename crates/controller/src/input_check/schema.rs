use serde_json::Value;
use std::{collections::BTreeMap, sync::OnceLock};

include!(concat!(env!("OUT_DIR"), "/worker_schemas.rs"));

pub(super) fn valid(name: &str, value: &Value) -> crate::Result<bool> {
    static SCHEMAS: OnceLock<Result<BTreeMap<String, jsonschema::Validator>, String>> =
        OnceLock::new();
    let schemas = SCHEMAS
        .get_or_init(|| {
            let parsed = PACKAGED_SCHEMAS
                .iter()
                .map(|(name, text)| {
                    serde_json::from_str::<Value>(text)
                        .map(|schema| ((*name).to_owned(), schema))
                        .map_err(|_| "invalid packaged input schema".to_owned())
                })
                .collect::<Result<Vec<_>, _>>()?;
            let mut registry = jsonschema::Registry::new();
            for (name, schema) in &parsed {
                registry = registry
                    .add(
                        format!("https://trustladderiii.invalid/workers/protocol-1/{name}"),
                        schema.clone(),
                    )
                    .map_err(|_| "invalid packaged input registry".to_owned())?;
            }
            let registry = registry
                .prepare()
                .map_err(|_| "invalid packaged input registry".to_owned())?;
            parsed
                .into_iter()
                .map(|(name, schema)| {
                    jsonschema::options()
                        .with_registry(&registry)
                        .build(&schema)
                        .map(|validator| (name, validator))
                        .map_err(|_| "invalid packaged input validator".to_owned())
                })
                .collect()
        })
        .as_ref()
        .map_err(|_| crate::Error::Invalid("input schema initialization failed"))?;
    schemas
        .get(&format!("{name}.json"))
        .map(|schema| schema.is_valid(value))
        .ok_or(crate::Error::Invalid("unsupported input schema"))
}
