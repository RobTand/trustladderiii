use super::*;
use tl3_controller::{
    adapters::{flux_checks::activation, workers::execution as worker},
    production,
};

const TARGET: &str = r#"import errno, json, os, pathlib, subprocess, tempfile
names = sorted(name for name in os.environ if name.startswith('FLUX_'))
assert names == ['FLUX_URI']
assert os.environ['FLUX_URI'] == 'local:///run/tl3-flux.sock'
query = subprocess.run(['flux', 'getattr', 'size'], check=True, capture_output=True, text=True, timeout=15)
assert int(query.stdout.strip()) > 0
status = dict(line.split(':', 1) for line in pathlib.Path('/proc/self/status').read_text().splitlines() if ':' in line)
assert int(status['CapEff'].strip(), 16) == 0
assert status['NoNewPrivs'].strip() == '1'
denied = []
for name in ['/x3-write-denied', '/checkout/.git/x3-write-denied', '/harness/x3-write-denied']:
    try:
        with open(name, 'x') as stream: stream.write('unexpected')
    except OSError as error:
        assert error.errno in (errno.EROFS, errno.EACCES, errno.EPERM)
        denied.append(name)
    else:
        raise AssertionError('protected mount was writable')
with tempfile.NamedTemporaryFile(dir='/tmp') as stream:
    stream.write(b'x3'); stream.flush(); os.fsync(stream.fileno())
probe = pathlib.Path('/checkout/x3-private-write')
probe.write_text('x3'); probe.unlink()
routes = pathlib.Path('/proc/net/route').read_text().splitlines()
assert not any(line.split()[1] == '00000000' for line in routes[1:] if line.split())
value = {'broker_read': True, 'capabilities_empty': True, 'flux_names': names,
         'flux_uri': os.environ['FLUX_URI'], 'network_isolated': True, 'no_new_privileges': True,
         'protected_writes_denied': denied, 'private_writes_allowed': True}
with open('/checkout/generated.json', 'x') as stream:
    stream.write(json.dumps(value, sort_keys=True) + '\n'); stream.flush(); os.fsync(stream.fileno())
"#;

pub fn admitted(inputs: &Inputs) -> Result<()> {
    prepare::require_admitted()?;
    let root = inputs.out.join("generator-prepared");
    fs::create_dir(&root)?;
    let mut options = admission::options(inputs, &inputs.out.join("qualified-production"))?;
    options.journal_root = inputs.out.join("generator-journal");
    options.bootstrap.config = options.journal_root.join("options.json");
    fs::create_dir(&options.journal_root)?;
    write(&options.bootstrap.config, &options)?;
    write(
        &root.join("job-environment.json"),
        &production::flux_job_environment(),
    )?;
    let preparation: Value =
        serde_json::from_slice(&durable::read(&inputs.out.join("prepared/result.json"))?)?;
    let bundle: api::BlobRef = serde_json::from_value(preparation["bundles"]["generator"].clone())?;
    let reservation = api::Reservation {
        cpus: 1.try_into()?,
        gpus: 0,
        memory_bytes: (2_u64 << 30).try_into()?,
        memory_guarantee: api::MemoryGuarantee::DeclaredOnly,
        parallelism: 1.try_into()?,
        native_threads: 1.try_into()?,
    };
    let runtime = api::prepared::prepare_runtime(&bundle.path, &reservation, vec![])?;
    let staging = root.join("staging");
    let record = activation::DeploymentRecord {
        schema: "tl3.fx_activation.v2".into(),
        qualification_sha256: activation::qualification_sha256(),
        producer_commit: inputs.head.clone(),
        backend_options_sha256: activation::options_identity(&options)?,
        backend_cli_sha256: activation::executable_sha256(&options.bootstrap.cli)?,
        executable_sha256: activation::executable_sha256(&options.executable)?,
        staging_root_sha256: activation::identity(&staging)?,
        container_mode: tl3_config::ContainerMode::InJob,
        acceptance_scope: activation::AcceptanceScope::PlannedFreshInit,
        profiles: vec![activation::ProfileBinding {
            manifest: bundle.path.clone(),
            manifest_sha256: bundle.sha256,
            image: runtime
                .runtime
                .image
                .clone()
                .ok_or("generator image identity")?,
            preflight: runtime.runtime.preflight.clone(),
            required_native_success_runs: 1.try_into()?,
            required_cleanup: activation::CleanupRequirement::Verified,
        }],
    };
    let candidate = root.join("activation-candidate.json");
    write(&candidate, &record)?;
    let candidate_raw = durable::read(&candidate)?;
    let pin = tl3_config::FluxActivation {
        record: candidate.clone(),
        sha256: durable::digest(&candidate_raw),
    };
    activation::RuntimeActivation::load_bound(
        Some(&pin),
        std::slice::from_ref(&bundle.path),
        tl3_config::ContainerMode::InJob,
        &staging,
        &options,
    )?;
    // Publish exactly the bytes that the actual rebuilt loader accepted.
    let destination = root.join(format!("activation-{}.json", pin.sha256));
    durable::atomic(&destination, &candidate_raw)?;
    let pin = tl3_config::FluxActivation {
        record: destination,
        sha256: pin.sha256,
    };
    activation::RuntimeActivation::load_bound(
        Some(&pin),
        std::slice::from_ref(&bundle.path),
        tl3_config::ContainerMode::InJob,
        &staging,
        &options,
    )?;
    if raw.is_empty() {
        return Err("empty activation record".into());
    }
    let mut delivery =
        super::super::fixture(&root.join("served"), super::super::FixtureKind::Success)?;
    delivery.repository.name = "prismaquant".into();
    let checkout = root.join("checkout");
    fs::create_dir(&checkout)?;
    fs::write(checkout.join("generate.py"), TARGET)?;
    let attempt = root.join("attempt");
    fs::create_dir(&attempt)?;
    let profile_path = attempt.join("execution.json");
    write(
        &attempt.join("request.json"),
        &json!({"worktree":checkout,"inputs":{"tests":{"execution":{"profile":profile_path}}}}),
    )?;
    let profile = worker::Profile {
        pass_flux_uri: true,
        activation: Some(pin),
        schema: "tl3.worker_execution.v1".into(),
        attempt: attempt.clone(),
        checkout: checkout.clone(),
        delivery,
        options: options.bootstrap.config.clone(),
        job_environment: root.join("job-environment.json"),
        staging_root: staging,
        runtime_bundles: vec![bundle.path],
        container_mode: tl3_config::ContainerMode::InJob,
        waits_root: root.join("waits"),
    };
    write(&profile_path, &profile)?;
    let call = durable::digest(inputs.out.as_os_str().as_encoded_bytes());
    let allocated = worker::execute(worker::Command::Allocate {
        profile: profile_path.clone(),
        call: call.clone(),
    })?;
    let transport = Path::new(
        allocated["root"]
            .as_str()
            .ok_or("allocated worker transport")?,
    );
    fs::create_dir(transport.join("checkout"))?;
    fs::write(transport.join("checkout/generate.py"), TARGET)?;
    let trusted = transport.join("trusted/tl3_harness");
    fs::create_dir_all(&trusted)?;
    // The producer inventory, rather than a fixed module count, closes imports.
    let tracked = git(&inputs.repo, &["ls-files", "harness/src/tl3_harness"])?;
    for name in tracked.lines().filter(|name| name.ends_with(".py")) {
        let relative = Path::new(name).strip_prefix("harness/src/tl3_harness")?;
        let destination = trusted.join(relative);
        fs::create_dir_all(destination.parent().ok_or("trusted parent")?)?;
        fs::copy(inputs.repo.join(name), destination)?;
    }
    fs::write(
        transport.join("action.py"),
        "import sys\nfrom pathlib import Path\nsys.path.insert(0,str(Path(__file__).resolve().parent/'trusted'))\nfrom tl3_harness.generator import main\nmain()\n",
    )?;
    let contract = json!({"command":"/opt/tl3/bin/python generate.py","generated_files":["generated.json"],"protected_paths":["generate.py"],
        "files":["generate.py"],"timeout":120,"cpus":1,"mem_gb":2});
    fs::write(
        transport.join("generator.json"),
        serde_json::to_vec(&contract)?,
    )?;
    let bytes = serde_json::to_vec(&contract)?;
    fs::write(transport.join("worker-input.json"), &bytes)?;
    git(transport, &["init", "--quiet", "--template=", "-b", "main"])?;
    git(transport, &["add", "--all"])?;
    git(
        transport,
        &[
            "commit",
            "--quiet",
            "-m",
            "Freeze generator canary transport",
        ],
    )?;
    let tree = git(transport, &["rev-parse", "HEAD^{tree}"])?;
    let plan = worker::execute(worker::Command::Prepare {
        profile: profile_path,
        call,
        served_tree: tree,
        contract: durable::digest(&bytes),
        generator: true,
        image: inputs.generator_image.clone(),
        cpus: 1,
        mem_gb: 2,
        parallel: 1,
        quiet_seconds: 180,
        walltime_seconds: 240,
        arguments: vec![],
    })?;
    let request: api::Request = serde_json::from_value(plan["submit"]["request"].clone())?;
    if !request.pass_flux_uri || request.repo != "RobTand/prismaquant" {
        return Err("PQ pass_flux_uri contract missing".into());
    }
    write(&root.join("request.json"), &request)?;
    write(
        &root.join("result.json"),
        &json!({"options":identity(&options.bootstrap.config)?,"activation":identity(&profile.activation.as_ref().ok_or("activation")?.record)?,
        "request":identity(&root.join("request.json"))?,"worker_plan":plan,"source_script":identity(&checkout.join("generate.py"))?}),
    )?;
    Ok(())
}

pub fn check(inputs: &Inputs, result: &mut evidence::CanaryEvidence) -> Result<()> {
    use base64::Engine;
    result.job.as_ref().ok_or("generator job absent")?;
    let receipt_ref = result
        .terminal_receipt
        .as_ref()
        .ok_or("generator receipt absent")?;
    let receipt: api::Receipt = serde_json::from_slice(&durable::read(&receipt_ref.path)?)?;
    let report: api::RunnerReport = serde_json::from_slice(&durable::read(
        &receipt.report.ok_or("generator report absent")?.path,
    )?)?;
    let api::WorkerPayload::Generator { artifact } =
        report.worker_payload.ok_or("generator payload absent")?
    else {
        return Err("wrong generator payload kind".into());
    };
    let raw = durable::read_limit(&artifact.path, api::GENERATOR_ARTIFACT_BYTES)?;
    if durable::digest(&raw) != artifact.sha256 || raw.len() as u64 != artifact.bytes {
        return Err("generator artifact changed".into());
    }
    let value: Value = serde_json::from_slice(&raw)?;
    let outputs = value["outputs"]
        .as_array()
        .ok_or("generator outputs missing")?;
    let correct = value["status"] == "completed"
        && value["exit_code"] == 0
        && value["identity"]["image"] == inputs.generator_image
        && receipt
            .runtime
            .image
            .as_ref()
            .is_some_and(|image| image.reference == inputs.generator_image)
        && durable::read(&inputs.out.join("generator-prepared/checkout/generate.py"))?
            == TARGET.as_bytes()
        && outputs.len() == 1
        && outputs[0]["path"] == "generated.json";
    result.checks.push(evidence::Check {
        name: "bounded_generator_artifact".into(),
        pass: correct,
        evidence: vec![artifact.clone()],
    });
    if !correct {
        result.status = "FAIL".into();
        return Ok(());
    }
    let bytes = base64::engine::general_purpose::STANDARD.decode(
        outputs[0]["data"]
            .as_str()
            .ok_or("generator output bytes")?,
    )?;
    let target: Value = serde_json::from_slice(&bytes)?;
    let probes = target["flux_uri"] == "local:///run/tl3-flux.sock"
        && target["flux_names"] == json!(["FLUX_URI"])
        && [
            "broker_read",
            "capabilities_empty",
            "network_isolated",
            "no_new_privileges",
            "private_writes_allowed",
        ]
        .iter()
        .all(|name| target[name] == true)
        && target["protected_writes_denied"]
            == json!([
                "/x3-write-denied",
                "/checkout/.git/x3-write-denied",
                "/harness/x3-write-denied"
            ]);
    let output = durable::blob(&inputs.out.join("generator-target-proof.json"), &bytes)?;
    result.artifacts.push(serde_json::to_value(output)?);
    result.checks.push(evidence::Check {
        name: "target_flux_uri_broker_and_containment_probes".into(),
        pass: probes,
        evidence: vec![artifact],
    });
    let mut observed = false;
    for execution in &result.executions {
        let path = Path::new(
            execution["journal"]["path"]
                .as_str()
                .ok_or("execution journal")?,
        );
        for line in fs::read_to_string(path)?.lines() {
            let event: Value = serde_json::from_str(line)?;
            if event["event"] != "container_configuration" {
                continue;
            }
            let facts = &event["facts"];
            let container_bound = receipt
                .runtime_ownerships
                .iter()
                .filter_map(|owned| owned.container.as_ref())
                .any(|container| event["container_id"] == container.id);
            let image = receipt
                .runtime
                .image
                .as_ref()
                .ok_or("runtime image absent")?;
            let actual = facts["Image"]
                .as_str()
                .unwrap_or("")
                .trim_start_matches("sha256:");
            let mounts = facts["Mounts"].as_array().ok_or("runtime mounts absent")?;
            let destinations: std::collections::BTreeSet<_> = mounts
                .iter()
                .filter_map(|m| m["Destination"].as_str())
                .collect();
            let modes = mounts.iter().all(|m| match m["Destination"].as_str() {
                Some("/checkout" | "/tmp") => m["RW"] == true,
                Some("/checkout/.git" | "/harness") => m["RW"] == false,
                Some("/run/tl3-flux.sock") => {
                    m["RW"] == false
                        && receipt
                            .binding
                            .as_ref()
                            .and_then(|binding| binding.id.uri.strip_prefix("local://"))
                            .is_some_and(|source| m["Source"] == source)
                }
                _ => false,
            });
            observed = container_bound
                && actual == image.config_digest.trim_start_matches("sha256:")
                && facts["ImageName"] == image.reference
                && facts["NetworkMode"] == "none"
                && facts["ReadonlyRootfs"] == true
                && facts["Privileged"] == false
                && facts["CapAdd"].as_array().is_none_or(Vec::is_empty)
                && facts["SecurityOpt"].as_array().is_some_and(|v| {
                    v.iter().any(|v| {
                        v.as_str()
                            .is_some_and(|s| s.starts_with("no-new-privileges"))
                    })
                })
                && facts["PidsLimit"]
                    .as_u64()
                    .is_some_and(|n| n > 0 && n <= 4096)
                && facts["PidMode"] != "host"
                && facts["IpcMode"] != "host"
                && facts["Init"] == true
                && modes
                && destinations
                    == std::collections::BTreeSet::from([
                        "/checkout",
                        "/tmp",
                        "/checkout/.git",
                        "/harness",
                        "/run/tl3-flux.sock",
                    ]);
        }
    }
    result.checks.push(evidence::Check {
        name: "actual_image_and_read_only_socket_mount".into(),
        pass: observed,
        evidence: vec![receipt_ref.clone()],
    });
    if result.checks.iter().any(|c| !c.pass) {
        result.status = "FAIL".into();
    }
    Ok(())
}
