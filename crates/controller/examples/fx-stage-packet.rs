//! Preparation-only sandbox fixture. Never constructs or calls a native backend.
//!
//! Canary packets are staged before an activation pin exists, so this example alone uses
//! the `activation-qualification` staging gate. Deployed binaries never enable it.
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Command,
};
use tl3_controller::{
    adapters::flux_checks::staging::Stager,
    executor::Delivery,
    production,
    run_backend::{self as api, ExecutionRuntime, MemoryGuarantee, Options, Reservation},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn git(root: &Path, arguments: &[&str]) -> Result<String> {
    let output = Command::new("/usr/bin/git")
        .current_dir(root)
        .env_clear()
        .envs(production::flux_job_environment())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_DATE", "2026-11-07T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2026-11-07T00:00:00Z")
        .args([
            "-c",
            "user.name=FX2 sandbox fixture",
            "-c",
            "user.email=fixture@localhost",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(arguments)
        .output()?;
    if !output.status.success() {
        return Err("fixture Git command failed".into());
    }
    Ok(String::from_utf8(output.stdout)?.trim().into())
}

fn identity(path: &Path) -> Result<serde_json::Value> {
    let bytes = fs::read(path)?;
    Ok(
        json!({"path": path, "sha256": format!("{:x}", Sha256::digest(&bytes)), "bytes": bytes.len(),
        "mode": fs::metadata(path)?.permissions().mode() & 0o777}),
    )
}

fn write(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    let mut target = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    target.write_all(&serde_json::to_vec_pretty(value)?)?;
    target.write_all(b"\n")?;
    target.sync_all()?;
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FixtureKind {
    Success,
    Cancel,
}

impl FixtureKind {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "success" => Ok(Self::Success),
            "cancel" => Ok(Self::Cancel),
            _ => Err("fixture kind must be success or cancel".into()),
        }
    }

    fn metadata(self) -> serde_json::Value {
        match self {
            Self::Success => json!({"kind":"success", "wait_seconds":0}),
            Self::Cancel => json!({"kind":"cancel", "wait_seconds":12}),
        }
    }

    fn value_source(self, value: u8) -> String {
        match self {
            Self::Success => format!("def value(): return {value}\n"),
            Self::Cancel => {
                format!("def value():\n    import time\n    time.sleep(12)\n    return {value}\n")
            }
        }
    }
}

fn fixture(root: &Path, kind: FixtureKind) -> Result<Delivery> {
    fs::create_dir(root)?;
    git(root, &["init", "--quiet", "--template=", "-b", "main"])?;
    fs::write(
        root.join("pyproject.toml"),
        "[tool.tl3.harness]\ncommand = ['python', '-m', 'pytest']\ntest-files = ['test_*.py']\n",
    )?;
    fs::write(root.join("value.py"), kind.value_source(1))?;
    fs::write(
        root.join("test_value.py"),
        "from value import value\ndef test_value(): assert value() == 2, 'sandbox value must be 2'\n",
    )?;
    git(root, &["add", "."])?;
    git(root, &["commit", "--quiet", "-m", "sandbox canary base"])?;
    let base = git(root, &["rev-parse", "HEAD"])?;
    fs::write(root.join("value.py"), kind.value_source(2))?;
    git(root, &["add", "."])?;
    git(root, &["commit", "--quiet", "-m", "sandbox canary head"])?;
    let head = git(root, &["rev-parse", "HEAD"])?;
    Ok(serde_json::from_value(json!({
        "key":"fx2:preparation-only:sandbox:1", "epoch":1,
        "output":{"Effect":{"node":0,"seq":1,"kind":"SubmitChecks","head":null}},
        "generation":1,"job_generation":1,"head_sha":head,
        "repository":{"owner":"RobTand","name":"tl3-sandbox","installation":1},
        "issue":1,"pull_request":null,"paths":["value.py"],"attempt":1,"round":0,
        "checks":{"repository":root,"base_sha":base,"reviewed_sha":base,
            "plan":[{"name":"sandbox value changes","kind":"demonstrating","tests":["test_value.py::test_value"],
                "proof":{"subjects":["value.py::value"],"observer_support":[],"red_message":"sandbox value must be 2"}}]}
    }))?)
}

fn staging_destination(root: &Path) -> Result<PathBuf> {
    let destination = root.to_str().ok_or("UTF-8 packet destination required")?;
    // Packet labels can resemble secrets. Keep executable paths digest-addressed.
    Ok(Path::new("/mnt/shared/flux-out/fx2-inputs")
        .join(api::durable::digest(destination.as_bytes())))
}

fn prepare(
    root: &Path,
    bundle: &Path,
    context: &Path,
    source: &Path,
    expected_generation: &str,
    kind: FixtureKind,
) -> Result<()> {
    if !api::absolute(root) || !root.starts_with("/mnt/shared/flux-out/tl3") || root.exists() {
        return Err("a fresh absolute shared packet destination is required".into());
    }
    let staging = staging_destination(root)?;
    if staging.exists() {
        return Err("a fresh disjoint staging destination is required".into());
    }
    let mut options: Options = serde_json::from_slice(&fs::read(context)?)?;
    // An explicit supplied generation is required. Never discover or refresh it.
    if !api::generation(expected_generation) || options.instance_generation != expected_generation {
        return Err(
            "context differs from explicitly supplied generation; re-preparation is required"
                .into(),
        );
    }
    fs::create_dir(root)?;
    fs::set_permissions(root, fs::Permissions::from_mode(0o700))?;
    let reservation = Reservation {
        cpus: 1.try_into()?,
        gpus: 0,
        memory_bytes: (2_u64 << 30).try_into()?,
        memory_guarantee: MemoryGuarantee::DeclaredOnly,
        parallelism: 1.try_into()?,
        native_threads: 1.try_into()?,
    };
    let canonical = api::prepared::prepare_runtime(bundle, &reservation, vec![])?;
    let image = canonical
        .runtime
        .image
        .as_ref()
        .ok_or("canonical image identity missing")?;
    if image.profile != "sandbox" {
        return Err("only the sandbox profile is supported by this fixture".into());
    }
    let ExecutionRuntime::RootlessPodman { mode, .. } = canonical.runtime.runtime else {
        return Err("Podman bundle required".into());
    };
    if mode != tl3_config::ContainerMode::InJob {
        return Err("OwnScope remains refused".into());
    }
    let template = source.join("crates/config/example.toml");
    let mut config = tl3_config::Config::parse(&fs::read_to_string(&template)?)?;
    config.execution_backend = tl3_config::ExecutionBackend::Flux;
    config.worktrees_root = root.join("worktrees");
    config.workers.runs = root.join("worker-runs");
    config.store.host = root
        .join("unused-store-socket")
        .to_string_lossy()
        .into_owned();
    config.store.database = "fx2_staging_only".into();
    config.github.app_credentials_file = root.join("unused-github-credentials");
    config.models.key_file = root.join("unused-model-credentials");
    config.github.repos.truncate(1);
    let repo = &mut config.github.repos[0];
    repo.owner = "RobTand".into();
    repo.name = "tl3-sandbox".into();
    repo.installation_id = 1;
    repo.container_image = Some(image.reference.clone());
    repo.test_demand = Some(tl3_config::TestDemand::default());
    repo.selected_tests = None;
    repo.always_tests.clear();
    config.flux = Some(tl3_config::Flux {
        // Canary packets precede the pin they qualify; Stager refuses a record here.
        activation: None,
        instance_generation: options.instance_generation.clone(),
        uri: options.uri.clone(),
        executable: "/no-native-execution/flux".into(),
        backend_cli: "/no-native-execution/tl3-run-backend".into(),
        environment: production::flux_job_environment(),
        container_mode: mode,
        runtime_bundles: vec![bundle.to_owned()],
        staging_root: staging,
        output_root: "/mnt/shared/flux-out/tl3".into(),
        container_image: image.reference.clone(),
    });
    config.validate()?; // Syntax only. No store, secrets, scheduler or container access.
    let delivery = fixture(&root.join("served"), kind)?;
    write(&root.join("delivery.json"), &delivery)?;
    let stager = Stager::for_canary_qualification(&config)?;
    let activation = stager
        .canary_qualification_marker()
        .ok_or("canary packets require the qualification staging gate")?;
    let prepared = stager.prepare_request(&delivery)?;
    let mut request = prepared.request;
    if request.reservation != reservation
        || request.environment != production::flux_job_environment()
    {
        return Err("stager reservation/job environment mismatch".into());
    }
    // A bounded canary packet derives its real action path from Stager, not the old template.
    let mut arguments = request.runner.arguments.clone();
    if arguments.first().map(String::as_str) != Some("--timeout") || arguments.len() < 2 {
        return Err("collector timeout argument missing".into());
    }
    arguments[1] = "60".into();
    let runtime = api::prepared::prepare_runtime(bundle, &request.reservation, arguments)?;
    request.runner = runtime.runner;
    request.runtime = runtime.runtime;
    request.walltime_seconds = 60.try_into()?;
    request.no_progress_seconds = Some(30.try_into()?);
    let run = delivery.checks.as_ref().ok_or("served check run missing")?;
    let head = delivery.head_sha.as_ref().ok_or("served head missing")?;
    let expected_suite = api::Suite::DaytimeChecks {
        base: run.base_sha.as_str().into(),
        head: head.clone(),
        reviewed: run.reviewed_sha.as_ref().map(|sha| sha.as_str().into()),
        plan_digest: api::durable::digest(&serde_json::to_vec(&run.plan)?),
    };
    if request.suite != expected_suite {
        return Err("staged served suite changed".into());
    }
    let staged: tl3_contracts::checks::CheckRequest =
        serde_json::from_slice(&fs::read(request.tree.root.join("request.json"))?)?;
    if staged.head.as_str() != head.as_str() || staged.base.as_str() != run.base_sha.as_str() {
        return Err("served request tuple changed".into());
    }
    git(&request.tree.root, &["bundle", "verify", "served.bundle"])?;
    let manifest: api::InputManifest = serde_json::from_slice(&fs::read(&request.tree.manifest)?)?;
    if manifest.head != request.tree.head || manifest.tree != request.tree.tree {
        return Err("transport manifest identity changed".into());
    }
    if identity(&request.tree.manifest)?["sha256"] != request.tree.input_digest {
        return Err("independent input manifest digest mismatch".into());
    }
    let mut staged_files = Vec::new();
    for entry in &manifest.files {
        let observed = identity(&request.tree.root.join(&entry.path))?;
        if observed["sha256"] != entry.sha256 {
            return Err("independent staged file digest mismatch".into());
        }
        staged_files.push(observed);
    }
    if git(&request.tree.root, &["rev-parse", "HEAD"])? != request.tree.head
        || git(&request.tree.root, &["rev-parse", "HEAD^{tree}"])? != request.tree.tree
    {
        return Err("independent transport Git identity mismatch".into());
    }
    options.journal_root = root.join("held-journal");
    options.bootstrap.config = root.join("held-options.json");
    write(&options.bootstrap.config, &options)?;
    write(&root.join("request.json"), &request)?;
    write(
        &root.join("output-bounds.json"),
        &api::output_bounds(&request.suite),
    )?;
    write(
        &root.join("job-environment.json"),
        &production::flux_job_environment(),
    )?;
    fs::copy(&template, root.join("config-template.toml"))?;
    let mut argv = vec![request.runner.executable.to_string_lossy().into_owned()];
    argv.extend(request.runner.arguments.clone());
    write(
        &root.join("packet.json"),
        &json!({
            "status":"HELD_NOT_RELEASED_NOT_EXECUTED", "runtime_qualified":false, "activation":activation,
            "release_requires":["PB lead DONE and passing checks/receipt", "explicit NEW generation and context verification", "parent inspection and sole FXR execution"],
            "source_snapshot":git(source, &["rev-parse", "HEAD"])?,
            "example_source":identity(&source.join("crates/controller/examples/fx-stage-packet.rs"))?,
            "executable":identity(&std::env::current_exe()?)?, "canonical_bundle":identity(bundle)?,
            "context_input":identity(context)?, "context_copy":identity(&options.bootstrap.config)?,
            "generation":options.instance_generation,"uri":options.uri,
            "fixture":kind.metadata(),
            "served":{"base":run.base_sha,"head":head,"reviewed":run.reviewed_sha,"plan":run.plan,"suite":request.suite},
            "transport":request.tree,"runner_argv_unexecuted":argv,
            "request":identity(&root.join("request.json"))?,"delivery":identity(&root.join("delivery.json"))?,
            "input_manifest":identity(&request.tree.manifest)?,"staged_files":staged_files,
            "output_bounds":identity(&root.join("output-bounds.json"))?,"job_environment":identity(&root.join("job-environment.json"))?,
            "isolated_config":{"template":identity(&root.join("config-template.toml"))?,"worktrees_root":config.worktrees_root,
                "worker_runs":config.workers.runs,"store_host":config.store.host,"store_database":config.store.database,
                "no_store_opened":true,"native_executables":"/no-native-execution/*"},
            "memory_guarantee":"declared_only","native_backend_calls":0,"container_calls":0
        }),
    )?;
    fs::File::open(root)?.sync_all()?;
    println!("{}", identity(&root.join("packet.json"))?);
    Ok(())
}

fn fixture_kind(arguments: &[std::ffi::OsString]) -> Result<FixtureKind> {
    match arguments.len() {
        5 => Ok(FixtureKind::Success),
        7 if arguments[5] == "--fixture-kind" => {
            FixtureKind::parse(arguments[6].to_str().ok_or("UTF-8 fixture kind required")?)
        }
        _ => {
            Err("expected five positional inputs and optional --fixture-kind success|cancel".into())
        }
    }
}

fn main() -> Result<()> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.len() == 1 && arguments[0] == "--help" {
        println!(
            "fx-stage-packet DESTINATION CANONICAL_SANDBOX_MANIFEST CONTEXT_OPTIONS SOURCE_ROOT EXPECTED_GENERATION [--fixture-kind success|cancel]\nPreparation only; no native backend or container operations. Fresh shared destination required."
        );
        return Ok(());
    }
    let kind = fixture_kind(&arguments)?;
    let generation = arguments[4].to_str().ok_or("UTF-8 generation required")?;
    let paths: Vec<PathBuf> = arguments[..4].iter().map(PathBuf::from).collect();
    prepare(&paths[0], &paths[1], &paths[2], &paths[3], generation, kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires admitted PrismaBuild action and FX_FIXTURE_EVIDENCE; run with --include-ignored"]
    fn served_fixture_verdicts_and_wait() {
        assert_eq!(FixtureKind::parse("success").unwrap(), FixtureKind::Success);
        assert_eq!(FixtureKind::parse("cancel").unwrap(), FixtureKind::Cancel);
        assert!(FixtureKind::parse("arbitrary-command").is_err());
        let mut arguments = vec![std::ffi::OsString::from("input"); 5];
        assert_eq!(fixture_kind(&arguments).unwrap(), FixtureKind::Success);
        arguments.extend(["--fixture-kind".into(), "cancel".into()]);
        assert_eq!(fixture_kind(&arguments).unwrap(), FixtureKind::Cancel);
        arguments[6] = "success".into();
        assert_eq!(fixture_kind(&arguments).unwrap(), FixtureKind::Success);
        arguments[6] = "arbitrary-command".into();
        assert!(fixture_kind(&arguments).is_err());
        arguments.pop();
        assert!(fixture_kind(&arguments).is_err());
        let temporary = tempfile::tempdir().unwrap();
        for kind in [FixtureKind::Success, FixtureKind::Cancel] {
            let name = kind.metadata()["kind"].as_str().unwrap().to_owned();
            let delivery = fixture(&temporary.path().join(&name), kind).unwrap();
            write(&temporary.path().join(format!("{name}.json")), &delivery).unwrap();
        }
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let status = Command::new(std::env::var("FX_FIXTURE_PYTHON").unwrap_or("python3".into()))
            .arg(source.join("tools/verify_fx_fixtures.py"))
            .arg(temporary.path())
            .status()
            .unwrap();
        assert!(
            status.success(),
            "served fixture production-path verification failed"
        );
        use tl3_contracts::report::{CheckReport, Outcome, RedMessageMatch, Verdict};
        use tl3_harness_core::judge as shared;
        let evidence = PathBuf::from(std::env::var_os("FX_FIXTURE_EVIDENCE").unwrap());
        let mut judgments = Vec::new();
        for name in ["red-original", "green-success", "green-cancel"] {
            let stdout = fs::read_to_string(evidence.join(name).join("stdout.log")).unwrap();
            let report: CheckReport = serde_json::from_str(
                stdout
                    .lines()
                    .find_map(|line| line.strip_prefix("TL3_CHECKS "))
                    .unwrap(),
            )
            .unwrap();
            let result = &report.results[0];
            let test = &result.tests[0];
            assert_eq!(test.base, Outcome::Assertion);
            assert_eq!(test.head, Outcome::Passed);
            let proof = result.proof.as_ref().unwrap();
            let reason = if proof.declaration.is_none() {
                shared::Reason::Undeclared
            } else {
                assert_eq!(
                    proof.red_message_matches.as_ref().unwrap()["test_value.py::test_value"],
                    RedMessageMatch::Message
                );
                shared::Reason::Matched
            };
            let input = shared::Input {
                kind: shared::Kind::Demonstrating,
                base: shared::Outcome::Assertion,
                head: shared::Outcome::Passed,
                forced: None,
                reason,
            };
            let decision = shared::judge(input);
            assert_eq!(decision.passed, result.verdict == Verdict::Pass);
            assert_eq!(decision.passed, name != "red-original");
            let message = shared::message(input, "", "", None, &[], "");
            if !decision.passed {
                assert_eq!(
                    message,
                    "Assertion RED needs a declared assertion reason in proof.red_message."
                );
            }
            judgments.push(json!({"fixture":name,"passed":decision.passed,"message":message}));
        }
        write(&evidence.join("rust-judge.json"), &judgments).unwrap();
        println!(
            "FX_RUST_JUDGE {}",
            serde_json::to_string(&judgments).unwrap()
        );
    }

    /// Shared-mount paths a fixture created; made writable and removed on drop.
    struct Shared(Vec<PathBuf>);

    impl Drop for Shared {
        fn drop(&mut self) {
            fn writable(path: &Path) {
                if path.is_dir() && !path.is_symlink() {
                    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
                    for child in fs::read_dir(path).into_iter().flatten().flatten() {
                        writable(&child.path());
                    }
                }
            }
            for path in &self.0 {
                writable(path);
                let _ = fs::remove_dir_all(path);
            }
        }
    }

    /// A sealed sandbox bundle built from the current harness, not from any activation pin.
    fn sandbox_bundle(root: &Path, shared: &mut Shared) -> PathBuf {
        let checkout = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let pin: serde_json::Value = serde_json::from_slice(
            &fs::read(
                checkout.join("crates/controller/src/adapters/flux_checks/activation/qualification.json"),
            )
            .unwrap(),
        )
        .unwrap();
        let fixtures = checkout.join("crates/controller/qualification/fx-exact42b/fixtures");
        let source = root.join("source");
        let (inventory, collector, cleanup) = (
            "collector-manifest.json",
            "host-collector",
            "host-cleanup",
        );
        let mut code = Vec::new();
        for file in pin["files"].as_array().unwrap() {
            let path = PathBuf::from(file["path"].as_str().unwrap());
            if path == Path::new(inventory) {
                continue;
            }
            let name = path.file_name().unwrap();
            let original = if fixtures.join(name).is_file() {
                fixtures.join(name)
            } else {
                checkout.join("harness/src/tl3_harness").join(name)
            };
            let raw = fs::read(&original).unwrap();
            fs::create_dir_all(source.join(&path).parent().unwrap()).unwrap();
            fs::write(source.join(&path), &raw).unwrap();
            if path != Path::new(collector) && path != Path::new(cleanup) {
                code.push(json!({"path": path, "bytes": raw.len(),
                    "sha256": api::durable::digest(&raw)}));
            }
        }
        fs::write(
            source.join(inventory),
            serde_json::to_vec(&json!({"interpreter": "/usr/bin/python3.14", "files": code}))
                .unwrap(),
        )
        .unwrap();
        let image = format!("fixture.invalid/sandbox@sha256:{}", "a".repeat(64));
        let config_digest = format!("sha256:{}", "b".repeat(64));
        fs::write(
            root.join("image-map.json"),
            serde_json::to_vec(&json!({"profiles":[{"profile":"sandbox", "registry_ref":image,
                "manifest_digest":format!("sha256:{}", "a".repeat(64)),
                "config_digest":config_digest, "platform":"linux/amd64"}]}))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            root.join("image-inspect.json"),
            serde_json::to_vec(&json!({"Id":config_digest, "Os":"linux", "Architecture":"amd64"}))
                .unwrap(),
        )
        .unwrap();
        let bundle = api::prepared::prepare_bundle(&api::PrepareBundleInput {
            source_root: source,
            code_inventory: inventory.into(),
            collector: collector.into(),
            cleanup: cleanup.into(),
            trusted_root: "lib/tl3_harness".into(),
            producer_commit: "f".repeat(40),
            container_interpreter: "/opt/tl3/bin/python".into(),
            cleanup_arguments: vec![],
            image_inventory: root.join("image-map.json"),
            image_inspect: root.join("image-inspect.json"),
            profile: "sandbox".into(),
            mode: tl3_config::ContainerMode::InJob,
            prerequisites: api::ModePrerequisites {
                no_cgroups_supported: true,
                init_available: true,
                user_scope_supported: false,
            },
        })
        .unwrap();
        shared.0.push(bundle.path.parent().unwrap().to_owned());
        bundle.path
    }

    #[test]
    #[ignore = "PB fixture: canonical shared bundle and packet paths under /mnt/shared"]
    fn canary_packet_stages_without_activation_record() {
        let temporary = tempfile::Builder::new()
            .prefix("fx-qual-staging-test-")
            .tempdir_in("/mnt/shared")
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let mut shared = Shared(vec![]);
        let bundle = sandbox_bundle(&root, &mut shared);
        let generation = format!("fixture/{}", "0".repeat(32));
        let flux = tl3_config::Flux {
            activation: None,
            instance_generation: generation.clone(),
            uri: "local:///fixture-endpoint".into(),
            executable: "/no-native-execution/flux".into(),
            backend_cli: "/no-native-execution/tl3-run-backend".into(),
            environment: production::flux_job_environment(),
            container_mode: tl3_config::ContainerMode::InJob,
            runtime_bundles: vec![bundle.clone()],
            staging_root: root.join("unused-staging"),
            output_root: "/mnt/shared/flux-out/tl3".into(),
            container_image: format!("fixture.invalid/sandbox@sha256:{}", "a".repeat(64)),
        };
        let context = root.join("context.json");
        fs::write(
            &context,
            serde_json::to_vec(&production::flux_options(&flux)).unwrap(),
        )
        .unwrap();
        let destination = Path::new("/mnt/shared/flux-out/tl3").join(format!(
            "fx-qual-staging-test-{}",
            api::durable::digest(root.to_str().unwrap().as_bytes())
        ));
        shared.0.push(destination.clone());
        shared.0.push(staging_destination(&destination).unwrap());
        let checkout = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        prepare(
            &destination,
            &bundle,
            &context,
            &checkout,
            &generation,
            FixtureKind::Success,
        )
        .expect("a canary packet stages without an activation record");
        let packet: serde_json::Value =
            serde_json::from_slice(&fs::read(destination.join("packet.json")).unwrap()).unwrap();
        assert_eq!(packet["status"], "HELD_NOT_RELEASED_NOT_EXECUTED");
        assert_eq!(packet["activation"], "tl3.fx_canary_qualification_staging.v1");
        assert_eq!(
            packet["activation"],
            tl3_controller::adapters::flux_checks::staging::CANARY_QUALIFICATION_MARKER
        );
        assert_eq!(packet["runtime_qualified"], false);
        assert_eq!(packet["native_backend_calls"], 0);
        assert_eq!(packet["container_calls"], 0);
        let request: api::Request =
            serde_json::from_slice(&fs::read(destination.join("request.json")).unwrap()).unwrap();
        assert_eq!(request.walltime_seconds.get(), 60);
        assert!(request.tree.root.starts_with(staging_destination(&destination).unwrap()));
    }

    fn flux_config(activation: Option<tl3_config::FluxActivation>) -> tl3_config::Config {
        let mut config =
            tl3_config::Config::parse(include_str!("../../config/example.toml")).unwrap();
        config.execution_backend = tl3_config::ExecutionBackend::Flux;
        config.flux = Some(tl3_config::Flux {
            activation,
            instance_generation: format!("fixture/{}", "0".repeat(32)),
            uri: "local:///fixture-endpoint".into(),
            executable: "/no-native-execution/flux".into(),
            backend_cli: "/no-native-execution/tl3-run-backend".into(),
            environment: production::flux_job_environment(),
            container_mode: tl3_config::ContainerMode::InJob,
            runtime_bundles: vec![],
            staging_root: "/mnt/shared/flux-out/fx2-inputs/unused".into(),
            output_root: "/mnt/shared/flux-out/tl3".into(),
            container_image: format!("fixture.invalid/sandbox@sha256:{}", "a".repeat(64)),
        });
        config
    }

    fn refusal<T>(result: tl3_controller::Result<T>) -> String {
        result.err().expect("expected a refusal").to_string()
    }

    #[test]
    fn qualification_gate_refuses_records_and_production_still_requires_one() {
        use tl3_controller::adapters::flux_checks::staging::CANARY_QUALIFICATION_MARKER;
        let config = flux_config(None);
        let canary = Stager::for_canary_qualification(&config).unwrap();
        assert_eq!(
            canary.canary_qualification_marker(),
            Some(CANARY_QUALIFICATION_MARKER)
        );
        // The production constructor stays activation-gated in this same build.
        assert_eq!(refusal(Stager::new(&config)), "flux.activation");
        // A configured record must use the gated path; the bypass never downgrades it.
        let record = Some(tl3_config::FluxActivation {
            record: "/mnt/shared/flux-out/tl3/.activation/absent.json".into(),
            sha256: "0".repeat(64),
        });
        let configured = flux_config(record);
        assert_eq!(
            refusal(Stager::for_canary_qualification(&configured)),
            "flux.activation.canary_qualification"
        );
        assert_eq!(refusal(Stager::new(&configured)), "flux.activation.record");
        let mut mutable = flux_config(None);
        mutable.flux.as_mut().unwrap().container_image = "fixture.invalid/sandbox:latest".into();
        assert_eq!(
            refusal(Stager::for_canary_qualification(&mutable)),
            "flux.activation.image"
        );
        let mut prismabuild = flux_config(None);
        prismabuild.execution_backend = tl3_config::ExecutionBackend::PrismaBuild;
        assert!(Stager::for_canary_qualification(&prismabuild).is_err());
    }

    #[test]
    fn staging_namespace_survives_consumer_argument_validation() {
        let root =
            Path::new("/mnt/shared/flux-out/tl3/fx2-staging-c2e2db3250-f107a7b0-20261107T045211Z");
        let staging = staging_destination(root).unwrap();
        assert_eq!(staging, staging_destination(root).unwrap());
        assert_ne!(
            staging,
            staging_destination(&root.with_extension("next")).unwrap()
        );
        let action = staging.join(format!(
            "{}-{}/tree/action.py",
            "a".repeat(64),
            "b".repeat(64)
        ));
        let argument = action.to_str().unwrap();
        assert!(argument.len() < 16_384 && !argument.chars().any(char::is_control));
        assert_eq!(api::durable::safe(argument).unwrap(), argument);
    }
}
