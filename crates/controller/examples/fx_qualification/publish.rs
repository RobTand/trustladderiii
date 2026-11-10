use super::*;

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
    let staged = bb::candidate(inputs, outputs)?;
    let build: bb::Build = serde_json::from_slice(&durable::read(
        &inputs.out.join("qualified-production/build.json"),
    )?)?;
    if build.build_commit != staged.commit || build.overlay_tree != staged.tree {
        return Err("candidate pin commit differs from rebuilt production".into());
    }
    let head = git(&inputs.repo, &["rev-parse", "HEAD"])?;
    if head != inputs.head && head != staged.commit {
        return Err("publication checkout moved from H or candidate".into());
    }
    if !git(
        &inputs.repo,
        &["status", "--porcelain", "--untracked-files=all"],
    )?
    .is_empty()
    {
        return Err("publication requires a clean operator checkout".into());
    }
    let journal = inputs.out.join("publication.json");
    let bytes = serde_json::to_vec_pretty(&staged)?;
    if journal.exists() && durable::read(&journal)? != bytes {
        return Err("publication candidate changed".into());
    }
    if !journal.exists() {
        durable::atomic(&journal, &bytes)?;
    }
    // The staged transport commit is also the final pin commit. No second
    // commit with a different identity or tree is manufactured at publication.
    if head != staged.commit {
        git(
            &inputs.repo,
            &["merge", "--ff-only", "--no-edit", &staged.commit],
        )?;
    }
    if git(&inputs.repo, &["rev-parse", "HEAD"])? != staged.commit
        || git(&inputs.repo, &["rev-parse", "HEAD^{tree}"])? != staged.tree
    {
        return Err("published candidate commit or tree differs".into());
    }
    let regenerated = d8::generate_checkout(&inputs.repo.join("crates/controller"))?;
    if regenerated != *pin || durable::read(&inputs.repo.join(policy::PIN))? != *pin {
        return Err("published pin differs from regeneration".into());
    }
    let mut installed = vec![];
    for path in policy::publication_order(&outputs.keys().cloned().collect::<Vec<_>>())? {
        let mut value = identity(&inputs.repo.join(&path))?;
        value["relative_path"] = path.into();
        installed.push(value);
    }
    Ok(
        json!({"path":inputs.repo.join(policy::PIN),"sha256":durable::digest(pin),"bytes":pin.len(),
        "commit":staged.commit,"tree":staged.tree,"transport_branch":staged.branch,
        "inputs_index":identity(&inputs.repo.join(policy::INDEX))?,"check":"byte_identical",
        "rebuilt_tree":build.overlay_tree,"activation_record":verified["deployment"],"installed_outputs":installed}),
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
