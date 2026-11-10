//! D8's current campaign format. Hash every input before interpreting it.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
};
use tl3_contracts::run_binding::Ending;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub sha256: String,
    pub bytes: u64,
    pub mode: u32,
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
    !name.is_empty()
        && !Path::new(name).is_absolute()
        && name.split('/').all(|part| !matches!(part, "" | "." | ".."))
}

fn checked(root: &Path, name: &str, input: &Input) -> Result<Vec<u8>, String> {
    if !relative(name) {
        return Err("qualification.input.path".into());
    }
    let path = root.join(name);
    if path
        .ancestors()
        .take_while(|p| *p != root)
        .any(Path::is_symlink)
    {
        return Err("qualification.input.symlink".into());
    }
    let raw = fs::read(&path).map_err(|e| e.to_string())?;
    if raw.len() as u64 != input.bytes
        || sha256(&raw) != input.sha256
        || fs::metadata(path)
            .map_err(|e| e.to_string())?
            .permissions()
            .mode()
            & 0o777
            != input.mode
    {
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
            result.insert(
                path.strip_prefix(root)
                    .map_err(|e| e.to_string())?
                    .to_str()
                    .ok_or("qualification.input.utf8")?
                    .into(),
            );
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
        || index.source.len() != 40
        || !index
            .source
            .bytes()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        || index
            .runs
            .iter()
            .map(|r| r.kind.as_str())
            .collect::<Vec<_>>()
            != ["launch_failure", "cancel", "success"]
    {
        return Err("qualification.inputs.schema".into());
    }
    let root = path
        .parent()
        .ok_or("qualification.inputs.root")?
        .join("fx-current");
    let mut observed = BTreeSet::new();
    inventory(&root, &root, &mut observed)?;
    let expected: BTreeSet<_> = index
        .inputs
        .keys()
        .chain(index.fixtures.keys())
        .cloned()
        .collect();
    if expected.len() != index.inputs.len() + index.fixtures.len() || observed != expected {
        return Err("qualification.inventory".into());
    }
    let mut values = BTreeMap::new();
    for (name, input) in &index.inputs {
        let raw = checked(&root, name, input)?;
        values.insert(
            name.clone(),
            serde_json::from_slice::<Value>(&raw).map_err(|e| e.to_string())?,
        );
    }
    for (name, input) in &index.fixtures {
        checked(&root, name, input)?;
    }
    let fault_name = "negative-interpreter-provenance.json";
    let mut used: BTreeSet<_> = index
        .runs
        .iter()
        .flat_map(|run| [run.manifest.clone(), run.acceptance.clone()])
        .collect();
    used.insert(fault_name.into());
    if used != index.inputs.keys().cloned().collect() {
        return Err("qualification.input_inventory".into());
    }
    let fault = values
        .get(fault_name)
        .ok_or("qualification.negative_fixture")?;
    if fault["source"] != index.source
        || fault["observed_interpreter_claim"] != false
        || fault["fault_input"]["value"] != "/opt/tl3/bin/python-canary2-absent"
    {
        return Err("qualification.negative_fixture".into());
    }
    let mut manifests = Vec::new();
    let mut provenance = Vec::new();
    for run in &index.runs {
        let manifest = values.get(&run.manifest).ok_or("qualification.manifest")?;
        let acceptance = values
            .get(&run.acceptance)
            .ok_or("qualification.acceptance")?;
        let expected_ending = match run.kind.as_str() {
            "launch_failure" => Ending::InfraLost,
            "cancel" => Ending::Cancelled,
            "success" => Ending::Passed,
            _ => return Err("qualification.kind".into()),
        };
        if manifest["producer_commit"] != index.source
            || acceptance["schema"] != "tl3.fx_canary_acceptance.v1"
            || acceptance["source"] != index.source
            || acceptance["kind"] != run.kind
            || acceptance["status"] != "PASS"
            || serde_json::from_value::<Ending>(acceptance["ending"].clone()).ok()
                != Some(expected_ending)
            || acceptance["cleanup_verified"] != true
            || acceptance["allocation_verified"] != true
            || acceptance["job_id"].as_str().is_none_or(str::is_empty)
            || acceptance["receipt_sha256"]
                .as_str()
                .is_none_or(|s| s.len() != 64)
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
    let files = positive["files"]
        .as_array()
        .ok_or("qualification.inventory")?;
    let names: Option<BTreeSet<_>> = files.iter().map(|f| f["path"].as_str()).collect();
    if files.is_empty()
        || names
            .as_ref()
            .is_none_or(|names| names.len() != files.len())
    {
        return Err("qualification.inventory".into());
    }
    let mut metadata = positive.clone();
    let fields = metadata.as_object_mut().ok_or("qualification.metadata")?;
    fields.remove("producer_commit");
    fields.remove("files");
    let profile_files = ["image-inspect.json", "image-map.json", "preflight.json"];
    let fixed: BTreeMap<_, _> = files
        .iter()
        .filter(|f| !profile_files.contains(&f["path"].as_str().unwrap_or("")))
        .map(|f| (format!("fixtures/{}", f["path"].as_str().unwrap_or("")), f))
        .collect();
    if fixed.len() != index.fixtures.len()
        || fixed.iter().any(|(name, file)| {
            index.fixtures.get(name).is_none_or(|input| {
                file["sha256"] != input.sha256
                    || file["bytes"] != input.bytes
                    || file["mode"]
                        != if input.mode & 0o111 != 0 {
                            0o500
                        } else {
                            0o400
                        }
            })
        })
    {
        return Err("qualification.fixture_inventory".into());
    }
    let mut raw = serde_json::to_vec_pretty(&json!({"schema":"tl3.fx_qualification.v1",
        "acceptance_source":index.source,"inputs_sha256":sha256(&fs::read(path).map_err(|e| e.to_string())?),
        "historical_runs":provenance,"metadata":metadata,"profile_files":profile_files,
        "files":files.iter().filter(|f| !profile_files.contains(&f["path"].as_str().unwrap_or(""))).collect::<Vec<_>>()}))
        .map_err(|e| e.to_string())?;
    raw.push(b'\n');
    Ok(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, Index) {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("fx-current");
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join("fixtures")).unwrap();
        let source = "e".repeat(40);
        let helper = b"trusted helper\n";
        let fixture = Input {
            sha256: sha256(helper),
            bytes: helper.len() as u64,
            mode: 0o644,
        };
        fs::write(root.join("fixtures/helper.py"), helper).unwrap();
        let mut index = Index {
            schema: "tl3.fx_qualification_inputs.v1".into(),
            source: source.clone(),
            inputs: BTreeMap::new(),
            runs: vec![],
            fixtures: BTreeMap::from([("fixtures/helper.py".into(), fixture)]),
        };
        let raw = serde_json::to_vec(&json!({"source":source,"observed_interpreter_claim":false,
            "fault_input":{"value":"/opt/tl3/bin/python-canary2-absent"}}))
        .unwrap();
        let name = "negative-interpreter-provenance.json";
        fs::write(root.join(name), &raw).unwrap();
        fs::set_permissions(root.join(name), fs::Permissions::from_mode(0o644)).unwrap();
        index.inputs.insert(
            name.into(),
            Input {
                sha256: sha256(&raw),
                bytes: raw.len() as u64,
                mode: 0o644,
            },
        );
        for (kind, ending, interpreter) in [
            (
                "launch_failure",
                Ending::InfraLost,
                "/opt/tl3/bin/python-canary2-absent",
            ),
            ("cancel", Ending::Cancelled, "/opt/tl3/bin/python"),
            ("success", Ending::Passed, "/opt/tl3/bin/python"),
        ] {
            let manifest_name = format!("{kind}-manifest.json");
            let acceptance_name = format!("{kind}-acceptance.json");
            let manifest = json!({"producer_commit":source,"container_interpreter":interpreter,"files":[
                {"path":"helper.py","sha256":sha256(helper),"bytes":helper.len(),"mode":0o400}]});
            let acceptance = json!({"schema":"tl3.fx_canary_acceptance.v1","source":source,"kind":kind,"ending":ending,
                "status":"PASS","cleanup_verified":true,"allocation_verified":true,"job_id":"f123","receipt_sha256":"a".repeat(64)});
            for (name, value) in [(&manifest_name, manifest), (&acceptance_name, acceptance)] {
                let raw = serde_json::to_vec(&value).unwrap();
                fs::write(root.join(name), &raw).unwrap();
                fs::set_permissions(root.join(name), fs::Permissions::from_mode(0o644)).unwrap();
                index.inputs.insert(
                    name.clone(),
                    Input {
                        sha256: sha256(&raw),
                        bytes: raw.len() as u64,
                        mode: 0o644,
                    },
                );
            }
            index.runs.push(Run {
                kind: kind.into(),
                manifest: manifest_name,
                acceptance: acceptance_name,
            });
        }
        fs::set_permissions(
            root.join("fixtures/helper.py"),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        fs::write(
            directory.path().join("current.json"),
            serde_json::to_vec(&index).unwrap(),
        )
        .unwrap();
        (directory, index)
    }

    #[test]
    fn current_index_is_deterministic_and_closes_fixture_inventory() {
        let (directory, _) = fixture();
        let path = directory.path().join("current.json");
        let first = generate(&path).unwrap();
        assert_eq!(first, generate(&path).unwrap());
        fs::write(
            directory.path().join("fx-current/fixtures/unlisted.py"),
            b"unexpected",
        )
        .unwrap();
        assert_eq!(generate(&path).unwrap_err(), "qualification.inventory");
    }

    #[test]
    fn current_hash_check_precedes_parsing_and_requires_all_passes() {
        let (directory, mut index) = fixture();
        let path = directory.path().join("current.json");
        let name = "success-acceptance.json";
        let file = directory.path().join("fx-current").join(name);
        fs::write(&file, b"{").unwrap();
        assert_eq!(generate(&path).unwrap_err(), "qualification.input.sha256");
        let input = index.inputs.get_mut(name).unwrap();
        input.sha256 = sha256(b"{");
        input.bytes = 1;
        fs::write(&path, serde_json::to_vec(&index).unwrap()).unwrap();
        assert!(generate(&path).unwrap_err().contains("EOF"));
        let (directory, mut index) = fixture();
        let path = directory.path().join("current.json");
        let file = directory.path().join("fx-current").join(name);
        let mut acceptance: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        acceptance["status"] = "FAIL".into();
        let raw = serde_json::to_vec(&acceptance).unwrap();
        fs::write(&file, &raw).unwrap();
        index.inputs.get_mut(name).unwrap().sha256 = sha256(&raw);
        index.inputs.get_mut(name).unwrap().bytes = raw.len() as u64;
        fs::write(&path, serde_json::to_vec(&index).unwrap()).unwrap();
        assert_eq!(
            generate(&path).unwrap_err(),
            "qualification.accepted_provenance"
        );
    }

    #[test]
    fn ending_must_be_the_producers_typed_canonical_value() {
        for ending in [json!(Ending::Passed), json!("infra_lost"), Value::Null, json!("unknown")] {
            let (directory, mut index) = fixture();
            let path = directory.path().join("current.json");
            let name = "launch_failure-acceptance.json";
            let file = directory.path().join("fx-current").join(name);
            let mut acceptance: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
            acceptance["ending"] = ending;
            let raw = serde_json::to_vec(&acceptance).unwrap();
            fs::write(&file, &raw).unwrap();
            index.inputs.get_mut(name).unwrap().sha256 = sha256(&raw);
            index.inputs.get_mut(name).unwrap().bytes = raw.len() as u64;
            fs::write(&path, serde_json::to_vec(&index).unwrap()).unwrap();
            assert_eq!(generate(&path).unwrap_err(), "qualification.accepted_provenance");
        }
    }

    #[test]
    fn missing_required_fixture_and_reordered_lifecycle_are_refused() {
        let (directory, mut index) = fixture();
        let path = directory.path().join("current.json");
        index.fixtures.clear();
        fs::remove_file(directory.path().join("fx-current/fixtures/helper.py")).unwrap();
        fs::write(&path, serde_json::to_vec(&index).unwrap()).unwrap();
        assert_eq!(
            generate(&path).unwrap_err(),
            "qualification.fixture_inventory"
        );
        let (directory, mut index) = fixture();
        let path = directory.path().join("current.json");
        index.runs.swap(0, 1);
        fs::write(&path, serde_json::to_vec(&index).unwrap()).unwrap();
        assert_eq!(generate(&path).unwrap_err(), "qualification.inputs.schema");
    }
}
