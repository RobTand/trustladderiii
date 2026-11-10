//! Generate the FX pin from hash-checked accepted records, never a candidate bundle.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

#[path = "fx_qualification/current.rs"]
pub mod current;

/// Prefer the source-owned current input index once it has been published.
pub fn generate_checkout(root: &Path) -> Result<Vec<u8>, String> {
    let index = root.join("qualification/current.json");
    if index.try_exists().map_err(|error| error.to_string())? {
        current::generate(&index)
    } else {
        generate(&root.join("qualification/fx-exact42b"))
    }
}

const SOURCE: &str = "42b68e30fcb3d37cc03c0a70166af9e912b25dc6";
const RECONCILIATION: &str = "0623e1deacca0adddba501822e433be68e7c5f1abcaeaa3fcfe8d96d485c03be";
const FAULT: &str = "0640df231230053922fc64d3bab8f154bc40c66d4f08dafd7d4423a8253faeed";
const PROFILE_FILES: [&str; 3] = ["image-inspect.json", "image-map.json", "preflight.json"];

// Retained source identities, not publisher provenance for future profiles.
struct Run {
    name: &'static str,
    manifest: &'static str,
    acceptance: &'static str,
    audit: &'static str,
    producer: &'static str,
    interpreter: &'static str,
}

const RUNS: [Run; 3] = [
    Run {
        name: "canary2",
        manifest: "640fe3b8b535b1db142ad099e0bf115f6255f90923b3dd73fc681eaca7e93391",
        acceptance: "e32cfdccf2a284c5446a53fbaf9699aba2e9937c7bd0aa0c9d1999ae78a08cbf",
        audit: "f339f5bb17c9f48c77ed07a9a72348777f01397de260d55df35a18813aeca47a",
        producer: "10a06385ce91404961e6b0d2894d0e75a7b24f21",
        interpreter: "/opt/tl3/bin/python-canary2-absent",
    },
    Run {
        name: "canary3",
        manifest: "4fbf54b83553eaaafc0ea6376bd4e1c1382d3fd58e84a5c44e85c1d73dea61c3",
        acceptance: "765f75dcc517039a31bdc73ae8846f1b3902c41725d4edfa3c06849c54943179",
        audit: "f8577888c7db9cbc6b927adc501d899f5df5b746ff4775775bf044d43b175a09",
        producer: "2a332ed2f7a6afbc6619e82802d378b6c9fb867a",
        interpreter: "/opt/tl3/bin/python",
    },
    Run {
        name: "success",
        manifest: "294d58f0b5feb517d7b70426e0b292b74b91aad03b55483bb9a84e57feb9930a",
        acceptance: "34515eb786c02fd1d1b14af5ddcc751d4b7e5f9f0ff3bdbf0451ed69e0707866",
        audit: "f95dde156917aa7012e5c090e945e9ceb3748094724f5890adab49c9af7b17a4",
        producer: "63da5ca7776ced528d6817e3dde46d52ce4634d5",
        interpreter: "/opt/tl3/bin/python",
    },
];

fn checked(inputs: &Path, name: &str, expected: &str) -> Result<Value, String> {
    let raw = fs::read(inputs.join(name)).map_err(|error| error.to_string())?;
    if format!("{:x}", Sha256::digest(&raw)) != expected {
        return Err("qualification.input.sha256".into());
    }
    serde_json::from_slice(&raw).map_err(|error| error.to_string())
}

pub fn generate(inputs: &Path) -> Result<Vec<u8>, String> {
    generate_from(|name, expected| checked(inputs, name, expected))
}

fn generate_from(
    mut load: impl FnMut(&str, &str) -> Result<Value, String>,
) -> Result<Vec<u8>, String> {
    let ruling = load("parent-reconciliation.json", RECONCILIATION)?;
    let fault = load("canary2-interpreter-provenance.json", FAULT)?;
    if fault["source"] != SOURCE
        || fault["observed_interpreter_claim"] != false
        || fault["fault_input"]["value"] != RUNS[0].interpreter
    {
        return Err("qualification.negative_fixture".into());
    }
    let mut manifests = Vec::new();
    let mut provenance = Vec::new();
    for run in &RUNS {
        let manifest = load(&format!("{}-manifest.json", run.name), run.manifest)?;
        let acceptance = load(&format!("{}-acceptance.json", run.name), run.acceptance)?;
        let audit = load(&format!("{}-parent-audit.json", run.name), run.audit)?;
        let retained = ruling["runs"]
            .as_array()
            .and_then(|runs| runs.iter().find(|retained| retained["run"] == run.name))
            .ok_or("qualification.accepted_provenance")?;
        if acceptance["status"] != "PASS"
            || acceptance["source"] != SOURCE
            || audit["source"] != SOURCE
            || audit["status"] != format!("PASS_PARENT_RETAINED_{}_AUDIT", run.name.to_uppercase())
            || manifest["producer_commit"] != run.producer
            || manifest["container_interpreter"] != run.interpreter
            || retained["manifest"]["sha256"] != run.manifest
            || retained["accepted_producer_provenance"] != run.producer
            || retained["accepted_interpreter"] != run.interpreter
        {
            return Err("qualification.accepted_provenance".into());
        }
        provenance.push(json!({
            "run": run.name, "manifest_sha256": run.manifest,
            "acceptance_sha256": run.acceptance, "parent_audit_sha256": run.audit,
            "producer_commit": run.producer, "container_interpreter": run.interpreter,
            "citations": retained.get("citations").ok_or("qualification.accepted_provenance")?,
        }));
        manifests.push(manifest);
    }
    let reference = &manifests[1];
    for (run, manifest) in RUNS.iter().zip(&manifests) {
        let mut expected = reference.clone();
        expected["producer_commit"] = run.producer.into();
        expected["container_interpreter"] = run.interpreter.into();
        if *manifest != expected {
            return Err("qualification.execution_metadata".into());
        }
    }
    let files = reference["files"]
        .as_array()
        .ok_or("qualification.inventory")?;
    let paths: Option<BTreeSet<_>> = files.iter().map(|file| file["path"].as_str()).collect();
    if files.len() != 37 || paths.as_ref().is_none_or(|paths| paths.len() != 37) {
        return Err("qualification.inventory".into());
    }
    let mut metadata = reference.clone();
    let metadata = metadata
        .as_object_mut()
        .ok_or("qualification.execution_metadata")?;
    metadata.remove("producer_commit");
    metadata.remove("files");
    let result = json!({
        "schema": "tl3.fx_qualification.v1", "acceptance_source": SOURCE,
        "reconciliation_sha256": RECONCILIATION, "historical_runs": provenance,
        "metadata": metadata,
        "files": files.iter().filter(|file| !PROFILE_FILES.contains(&file["path"].as_str().unwrap())).collect::<Vec<_>>(),
        "profile_files": PROFILE_FILES,
    });
    let mut raw = serde_json::to_vec_pretty(&result).map_err(|error| error.to_string())?;
    raw.push(b'\n');
    Ok(raw)
}

#[cfg(not(test))]
fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[cfg(not(test))]
fn run() -> Result<(), String> {
    let mut root = Path::new(env!("CARGO_MANIFEST_DIR")).to_owned();
    let mut output = root.join("src/adapters/flux_checks/activation/qualification.json");
    let mut check = false;
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--check" {
            check = true;
        } else if arg == "--repo" {
            root = args
                .next()
                .ok_or("--repo requires the controller crate path")?
                .into();
        } else if arg == "--output" {
            output = args.next().ok_or("--output requires a path")?.into();
        } else if arg == "--help" || arg == "-h" {
            println!(
                "Generate the FX pin from hash-checked accepted records, never a candidate bundle.\nUsage: generate-fx-qualification [--output PATH] [--check]"
            );
            return Ok(());
        } else if let Some(path) = arg.to_str().and_then(|arg| arg.strip_prefix("--output=")) {
            output = path.into();
        } else {
            return Err("unrecognized argument".into());
        }
    }
    if output
        == Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/adapters/flux_checks/activation/qualification.json")
    {
        output = root.join("src/adapters/flux_checks/activation/qualification.json");
    }
    let raw = generate_checkout(&root)?;
    if check {
        if fs::read(output).map_err(|error| error.to_string())? != raw {
            return Err("qualification.regeneration".into());
        }
    } else {
        fs::write(output, raw).map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("qualification/fx-exact42b")
    }

    #[test]
    fn hash_check_precedes_json_parsing() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("record.json"), b"{").unwrap();
        assert_eq!(
            checked(directory.path(), "record.json", RECONCILIATION).unwrap_err(),
            "qualification.input.sha256"
        );
        let digest = format!("{:x}", Sha256::digest(b"{"));
        assert!(
            checked(directory.path(), "record.json", &digest)
                .unwrap_err()
                .contains("EOF")
        );
    }

    #[test]
    fn every_accepted_record_is_hash_checked() {
        let directory = tempfile::tempdir().unwrap();
        let names: Vec<_> = fs::read_dir(inputs())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "json")
            })
            .collect();
        assert_eq!(names.len(), 11);
        for path in &names {
            fs::copy(path, directory.path().join(path.file_name().unwrap())).unwrap();
        }
        assert!(generate(directory.path()).is_ok());
        for path in &names {
            let target = directory.path().join(path.file_name().unwrap());
            let mut raw = fs::read(path).unwrap();
            raw.push(b' ');
            fs::write(&target, raw).unwrap();
            assert_eq!(
                generate(directory.path()).unwrap_err(),
                "qualification.input.sha256",
                "{}",
                path.display()
            );
            fs::copy(path, target).unwrap();
        }
    }

    #[test]
    fn retained_semantic_refusals() {
        for (target, field, value, diagnostic) in [
            (
                "canary2-interpreter-provenance.json",
                "/observed_interpreter_claim",
                json!(true),
                "qualification.negative_fixture",
            ),
            (
                "canary2-manifest.json",
                "/container_interpreter",
                json!("/opt/tl3/bin/python"),
                "qualification.accepted_provenance",
            ),
            (
                "parent-reconciliation.json",
                "/runs/0/accepted_producer_provenance",
                json!(SOURCE),
                "qualification.accepted_provenance",
            ),
            (
                "success-acceptance.json",
                "/status",
                json!("FAIL"),
                "qualification.accepted_provenance",
            ),
            (
                "success-parent-audit.json",
                "/source",
                json!("candidate"),
                "qualification.accepted_provenance",
            ),
            (
                "success-manifest.json",
                "/unexpected",
                json!(true),
                "qualification.execution_metadata",
            ),
        ] {
            let result = generate_from(|name, expected| {
                let mut record = checked(&inputs(), name, expected)?;
                if name == target {
                    if field == "/unexpected" {
                        record["unexpected"] = value.clone();
                    } else {
                        *record.pointer_mut(field).unwrap() = value.clone();
                    }
                }
                Ok(record)
            });
            assert_eq!(result.unwrap_err(), diagnostic, "{target}{field}");
        }
        for duplicate in [false, true] {
            let result = generate_from(|name, expected| {
                let mut record = checked(&inputs(), name, expected)?;
                if name.ends_with("-manifest.json") {
                    let files = record["files"].as_array_mut().unwrap();
                    if duplicate {
                        files[1] = files[0].clone();
                    } else {
                        files.pop();
                    }
                }
                Ok(record)
            });
            assert_eq!(result.unwrap_err(), "qualification.inventory");
        }
    }
}
