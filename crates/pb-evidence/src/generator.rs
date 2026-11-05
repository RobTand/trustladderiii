use crate::error::{Refusal, Result, get};
use crate::python_json::{dumps, equal, sha};
use base64::{Engine, engine::general_purpose::STANDARD};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct GeneratorContract {
    pub command: String,
    pub argv: Vec<String>,
    pub generated_files: Vec<String>,
    pub protected_paths: Vec<String>,
    pub image: String,
    pub implementation: String,
    pub budget: Value,
}
pub fn path(name: &str) -> Result<()> {
    let parts: Vec<_> = name.split('/').collect();
    if name.is_empty()
        || name.starts_with('/')
        || name.ends_with('/')
        || name.contains("//")
        || name != "." && parts.contains(&".")
        || parts.contains(&"..")
        || parts.contains(&".git")
        || name.chars().any(|c| c < ' ' || c == '\\')
        || name.len() > 3072
        || parts.iter().any(|p| p.len() > 255)
    {
        return Err(Refusal::value(
            "generator path must be a canonical relative file path",
        ));
    }
    if parts.last() == Some(&".tl3-generator-import") {
        return Err(Refusal::value("reserved generator import path"));
    }
    Ok(())
}
// PurePosixPath.full_match treats ** as a complete path component and matches
// zero directory components as well as arbitrarily deep ones.
fn component(pattern: &str) -> String {
    let chars: Vec<_> = pattern.chars().collect();
    let mut i = 0;
    let mut out = String::new();
    while i < chars.len() {
        match chars[i] {
            '*' => out.push_str("[^/]*"),
            '?' => out.push_str("[^/]"),
            '[' => {
                let start = i;
                let mut end = i + 1;
                if end < chars.len() && chars[end] == '!' {
                    end += 1;
                }
                if end < chars.len() && chars[end] == ']' {
                    end += 1;
                }
                while end < chars.len() && chars[end] != ']' {
                    end += 1;
                }
                if end == chars.len() {
                    out.push_str("\\[");
                } else {
                    let content: String = chars[start + 1..end].iter().collect();
                    out.push('[');
                    if let Some(rest) = content.strip_prefix('!') {
                        out.push('^');
                        out.push_str(rest);
                    } else {
                        if content.starts_with('^') {
                            out.push('\\');
                        }
                        out.push_str(&content);
                    }
                    out.push(']');
                    i = end;
                }
            }
            ch => out.push_str(&regex::escape(&ch.to_string())),
        }
        i += 1;
    }
    out
}
pub fn full_match(name: &str, pattern: &str) -> bool {
    if pattern.starts_with('/') {
        return false;
    }
    let mut pattern = pattern
        .split('/')
        .filter(|part| *part != ".")
        .collect::<Vec<_>>()
        .join("/");
    while pattern.contains("//") {
        pattern = pattern.replace("//", "/");
    }
    let parts: Vec<_> = pattern.split('/').collect();
    let mut expression = String::from("\\A");
    for (i, part) in parts.iter().enumerate() {
        if *part == "**" {
            if i + 1 == parts.len() {
                expression.push_str(".*");
            } else {
                expression.push_str("(?:[^/]+/)*");
            }
        } else {
            expression.push_str(&component(part));
            if i + 1 < parts.len() {
                expression.push('/');
            }
        }
    }
    expression.push_str("\\z");
    regex::Regex::new(&expression).is_ok_and(|pattern| pattern.is_match(name))
}
fn digest(value: &Value) -> String {
    sha(dumps(value, true, true).as_bytes())
}

pub fn identity(
    artifact: &Value,
    command: &str,
    image: &str,
    implementation: &str,
    budget: &Value,
) -> Result<()> {
    if artifact["command"] != command
        || !equal(
            &artifact["identity"],
            &json!({"implementation":implementation,"image":image,"budget":budget}),
        )
        || !equal(&artifact["budget"], budget)
    {
        Err(Refusal::value(
            "Generator artifact identity differs from its configured command/image/budget.",
        ))
    } else {
        Ok(())
    }
}

pub fn validate(
    artifact: &Value,
    before: &Value,
    contract: &GeneratorContract,
    check_identity: bool,
) -> Result<Value> {
    if check_identity
        && (artifact["command"] != contract.command
            || !equal(
                &artifact["identity"],
                &json!({"implementation":contract.implementation,"image":contract.image,"budget":contract.budget}),
            )
            || !equal(&artifact["budget"], &contract.budget))
    {
        return Err(Refusal::value(
            "Generator artifact identity differs from its configured command/image/budget.",
        ));
    }
    if artifact["status"] != "completed" || !equal(&artifact["exit_code"], &json!(0)) {
        return Err(Refusal::value(artifact["message"].as_str().unwrap_or("")));
    }
    if artifact["input_manifest"] != digest(before) {
        return Err(Refusal::value("generator artifact has a stale input tree"));
    }
    if artifact["command"] != contract.command || !equal(&artifact["argv"], &json!(contract.argv)) {
        return Err(Refusal::value(
            "generator artifact argv differs from its configured command",
        ));
    }
    let outputs = get(artifact, "outputs")?
        .as_array()
        .filter(|outputs| outputs.len() <= 128)
        .ok_or_else(|| Refusal::value("generator output file count exceeds limit"))?;
    let mut after = before
        .as_object()
        .cloned()
        .ok_or_else(|| Refusal::value("manifest must be an object"))?;
    let mut seen = BTreeSet::new();
    let mut total = 0usize;
    for output in outputs {
        let name = get(output, "path")?.as_str().ok_or_else(|| {
            Refusal::value("generator path must be a canonical relative file path")
        })?;
        path(name)?;
        if !seen.insert(name.to_owned()) {
            return Err(Refusal::value("duplicate generator output"));
        }
        let raw = STANDARD
            .decode(get(output, "data")?.as_str().unwrap_or(""))
            .map_err(|error| Refusal::value(error.to_string()))?;
        total += raw.len();
        if total > 32 * 1_048_576 {
            return Err(Refusal::value("generator output exceeds total size limit"));
        }
        if raw.len() > 16 * 1_048_576 {
            return Err(Refusal::value(
                "generator output exceeds the file size limit",
            ));
        }
        std::str::from_utf8(&raw).map_err(|e| Refusal::utf8(&raw, e))?;
        if raw.iter().any(|b| *b < 32 && !matches!(b, 9 | 10 | 13)) {
            return Err(Refusal::value("generator output must be text"));
        }
        let entry = get(output, "entry")?;
        if entry["sha256"] != sha(&raw)
            || !equal(&entry["bytes"], &json!(raw.len()))
            || ![json!(420), json!(493)]
                .iter()
                .any(|mode| equal(mode, &entry["mode"]))
        {
            return Err(Refusal::value("generator output digest or mode mismatch"));
        }
        after.insert(name.to_owned(), entry.clone());
    }
    let mut changed = BTreeSet::new();
    for name in after.keys().chain(
        before
            .as_object()
            .into_iter()
            .flat_map(|before| before.keys()),
    ) {
        let a = after.get(name);
        let b = before.get(name);
        if match (a, b) {
            (Some(a), Some(b)) => !equal(a, b),
            _ => true,
        } {
            changed.insert(name.to_owned());
        }
    }
    if changed.len() > 128 {
        return Err(Refusal::value("generator output file count exceeds limit"));
    }
    for name in &changed {
        path(name)?;
        if contract
            .protected_paths
            .iter()
            .any(|pattern| full_match(name, pattern))
        {
            return Err(Refusal::value(format!(
                "generator changed protected output: {name}"
            )));
        }
        if !contract
            .generated_files
            .iter()
            .any(|pattern| full_match(name, pattern))
        {
            return Err(Refusal::value(format!(
                "generator changed undeclared generated output: {name}"
            )));
        }
        let entry = after
            .get(name)
            .ok_or_else(|| Refusal::value(format!("generator deleted an output: {name}")))?;
        let expected = before
            .get(name)
            .and_then(|entry| entry.get("mode"))
            .cloned()
            .unwrap_or(json!(420));
        if !equal(&entry["mode"], &expected) {
            return Err(Refusal::value(format!(
                "generator changed an output mode: {name}; new outputs require mode 0644"
            )));
        }
    }
    if changed != seen {
        return Err(Refusal::value(
            "generator artifact contains an unchanged output",
        ));
    }
    let after = Value::Object(after);
    if artifact["output_manifest"] != digest(&after) {
        return Err(Refusal::value("generator output manifest digest mismatch"));
    }
    Ok(after)
}
