use super::*;
use std::process::{Command, Stdio};

pub fn require_admitted() -> Result<()> {
    let id = std::env::var("FLUX_JOB_ID").map_err(|_| "operation requires an admitted Flux job")?;
    let cgroup = fs::read_to_string("/proc/self/cgroup")?;
    if id.is_empty() || !cgroup.contains("/flux-jobs.slice/shell-") {
        return Err("admitted Flux cgroup required".into());
    }
    Ok(())
}

fn run(command: &mut Command, root: &Path, name: &str) -> Result<Vec<u8>> {
    let output = command.stdin(Stdio::null()).output()?;
    durable::atomic(&root.join(format!("{name}.stdout")), &output.stdout)?;
    durable::atomic(&root.join(format!("{name}.stderr")), &output.stderr)?;
    if !output.status.success() {
        return Err(format!("admitted {name} failed; no substitution permitted").into());
    }
    Ok(output.stdout)
}

pub fn admitted(inputs: &Inputs) -> Result<()> {
    require_admitted()?;
    let root = inputs.out.join("prepared");
    fs::create_dir(&root)?;
    let binary = inputs.out.join("production");
    let options = admission::options(inputs, &binary)?;
    let context = root.join("options.json");
    write(&context, &options)?;
    let environment = tl3_controller::production::flux_job_environment();
    let prerequisites = root.join("mode-prerequisites.json");
    let observed = run(
        Command::new("/usr/bin/podman")
            .args(["info", "--format", "{{.Host.Security.Rootless}}"])
            .env_clear()
            .envs(&environment),
        &root,
        "rootless",
    )?;
    if observed.strip_suffix(b"\n").unwrap_or(&observed) != b"true" {
        return Err("rootless Podman required".into());
    }
    for tool in [
        "/usr/bin/python3.14",
        "/usr/bin/git",
        "/usr/bin/podman",
        "/usr/bin/crun",
        "/usr/bin/catatonit",
    ] {
        if !Path::new(tool).is_file() {
            return Err(format!("missing provisioned tool {tool}").into());
        }
    }
    let filesystem = run(
        Command::new("/usr/bin/findmnt")
            .args([
                "-n",
                "-o",
                "FSTYPE",
                "-T",
                "/home/rob/tl-gates/flux-validation",
            ])
            .env_clear()
            .envs(&environment),
        &root,
        "scratch-filesystem",
    )?;
    if std::str::from_utf8(&filesystem)?.trim() != "xfs" {
        return Err("approved PQ XFS scratch required".into());
    }
    write(
        &prerequisites,
        &api::ModePrerequisites {
            no_cgroups_supported: true,
            init_available: true,
            user_scope_supported: false,
        },
    )?;
    let mut bundles = BTreeMap::new();
    for (kind, profile, interpreter) in [
        ("success", "sandbox", "/opt/tl3/bin/python"),
        (
            "launch_failure",
            "sandbox",
            "/opt/tl3/bin/python-canary2-absent",
        ),
        ("generator", "prismaquant", "/opt/tl3/bin/python"),
    ] {
        let destination = root.join(kind);
        let mut command = Command::new("/usr/bin/python3.14");
        command.env_clear().envs(&environment)
            .arg("-c").arg("import sys; sys.path.insert(0,sys.argv.pop(1)); from tl3_harness.runtime_image import producer_main; producer_main()")
            .arg(inputs.repo.join("harness/src"))
            .arg("--destination").arg(&destination).args(["--host-interpreter","/usr/bin/python3.14","--container-interpreter",interpreter,
                "--podman","/usr/bin/podman","--systemctl","/usr/bin/systemctl","--mode","in_job"])
            .arg("--image-map").arg(&inputs.image_map).args(["--profile",profile,"--host","dl380g10","--platform","linux/amd64"])
            .arg("--producer-commit").arg(&inputs.head).arg("--mode-prerequisites").arg(&prerequisites);
        run(&mut command, &root, &format!("prepare-{kind}"))?;
        let value: Value = serde_json::from_slice(&durable::read(
            &destination.join("prepare-bundle-command.json"),
        )?)?;
        let input: api::PrepareBundleInput = serde_json::from_value(value["input"].clone())?;
        let bundle = api::prepared::prepare_bundle(&input)?;
        bundles.insert(kind.to_owned(), bundle);
    }
    // The image preflight never pulls. Check the real PQ interpreter/tool contract
    // inside this already admitted action, with no network or writable rootfs.
    run(
        Command::new("/usr/bin/podman")
            .env_clear()
            .envs(&environment)
            .args([
                "run",
                "--rm",
                "--cidfile",
                root.join("pq-preflight.cid")
                    .to_str()
                    .ok_or("preflight cidfile")?,
                "--pull=never",
                "--cgroups=disabled",
                "--network=none",
                "--read-only",
                "--cap-drop=ALL",
                "--security-opt=no-new-privileges",
                "--entrypoint",
                "/opt/tl3/bin/python",
                &inputs.generator_image,
                "-c",
                "import shutil; assert shutil.which('git'); assert shutil.which('flux')",
            ]),
        &root,
        "pq-tool-preflight",
    )?;
    for (kind, fixture, bundle_key) in [
        (
            "launch_failure",
            super::super::FixtureKind::Success,
            "launch_failure",
        ),
        ("cancel", super::super::FixtureKind::ReadyCancel, "success"),
        ("success", super::super::FixtureKind::Success, "success"),
    ] {
        let packet = inputs.out.join(format!("packet-{kind}"));
        super::super::prepare(
            &packet,
            &bundles[bundle_key].path,
            &context,
            &inputs.repo,
            "runtime",
            fixture,
        )?;
        let path = packet.join("request.json");
        let mut request: api::Request = serde_json::from_slice(&durable::read(&path)?)?;
        let seconds = if kind == "cancel" { 300 } else { 120 };
        request.runner.arguments[1] = seconds.to_string();
        request.walltime_seconds = seconds.try_into()?;
        request.no_progress_seconds = Some(if kind == "cancel" { 270 } else { 30 }.try_into()?);
        let runtime = api::prepared::prepare_runtime(
            &bundles[bundle_key].path,
            &request.reservation,
            request.runner.arguments.clone(),
        )?;
        request.runner = runtime.runner;
        request.runtime = runtime.runtime;
        durable::atomic(&path, &serde_json::to_vec(&request)?)?;
    }
    write(
        &root.join("result.json"),
        &json!({"bundles":bundles,"host":"dl380g10","source":inputs.head,
        "rootless":identity(&root.join("rootless.stdout"))?,"pq_preflight":identity(&root.join("pq-tool-preflight.stdout"))?}),
    )?;
    Ok(())
}

pub fn lifecycle_inputs(
    inputs: &Inputs,
    canaries: &[evidence::CanaryEvidence],
) -> Result<BTreeMap<String, Vec<u8>>> {
    if canaries.len() != 3 || canaries.iter().any(|c| c.status != "PASS") {
        return Err("three accepted lifecycle canaries required".into());
    }
    let private = inputs.out.join("private");
    fs::create_dir_all(private.join("qualification/fx-current"))?;
    let mut index = d8::current::Index {
        schema: "tl3.fx_qualification_inputs.v1".into(),
        source: inputs.head.clone(),
        inputs: BTreeMap::new(),
        runs: vec![],
        fixtures: BTreeMap::new(),
    };
    let prepared: Value =
        serde_json::from_slice(&durable::read(&inputs.out.join("prepared/result.json"))?)?;
    let mut output = BTreeMap::new();
    for canary in canaries {
        let key = if canary.kind == "launch_failure" {
            "launch_failure"
        } else {
            "success"
        };
        let bundle: api::BlobRef = serde_json::from_value(prepared["bundles"][key].clone())?;
        let raw = durable::read(&bundle.path)?;
        if durable::digest(&raw) != bundle.sha256 || raw.len() as u64 != bundle.bytes {
            return Err("bundle changed before D8".into());
        }
        let manifest_name = format!("{}-manifest.json", canary.kind);
        let acceptance_name = format!("{}-acceptance.json", canary.kind);
        let acceptance = serde_json::to_vec_pretty(
            &json!({"schema":"tl3.fx_canary_acceptance.v1","source":inputs.head,
            "kind":canary.kind,"status":canary.status,"ending":canary.ending,"cleanup_verified":canary.cleanup.as_ref().is_some_and(|c|c.stopped && c.pending_executions.is_empty()),
            "allocation_verified":canary.checks.iter().any(|c|c.name=="shape_allocation"&&c.pass),"job_id":canary.job.as_ref().and_then(|j|j["id"].as_str()),
            "receipt_sha256":canary.terminal_receipt.as_ref().map(|r|&r.sha256),"evidence":canary}),
        )?;
        for (name, bytes) in [
            (&manifest_name, raw.clone()),
            (&acceptance_name, acceptance),
        ] {
            index.inputs.insert(
                name.clone(),
                d8::current::Input {
                    sha256: durable::digest(&bytes),
                    bytes: bytes.len() as u64,
                    mode: 0o644,
                },
            );
            output.insert(format!("{}{}", policy::INPUTS, name), bytes);
        }
        index.runs.push(d8::current::Run {
            kind: canary.kind.clone(),
            manifest: manifest_name,
            acceptance: acceptance_name,
        });
        if canary.kind == "success" {
            let manifest: api::RuntimeBundleManifest = serde_json::from_slice(&raw)?;
            for file in &manifest.files {
                if ["image-map.json", "image-inspect.json", "preflight.json"]
                    .iter()
                    .any(|p| file.path == Path::new(p))
                {
                    continue;
                }
                let bytes =
                    durable::read(&bundle.path.parent().ok_or("bundle root")?.join(&file.path))?;
                if durable::digest(&bytes) != file.sha256 || bytes.len() as u64 != file.bytes {
                    return Err("fixed helper inventory changed".into());
                }
                let name = format!("fixtures/{}", file.path.display());
                index.fixtures.insert(
                    name.clone(),
                    d8::current::Input {
                        sha256: durable::digest(&bytes),
                        bytes: bytes.len() as u64,
                        mode: if file.mode & 0o111 != 0 { 0o755 } else { 0o644 },
                    },
                );
                output.insert(format!("{}{}", policy::INPUTS, name), bytes);
            }
        }
    }
    let fault_name = "negative-interpreter-provenance.json";
    let bytes = serde_json::to_vec_pretty(
        &json!({"source":inputs.head,"observed_interpreter_claim":false,
        "fault_input":{"value":"/opt/tl3/bin/python-canary2-absent"},"only_intended_fault":"container interpreter absent"}),
    )?;
    index.inputs.insert(
        fault_name.into(),
        d8::current::Input {
            sha256: durable::digest(&bytes),
            bytes: bytes.len() as u64,
            mode: 0o644,
        },
    );
    output.insert(format!("{}{}", policy::INPUTS, fault_name), bytes);
    output.insert(policy::INDEX.into(), serde_json::to_vec_pretty(&index)?);
    for (path, bytes) in &output {
        let path = private.join(
            path.strip_prefix("crates/controller/")
                .ok_or("derived output")?,
        );
        fs::create_dir_all(path.parent().ok_or("private parent")?)?;
        durable::atomic(&path, bytes)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644))?;
        if let Some(name) = path
            .strip_prefix(private.join("qualification/fx-current"))
            .ok()
            .and_then(|p| p.to_str())
        {
            if let Some(input) = index.fixtures.get(name) {
                fs::set_permissions(&path, fs::Permissions::from_mode(input.mode))?;
            }
        }
    }
    let pin = d8::current::generate(&private.join("qualification/current.json"))?;
    let private_pin = private.join("src/adapters/flux_checks/activation/qualification.json");
    fs::create_dir_all(private_pin.parent().ok_or("private pin parent")?)?;
    durable::atomic(&private_pin, &pin)?;
    output.insert(policy::PIN.into(), pin);
    Ok(output)
}
