//! Preparation-only sandbox fixture. Never constructs or calls a native backend.
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{fs, io::Write, os::unix::fs::{OpenOptionsExt, PermissionsExt}, path::{Path, PathBuf}, process::Command};
use tl3_controller::{adapters::flux_checks::staging::Stager, executor::Delivery, production,
    run_backend::{self as api, ExecutionRuntime, MemoryGuarantee, Options, Reservation}};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn git(root: &Path, arguments: &[&str]) -> Result<String> {
    let output = Command::new("/usr/bin/git").current_dir(root).env_clear()
        .envs(production::flux_job_environment())
        .env("GIT_CONFIG_NOSYSTEM", "1").env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_DATE", "2026-11-07T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2026-11-07T00:00:00Z")
        .args(["-c", "user.name=FX2 sandbox fixture", "-c", "user.email=fixture@localhost",
            "-c", "core.hooksPath=/dev/null", "-c", "commit.gpgsign=false"])
        .args(arguments).output()?;
    if !output.status.success() { return Err("fixture Git command failed".into()); }
    Ok(String::from_utf8(output.stdout)?.trim().into())
}

fn identity(path: &Path) -> Result<serde_json::Value> {
    let bytes = fs::read(path)?;
    Ok(json!({"path": path, "sha256": format!("{:x}", Sha256::digest(&bytes)), "bytes": bytes.len(),
        "mode": fs::metadata(path)?.permissions().mode() & 0o777}))
}

fn write(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    let mut target = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)?;
    target.write_all(&serde_json::to_vec_pretty(value)?)?;
    target.write_all(b"\n")?;
    target.sync_all()?;
    Ok(())
}

fn fixture(root: &Path) -> Result<Delivery> {
    fs::create_dir(root)?;
    git(root, &["init", "--quiet", "--template=", "-b", "main"])?;
    fs::write(root.join("pyproject.toml"), "[tool.tl3.harness]\ncommand = ['python', '-m', 'pytest']\ntest-files = ['test_*.py']\n")?;
    fs::write(root.join("value.py"), "def value(): return 1\n")?;
    fs::write(root.join("test_value.py"), "from value import value\ndef test_value(): assert value() == 2\n")?;
    git(root, &["add", "."])?;
    git(root, &["commit", "--quiet", "-m", "sandbox canary base"])?;
    let base = git(root, &["rev-parse", "HEAD"])?;
    fs::write(root.join("value.py"), "def value(): return 2\n")?;
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
            "plan":[{"name":"sandbox value changes","kind":"demonstrating","tests":["test_value.py::test_value"]}]}
    }))?)
}

fn prepare(root: &Path, bundle: &Path, context: &Path, source: &Path) -> Result<()> {
    if !api::absolute(root) || !root.starts_with("/mnt/shared/flux-out/tl3") || root.exists() {
        return Err("a fresh absolute shared packet destination is required".into());
    }
    let mut options: Options = serde_json::from_slice(&fs::read(context)?)?;
    // The retained context is deliberately not refreshed or queried.
    if options.instance_generation != "fv1-model/61f47a7f95af46d794ca89c3a71f0654" {
        return Err("this held fixture requires the reviewed old context; explicit re-preparation is required".into());
    }
    fs::create_dir(root)?;
    fs::set_permissions(root, fs::Permissions::from_mode(0o700))?;
    let reservation = Reservation { cpus: 1.try_into()?, gpus: 0, memory_bytes: (2_u64 << 30).try_into()?,
        memory_guarantee: MemoryGuarantee::DeclaredOnly, parallelism: 1.try_into()?, native_threads: 1.try_into()? };
    let canonical = api::prepared::prepare_runtime(bundle, &reservation, vec![])?;
    let image = canonical.runtime.image.as_ref().ok_or("canonical image identity missing")?;
    if image.profile != "sandbox" { return Err("only the sandbox profile is supported by this fixture".into()); }
    let ExecutionRuntime::RootlessPodman { mode, .. } = canonical.runtime.runtime else { return Err("Podman bundle required".into()); };
    if mode != tl3_config::ContainerMode::InJob { return Err("OwnScope remains refused".into()); }
    let template = source.join("crates/config/example.toml");
    let mut config = tl3_config::Config::parse(&fs::read_to_string(&template)?)?;
    config.execution_backend = tl3_config::ExecutionBackend::Flux;
    config.worktrees_root = root.join("worktrees");
    config.workers.runs = root.join("worker-runs");
    config.store.host = root.join("unused-store-socket").to_string_lossy().into_owned();
    config.store.database = "fx2_staging_only".into();
    config.github.app_credentials_file = root.join("unused-github-credentials");
    config.models.key_file = root.join("unused-model-credentials");
    config.github.repos.truncate(1);
    let repo = &mut config.github.repos[0];
    repo.owner = "RobTand".into(); repo.name = "tl3-sandbox".into(); repo.installation_id = 1;
    repo.container_image = Some(image.reference.clone());
    repo.test_demand = Some(tl3_config::TestDemand::default());
    repo.selected_tests = None; repo.always_tests.clear();
    config.flux = Some(tl3_config::Flux {
        instance_generation: options.instance_generation.clone(), uri: options.uri.clone(),
        executable: "/no-native-execution/flux".into(), backend_cli: "/no-native-execution/tl3-run-backend".into(),
        environment: production::flux_job_environment(), container_mode: mode, runtime_bundles: vec![bundle.to_owned()],
        staging_root: root.join("inputs"), output_root: "/mnt/shared/flux-out/tl3".into(), container_image: image.reference.clone(),
    });
    config.validate()?; // Syntax only. No store, secrets, scheduler or container access.
    let delivery = fixture(&root.join("served"))?;
    write(&root.join("delivery.json"), &delivery)?;
    let prepared = Stager::new(&config)?.prepare_request(&delivery)?;
    let mut request = prepared.request;
    if request.reservation != reservation || request.environment != production::flux_job_environment() {
        return Err("stager reservation/job environment mismatch".into());
    }
    // A bounded canary packet derives its real action path from Stager, not the old template.
    let mut arguments = request.runner.arguments.clone();
    if arguments.first().map(String::as_str) != Some("--timeout") || arguments.len() < 2 { return Err("collector timeout argument missing".into()); }
    arguments[1] = "60".into();
    let runtime = api::prepared::prepare_runtime(bundle, &request.reservation, arguments)?;
    request.runner = runtime.runner;
    request.runtime = runtime.runtime;
    request.walltime_seconds = 60.try_into()?;
    request.no_progress_seconds = Some(30.try_into()?);
    let run = delivery.checks.as_ref().ok_or("served check run missing")?;
    let head = delivery.head_sha.as_ref().ok_or("served head missing")?;
    let expected_suite = api::Suite::DaytimeChecks { base: run.base_sha.as_str().into(), head: head.clone(),
        reviewed: run.reviewed_sha.as_ref().map(|sha| sha.as_str().into()), plan_digest: api::durable::digest(&serde_json::to_vec(&run.plan)?) };
    if request.suite != expected_suite { return Err("staged served suite changed".into()); }
    let staged: tl3_contracts::checks::CheckRequest = serde_json::from_slice(&fs::read(request.tree.root.join("request.json"))?)?;
    if staged.head != *head || staged.base != run.base_sha.as_str() { return Err("served request tuple changed".into()); }
    git(&request.tree.root, &["bundle", "verify", "served.bundle"])?;
    let manifest: api::InputManifest = serde_json::from_slice(&fs::read(&request.tree.manifest)?)?;
    if manifest.head != request.tree.head || manifest.tree != request.tree.tree { return Err("transport manifest identity changed".into()); }
    let mut staged_files = Vec::new();
    for entry in &manifest.files {
        let observed = identity(&request.tree.root.join(&entry.path))?;
        if observed["sha256"] != entry.sha256 { return Err("independent staged file digest mismatch".into()); }
        staged_files.push(observed);
    }
    if git(&request.tree.root, &["rev-parse", "HEAD"])? != request.tree.head
        || git(&request.tree.root, &["rev-parse", "HEAD^{tree}"])? != request.tree.tree {
        return Err("independent transport Git identity mismatch".into());
    }
    options.journal_root = root.join("held-journal");
    options.bootstrap.config = root.join("held-options.json");
    write(&options.bootstrap.config, &options)?;
    write(&root.join("request.json"), &request)?;
    write(&root.join("output-bounds.json"), &api::output_bounds(&request.suite))?;
    write(&root.join("job-environment.json"), &production::flux_job_environment())?;
    fs::copy(&template, root.join("config-template.toml"))?;
    let mut argv = vec![request.runner.executable.to_string_lossy().into_owned()];
    argv.extend(request.runner.arguments.clone());
    write(&root.join("packet.json"), &json!({
        "status":"HELD_NOT_RELEASED_NOT_EXECUTED", "runtime_qualified":false,
        "release_requires":["PB lead DONE and passing checks/receipt", "explicit NEW generation and context verification", "parent inspection and sole FXR execution"],
        "source_snapshot":git(source, &["rev-parse", "HEAD"])?,
        "example_source":identity(&source.join("crates/controller/examples/fx-stage-packet.rs"))?,
        "executable":identity(&std::env::current_exe()?)?, "canonical_bundle":identity(bundle)?,
        "context_input":identity(context)?, "context_copy":identity(&options.bootstrap.config)?,
        "generation":options.instance_generation,"uri":options.uri,
        "served":{"base":run.base_sha,"head":head,"reviewed":run.reviewed_sha,"plan":run.plan,"suite":request.suite},
        "transport":request.tree,"runner_argv_unexecuted":argv,
        "request":identity(&root.join("request.json"))?,"delivery":identity(&root.join("delivery.json"))?,
        "input_manifest":identity(&request.tree.manifest)?,"staged_files":staged_files,
        "output_bounds":identity(&root.join("output-bounds.json"))?,"job_environment":identity(&root.join("job-environment.json"))?,
        "isolated_config":{"template":identity(&root.join("config-template.toml"))?,"worktrees_root":config.worktrees_root,
            "worker_runs":config.workers.runs,"store_host":config.store.host,"store_database":config.store.database,
            "no_store_opened":true,"native_executables":"/no-native-execution/*"},
        "memory_guarantee":"declared_only","native_backend_calls":0,"container_calls":0
    }))?;
    fs::File::open(root)?.sync_all()?;
    println!("{}", identity(&root.join("packet.json"))?);
    Ok(())
}

fn main() -> Result<()> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.len() == 1 && arguments[0] == "--help" {
        println!("fx-stage-packet DESTINATION CANONICAL_SANDBOX_MANIFEST OLD_CONTEXT_OPTIONS SOURCE_ROOT\nPreparation only; no native backend or container operations. Fresh shared destination required.");
        return Ok(());
    }
    if arguments.len() != 4 { return Err("expected destination, canonical sandbox manifest, old context options, source root".into()); }
    let paths: Vec<PathBuf> = arguments.into_iter().map(PathBuf::from).collect();
    prepare(&paths[0], &paths[1], &paths[2], &paths[3])
}
