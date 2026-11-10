use super::*;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Publication {
    schema: String,
    preimages: BTreeMap<String, Option<String>>,
    outputs: BTreeMap<String, Option<String>>,
}

pub fn verify_private(inputs: &Inputs) -> Result<()> {
    prepare::require_admitted()?;
    let mut canary: evidence::CanaryEvidence = serde_json::from_slice(&durable::read(
        &inputs.out.join("canary-generator/result.json"),
    )?)?;
    evidence::verify_references(&serde_json::to_value(&canary)?)?;
    if let Err(error) = generator::check(inputs, &mut canary) {
        canary.status = "FAIL".into();
        let reason = durable::blob(
            &inputs.out.join("generator-validation-error.json"),
            &serde_json::to_vec(&json!({"reason":error.to_string()}))?,
        )?;
        canary.checks.push(evidence::Check {
            name: "admitted_generator_validation".into(),
            pass: false,
            evidence: vec![reason],
        });
    }
    durable::atomic(
        &inputs.out.join("generator-checked.json"),
        &serde_json::to_vec_pretty(&canary)?,
    )?;
    if canary.status != "PASS" {
        return Err("admitted generator validation failed".into());
    }
    let private = inputs.out.join("private");
    let generated = d8::current::generate(&private.join("qualification/current.json"))?;
    if durable::digest(&generated)
        != tl3_controller::adapters::flux_checks::activation::qualification_sha256()
    {
        return Err("rebuilt production pin differs from D8 inputs".into());
    }
    let generator: Value = serde_json::from_slice(&durable::read(
        &inputs.out.join("generator-prepared/result.json"),
    )?)?;
    evidence::verify_references(&generator)?;
    // Re-run the production activation boundary with the exact prepared options,
    // bundle set and private record. No service registration or deployment.
    let profile_path = inputs.out.join("generator-prepared/attempt/execution.json");
    tl3_controller::adapters::workers::execution::execute(
        tl3_controller::adapters::workers::execution::Command::Activation {
            profile: profile_path,
            image: inputs.generator_image.clone(),
        },
    )?;
    let mut fixtures = vec![];
    for (index, (binary, filter)) in [
        (
            "lib_test_tl3_controller_binary",
            "adapters::flux_checks::activation::tests::",
        ),
        (
            "example_test_fx-stage-packet_binary",
            "tests::canary_packet_stages_without_activation_record",
        ),
        (
            "lib_test_tl3_controller_binary",
            "adapters::workers::execution::inputs::tests::direct_prepare_requires_deployment_activation_and_checks_actual_options",
        ),
    ].into_iter().enumerate() {
        let output =
            std::process::Command::new(inputs.out.join("qualified-production").join(binary))
                .args([
                    filter,
                    "--include-ignored",
                    "--test-threads=1",
                    "--nocapture",
                ])
                .env_clear()
                .envs(tl3_controller::production::flux_job_environment())
                .env("TL3_FX_QUALIFICATION_ROOT", &private)
                .env("TL3_FX_QUALIFICATION_CHECKOUT", &inputs.repo)
                .stdin(std::process::Stdio::null())
                .output()?;
        let stdout = durable::blob(&inputs.out.join(format!("{binary}-{index}.stdout")), &output.stdout)?;
        let stderr = durable::blob(&inputs.out.join(format!("{binary}-{index}.stderr")), &output.stderr)?;
        let log = String::from_utf8_lossy(&output.stdout);
        let pass = output.status.success() && fixtures_passed(&log);
        fixtures.push(json!({"binary":binary,"filter":filter,"exit":output.status.code(),"pass":pass,"stdout":stdout,"stderr":stderr}));
    }
    write(&inputs.out.join("canonical-fixtures.json"), &fixtures)?;
    if fixtures.iter().any(|fixture| fixture["pass"] != true) {
        return Err("canonical-path fixtures failed; no publication".into());
    }
    durable::atomic(
        &inputs.out.join("private-verification.json"),
        &serde_json::to_vec_pretty(&json!({
        "status":"PASS","d8_byte_identical":true,"production_activation_load":true,
        "qualification_sha256":durable::digest(&generated),"deployment":generator["activation"],"canonical_fixtures":fixtures}))?,
    )?;
    Ok(())
}

pub fn publish(
    inputs: &Inputs,
    outputs: &BTreeMap<String, Vec<u8>>,
    canaries: &[evidence::CanaryEvidence],
) -> Result<Value> {
    let decisions = evidence::decisions(canaries)?;
    if policy::verdict(&decisions, true) != policy::Status::Pass {
        return Err("publication requires all four accepted canaries".into());
    }
    for canary in canaries {
        evidence::verify_references(&serde_json::to_value(canary)?)?;
    }
    let verified: Value = serde_json::from_slice(&durable::read(
        &inputs.out.join("private-verification.json"),
    )?)?;
    let pin = outputs.get(policy::PIN).ok_or("generated pin missing")?;
    if verified["status"] != "PASS" || verified["qualification_sha256"] != durable::digest(pin) {
        return Err("final admitted verification missing or changed".into());
    }
    let journal = inputs.out.join("publication.json");
    let mut hashes: BTreeMap<_, _> = outputs
        .iter()
        .map(|(path, bytes)| (path.clone(), Some(durable::digest(bytes))))
        .collect();
    // Previous current inputs are owned derived files. Remove an obsolete one
    // only after the previous index proves its exact bytes, never by sweeping.
    let previous_index = inputs.repo.join(policy::INDEX);
    if previous_index.exists() && !journal.exists() {
        let previous: d8::current::Index =
            serde_json::from_slice(&durable::read(&previous_index)?)?;
        for (name, input) in previous.inputs.iter().chain(&previous.fixtures) {
            let path = format!("{}{}", policy::INPUTS, name);
            if !outputs.contains_key(&path) {
                let bytes = durable::read(&inputs.repo.join(&path))?;
                if durable::digest(&bytes) != input.sha256 || bytes.len() as u64 != input.bytes {
                    return Err("previous derived input changed".into());
                }
                hashes.insert(path, None);
            }
        }
    }
    if journal.exists() {
        let previous: Publication = serde_json::from_slice(&durable::read(&journal)?)?;
        for (path, hash) in previous.outputs {
            if hash.is_none() && policy::derived(&path) {
                hashes.insert(path, None);
            }
        }
    }
    let paths = policy::publication_order(&hashes.keys().cloned().collect::<Vec<_>>())?;
    let publication = if journal.exists() {
        let publication: Publication = serde_json::from_slice(&durable::read(&journal)?)?;
        if publication.schema != "tl3.fx_pin_publication.v1" || publication.outputs != hashes {
            return Err("publication inputs changed".into());
        }
        publication
    } else {
        let mut preimages = BTreeMap::new();
        for path in &paths {
            let target = inputs.repo.join(path);
            if target.ancestors().any(Path::is_symlink) {
                return Err("publication symlink refused".into());
            }
            preimages.insert(
                path.clone(),
                if target.exists() {
                    Some(durable::digest(&durable::read(&target)?))
                } else {
                    None
                },
            );
        }
        let publication = Publication {
            schema: "tl3.fx_pin_publication.v1".into(),
            preimages,
            outputs: hashes,
        };
        write(&journal, &publication)?;
        publication
    };
    for path in &paths {
        let target = inputs.repo.join(path);
        let observed = if target.exists() {
            Some(durable::digest(&durable::read(&target)?))
        } else {
            None
        };
        if observed != publication.preimages[path] && observed != publication.outputs[path] {
            return Err("publication target differs from both retained preimage and output".into());
        }
    }
    let mut installed = vec![];
    let index: d8::current::Index =
        serde_json::from_slice(outputs.get(policy::INDEX).ok_or("index missing")?)?;
    for path in paths {
        let target = inputs.repo.join(&path);
        fs::create_dir_all(target.parent().ok_or("publication parent")?)?;
        if publication.outputs[&path].is_none() {
            if target.exists() {
                fs::remove_file(&target)?;
                fs::File::open(target.parent().ok_or("publication parent")?)?.sync_all()?;
            }
            continue;
        }
        durable::atomic(&target, &outputs[&path])?;
        fs::set_permissions(&target, fs::Permissions::from_mode(0o644))?;
        if let Some(input) = path
            .strip_prefix(policy::INPUTS)
            .and_then(|p| index.fixtures.get(p))
        {
            fs::set_permissions(&target, fs::Permissions::from_mode(input.mode))?;
        }
        let mut value = identity(&target)?;
        value["relative_path"] = path.into();
        installed.push(value);
    }
    let regenerated = d8::generate_checkout(&inputs.repo.join("crates/controller"))?;
    if regenerated != *pin || durable::read(&inputs.repo.join(policy::PIN))? != *pin {
        return Err("published pin differs from regeneration".into());
    }
    Ok(
        json!({"path":inputs.repo.join(policy::PIN),"sha256":durable::digest(pin),"bytes":pin.len(),
        "inputs_index":identity(&inputs.repo.join(policy::INDEX))?,"check":"byte_identical",
        "rebuilt_tree":serde_json::from_slice::<Value>(&durable::read(&inputs.out.join("qualified-production/runtime-build.json"))?)?["tree"],
        "activation_record":verified["deployment"],"installed_outputs":installed}),
    )
}

fn fixtures_passed(log: &str) -> bool {
    let summaries: Vec<_> = log
        .lines()
        .filter(|line| line.starts_with("test result:"))
        .collect();
    !summaries.is_empty()
        && summaries.iter().all(|line| {
            let Some(counts) = line.strip_prefix("test result: ok. ") else {
                return false;
            };
            let mut counts = counts.split(';').map(str::trim);
            let passed = counts
                .next()
                .and_then(|s| s.strip_suffix(" passed"))
                .and_then(|n| n.parse::<u64>().ok());
            let failed = counts
                .next()
                .and_then(|s| s.strip_suffix(" failed"))
                .and_then(|n| n.parse::<u64>().ok());
            passed.is_some_and(|n| n > 0) && failed == Some(0)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixture_counts_require_tests_and_zero_failures() {
        for passed in [1, 10, 17, 20, 100] {
            assert!(fixtures_passed(&format!(
                "test result: ok. {passed} passed; 0 failed; 0 ignored;"
            )));
        }
        for log in [
            "",
            "test result: ok. 0 passed; 0 failed;",
            "test result: FAILED. 10 passed; 1 failed;",
            "test result: ok. 10 passed; 1 failed;",
            "test result: ok. nope passed; 0 failed;",
        ] {
            assert!(!fixtures_passed(log));
        }
    }
}
