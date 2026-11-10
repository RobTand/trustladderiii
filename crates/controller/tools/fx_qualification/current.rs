//! D8's current campaign format. Hash every input before interpreting it.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::{BTreeMap, BTreeSet}, fs, path::{Path, PathBuf}};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Index {
    pub schema: String,
    pub source: String,
    pub inputs: BTreeMap<String, Input>,
    pub runs: Vec<Run>,
    pub fixtures: BTreeMap<String, Input>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Run {
    pub kind: String,
    pub manifest: String,
    pub acceptance: String,
}

pub fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn relative(name: &str) -> bool {
    !name.is_empty() && !Path::new(name).is_absolute()
        && name.split('/').all(|part| !matches!(part, "" | "." | ".."))
}

fn checked(root: &Path, name: &str, input: &Input) -> Result<Vec<u8>, String> {
    if !relative(name) {
        return Err("qualification.input.path".into());
    }
    let path = root.join(name);
    if path.ancestors().take_while(|p| *p != root).any(Path::is_symlink) {
        return Err("qualification.input.symlink".into());
    }
    let raw = fs::read(path).map_err(|e| e.to_string())?;
    if raw.len() as u64 != input.bytes || sha256(&raw) != input.sha256 {
        return Err("qualification.input.sha256".into());
    }
    Ok(raw)
}

fn inventory(root: &Path, at: &Path, result: &mut BTreeSet<String>) -> Result<(), String> {
    for entry in fs::read_dir(at).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_dir() {
            inventory(root, &entry.path(), result)?;
        } else if kind.is_file() {
            let path = entry.path();
            result.insert(path.strip_prefix(root).map_err(|e| e.to_string())?
                .to_str().ok_or("qualification.input.utf8")?.into());
        } else {
            return Err("qualification.input.nonregular".into());
        }
    }
    Ok(())
}

pub fn generate(path: &Path) -> Result<Vec<u8>, String> {
    let index: Index = serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    if index.schema != "tl3.fx_qualification_inputs.v1"
        || index.source.len() != 40 || !index.source.bytes().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        || index.runs.iter().map(|r| r.kind.as_str()).collect::<Vec<_>>() != ["launch_failure", "cancel", "success"]
    {
        return Err("qualification.inputs.schema".into());
    }
    let root = path.parent().ok_or("qualification.inputs.root")?.join("fx-current");
    let mut observed = BTreeSet::new();
    inventory(&root, &root, &mut observed)?;
    let expected: BTreeSet<_> = index.inputs.keys().chain(index.fixtures.keys()).cloned().collect();
    if expected.len() != index.inputs.len() + index.fixtures.len() || observed != expected {
        return Err("qualification.inventory".into());
    }
    let mut values = BTreeMap::new();
    for (name, input) in &index.inputs {
        let raw = checked(&root, name, input)?;
        values.insert(name.clone(), serde_json::from_slice::<Value>(&raw).map_err(|e| e.to_string())?);
    }
    for (name, input) in &index.fixtures {
        checked(&root, name, input)?;
    }
    let mut manifests = Vec::new();
    let mut provenance = Vec::new();
    for run in &index.runs {
        let manifest = values.get(&run.manifest).ok_or("qualification.manifest")?;
        let acceptance = values.get(&run.acceptance).ok_or("qualification.acceptance")?;
        let expected_ending = match run.kind.as_str() {
            "launch_failure" => "infra_lost",
            "cancel" => "cancelled",
            "success" => "passed",
            _ => return Err("qualification.kind".into()),
        };
        if manifest["producer_commit"] != index.source
            || acceptance["schema"] != "tl3.fx_canary_acceptance.v1"
            || acceptance["source"] != index.source || acceptance["kind"] != run.kind
            || acceptance["status"] != "PASS" || acceptance["ending"] != expected_ending
            || acceptance["cleanup_verified"] != true || acceptance["allocation_verified"] != true
            || acceptance["job_id"].as_str().is_none_or(str::is_empty)
            || acceptance["receipt_sha256"].as_str().is_none_or(|s| s.len() != 64)
        {
            return Err("qualification.accepted_provenance".into());
        }
        provenance.push(json!({"run":run.kind, "manifest_sha256":index.inputs[&run.manifest].sha256,
            "acceptance_sha256":index.inputs[&run.acceptance].sha256,"producer_commit":index.source}));
        manifests.push(manifest.clone());
    }
    let positive = &manifests[2];
    for (run, manifest) in index.runs.iter().zip(&manifests) {
        let mut expected = positive.clone();
        if run.kind == "launch_failure" {
            expected["container_interpreter"] = "/opt/tl3/bin/python-canary2-absent".into();
        }
        if *manifest != expected {
            return Err("qualification.execution_metadata".into());
        }
    }
    let files = positive["files"].as_array().ok_or("qualification.inventory")?;
    let names: Option<BTreeSet<_>> = files.iter().map(|f| f["path"].as_str()).collect();
    if files.is_empty() || names.as_ref().is_none_or(|names| names.len() != files.len()) {
        return Err("qualification.inventory".into());
    }
    let mut metadata = positive.clone();
    let fields = metadata.as_object_mut().ok_or("qualification.metadata")?;
    fields.remove("producer_commit");
    fields.remove("files");
    let profile_files = ["image-inspect.json", "image-map.json", "preflight.json"];
    let mut raw = serde_json::to_vec_pretty(&json!({"schema":"tl3.fx_qualification.v1",
        "acceptance_source":index.source,"inputs_sha256":sha256(&fs::read(path).map_err(|e| e.to_string())?),
        "historical_runs":provenance,"metadata":metadata,"profile_files":profile_files,
        "files":files.iter().filter(|f| !profile_files.contains(&f["path"].as_str().unwrap_or(""))).collect::<Vec<_>>()}))
        .map_err(|e| e.to_string())?;
    raw.push(b'\n');
    Ok(raw)
}

pub fn installed_paths(index: &Index) -> Vec<PathBuf> {
    index.inputs.keys().chain(index.fixtures.keys()).map(|p| Path::new("qualification/fx-current").join(p)).collect()
}
