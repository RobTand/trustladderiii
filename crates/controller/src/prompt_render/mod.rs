//! Selected-worker-release prompt rendering (#819).
//!
//! The worker calls this core at its original sampling points. The launch pin,
//! not the controller's executable, selects the immutable resources it uses.
mod decision;
mod json;
mod resources;

use crate::{Error, Result};
use json::{Value, text};
use sha2::{Digest, Sha256};

include!(concat!(env!("OUT_DIR"), "/worker_schemas.rs"));

fn parse_role(role: &str) -> Result<decision::Role> {
    serde_json::from_str(&Value::from(role).wire())
        .map_err(|_| Error::Invalid("unknown prompt role"))
}

fn shared_role(role: &str) -> Result<bool> {
    Ok(decision::shared(parse_role(role)?))
}

fn role(document: &Value) -> Result<Vec<u32>> {
    let role = document.get("role").string()?;
    let builtin = crate::prompts::BUILTIN
        .iter()
        .find(|(name, _)| *name == role)
        .ok_or(Error::Invalid("unknown prompt role"))?
        .1;
    let mut prompt = match document.get("text") {
        Value::Null => text(builtin),
        Value::String(value) => value.clone(),
        _ => return Err(Error::Invalid("prompt renderer expected text")),
    };
    if shared_role(&role)? {
        let cpus = match document.find("test_cpus") {
            None => "1".into(),
            Some(Value::Number(cpus)) if cpus.parse::<u64>().is_ok() => cpus.clone(),
            _ => return Err(Error::Invalid("invalid prompt test_cpus")),
        };
        prompt.extend(text("\n\n"));
        prompt.extend(text(&resources::forcing(&cpus)));
    }
    if role == "planner" {
        prompt.extend(text("\n\n"));
        prompt.extend(text(resources::PLANNER));
    }
    Ok(prompt)
}

fn shared(inputs: &Value) -> Value {
    Value::object(
        ["repo", "base_sha"]
            .into_iter()
            .filter_map(|key| inputs.find(key).map(|value| (key, value.clone()))),
    )
}

fn cache(request: &Value) -> Result<Value> {
    let name = request.get("role").string()?;
    if !shared_role(&name)? {
        return Err(Error::Invalid("session identity stays in the worker"));
    }
    let prompt = request.get("prompt");
    let system = role(&Value::object([
        ("role", Value::from(name.as_str())),
        ("text", prompt.get("text").clone()),
        (
            "test_cpus",
            request
                .find("test_cpus")
                .cloned()
                .unwrap_or(Value::Number("1".into())),
        ),
    ]))?;
    let filename = format!("{name}-reply.json");
    let schema = PACKAGED_SCHEMAS
        .iter()
        .find(|(name, _)| *name == filename)
        .ok_or(Error::Invalid("unknown prompt schema"))?
        .1;
    let repository = request
        .get("trace")
        .find("repo")
        .unwrap_or_else(|| request.get("memory").get("repo"));
    let document = Value::object([
        ("role", Value::from(name.as_str())),
        ("version", prompt.get("version").clone()),
        ("prompt", Value::String(system)),
        ("schema", json::parse(schema)?),
        (
            "planner_schema",
            if name == "planner" {
                json::parse(resources::EXPLORATION)?
            } else {
                Value::Null
            },
        ),
        ("format", Value::from(resources::ONLY_JSON)),
        ("repository", repository.clone()),
        ("orientation", shared(request.get("inputs"))),
    ])
    .dump();
    let Ok(encoded) = json::utf8(&document) else {
        // Python must raise its original UnicodeEncodeError at this call point.
        return Ok(Value::object([("encoding_error", Value::String(document))]));
    };
    let digest = format!("{:x}", Sha256::digest(encoded.as_bytes()));
    Ok(Value::object([(
        "identity",
        Value::from(format!("tl3-cache-{}", &digest[..48]).as_str()),
    )]))
}

fn prefix(request: &Value) -> Result<Value> {
    let role = request.get("role").string()?;
    let shared = if shared_role(&role)? {
        shared(request.get("inputs"))
    } else {
        Value::object([])
    };
    let Value::Object(keys) = &shared else {
        unreachable!()
    };
    let claude = request
        .get("model")
        .get("name")
        .string()
        .unwrap_or_default()
        .starts_with("claude");
    let policy = decision::prefix(parse_role(&role)?, !keys.is_empty(), claude);
    if policy == decision::Prefix::None {
        return Ok(Value::object([("prefix", Value::Null)]));
    }
    let mut prefix = shared.dump();
    prefix.pop();
    prefix.extend(text(", "));
    let mut block = vec![
        ("type", Value::from("text")),
        ("text", Value::String(prefix)),
    ];
    if policy == decision::Prefix::Ephemeral {
        block.push((
            "cache_control",
            Value::object([("type", Value::from("ephemeral"))]),
        ));
    }
    Ok(Value::object([("prefix", Value::object(block))]))
}

/// Pure transport entry, also used directly by controller parity tests.
pub fn render(kind: &str, document: &str) -> Result<String> {
    let document = json::parse(document)?;
    let output = match kind {
        "role" => Value::object([("text", Value::String(role(&document)?))]),
        "cache" => cache(&document)?,
        "prefix" => prefix(&document)?,
        "generation" => Value::object([("text", Value::from(resources::GENERATION))]),
        "serialize" => Value::object([("text", Value::String(document.dump()))]),
        _ => {
            return Err(Error::Invalid(
                "usage: tl3 render-prompt <role|cache|prefix|generation|serialize>",
            ));
        }
    };
    Ok(output.wire())
}

/// No config, credentials, clock or filesystem reads at the renderer boundary.
pub fn command(kind: Option<&str>) -> Result<()> {
    use std::io::{Read, Write};
    let mut document = String::new();
    std::io::stdin().read_to_string(&mut document)?;
    let output = render(kind.unwrap_or(""), &document)?;
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(output.as_bytes())?;
    stdout.write_all(b"\n")?;
    Ok(())
}

#[cfg(test)]
mod tests;
