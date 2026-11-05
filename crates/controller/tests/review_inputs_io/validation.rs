use super::*;
use std::sync::OnceLock;
include!(concat!(env!("OUT_DIR"), "/worker_schemas.rs"));
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

pub fn validate_real(role: &str, inputs: &Value) -> Result<()> {
    match validator(role, "input")?.validate(inputs) {
        Ok(()) => Ok(()),
        Err(error) => Err(crate::worker_inputs::Failure::capture(role, &error).into()),
    }
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

