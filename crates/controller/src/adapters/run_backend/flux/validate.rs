use super::*;
use std::collections::BTreeSet;

pub(super) fn options(options: &Options) -> Result<()> {
    if !absolute(&options.executable) || !absolute(&options.journal_root) || !generation(&options.instance_generation)
        || !absolute(&options.bootstrap.cli) || !absolute(&options.bootstrap.config)
        || !options.uri.starts_with("local:///") || options.uri.chars().any(char::is_control)
        || options.environment.get("FLUX_URI").is_some_and(|uri| uri != &options.uri)
        || options.arguments.len() > 32 || !options.environment.contains_key("PATH") {
        return Err(BackendError::invalid("explicit executable, PATH, generation and journal root required"));
    }
    for (name, value) in &options.environment {
        if !matches!(name.as_str(), "PATH" | "LD_LIBRARY_PATH" | "FLUX_URI" | "FLUX_VANILLA_RUN" | "LANG" | "HOME" | "XDG_RUNTIME_DIR" | "DBUS_SESSION_BUS_ADDRESS")
            || value.len() > 4096 || value.chars().any(char::is_control) || safe(value)? != *value {
            return Err(BackendError::invalid("unsupported or sensitive access environment; values withheld"));
        }
    }
    if options.arguments.iter().any(|s| s.len() > 4096 || s.chars().any(char::is_control)) {
        return Err(BackendError::invalid("invalid provisioned CLI arguments"));
    }
    Ok(())
}

pub(super) fn reservation(reservation: &Reservation) -> Result<()> {
    let demand = reservation.parallelism.get().checked_mul(reservation.native_threads.get());
    if demand.is_none_or(|demand| demand > reservation.cpus.get()) || reservation.cpus.get() > 65_536 || reservation.gpus > 256 {
        return Err(BackendError::invalid("parallelism times native threads exceeds aggregate CPU declaration"));
    }
    Ok(())
}

pub(super) fn request(request: &Request, intent: &SubmissionIntent, options: &Options) -> Result<()> {
    reservation(&request.reservation)?;
    if !generation(&intent.instance) || !token(&intent.owner) || !token(&intent.correlation)
        || intent.instance != options.instance_generation || intent.uri != options.uri || !hex(&intent.request_digest, 64)
        || intent.request_digest != digest(&serde_json::to_vec(request)?)
        || intent.output_dir != Path::new("/mnt/shared/flux-out/tl3").join(&intent.correlation) {
        return Err(BackendError::invalid("invalid request/owner/instance/output binding"));
    }
    if !hex(&request.tree.head, 40) || !hex(&request.tree.tree, 40) || !hex(&request.tree.input_digest, 64)
        || !hex(&request.runner.executable_digest, 64) || !hex(&request.runner.manifest_digest, 64) || !hex(&request.runtime.manifest_digest, 64)
        || request.repo.is_empty() || request.repo.len() > 256 || safe(&request.repo)? != request.repo {
        return Err(BackendError::invalid("invalid exact-tree or trusted-runner identity"));
    }
    for path in [&request.tree.root, &request.tree.manifest, &request.runner.executable, &request.runner.manifest, &request.runtime.manifest, &request.scratch_root] {
        if !absolute(path) { return Err(BackendError::invalid("input and runtime paths must be absolute")); }
    }
    for path in [&request.tree.root, &request.tree.manifest, &request.runner.executable, &request.runner.manifest, &request.runtime.manifest] {
        if !path.starts_with("/mnt/shared") { return Err(BackendError::invalid("immutable inputs must reside under approved shared input root")); }
    }
    if request.scratch_root.starts_with("/mnt/shared") { return Err(BackendError::invalid("working scratch must be local, outside shared storage")); }
    match &request.suite {
        Suite::Full { membership_digest } | Suite::SelectedCases { membership_digest } => {
            if !hex(membership_digest, 64) { return Err(BackendError::invalid("membership digest required")); }
        }
        Suite::DaytimeChecks { plan_digest, base, head, reviewed } => {
            if !hex(plan_digest, 64) || !hex(base, 40) || !hex(head, 40) || reviewed.as_ref().is_some_and(|reviewed| !hex(reviewed, 40)) { return Err(BackendError::invalid("daytime plan/base/head and optional reviewed identity required")); }
        }
    }
    if request.environment.len() > 64 || !request.environment.contains_key("PATH") || request.runner.arguments.len() > 128 {
        return Err(BackendError::invalid("explicit bounded job environment and runner argv required"));
    }
    for (name, value) in &request.environment {
        let reserved = name.starts_with("FLUX_") || name.starts_with("TL3_RUN_") || name.starts_with("PRISMABUILD_")
            || matches!(name.as_str(), "TL3_EXECUTION_BACKEND" | "OUT" | "OMP_NUM_THREADS" | "MKL_NUM_THREADS" | "OPENBLAS_NUM_THREADS" | "TORCH_NUM_THREADS" | "NUMEXPR_NUM_THREADS" | "VECLIB_MAXIMUM_THREADS" | "BLIS_NUM_THREADS");
        if reserved || !token(name) || name.bytes().any(|b| !b.is_ascii_alphanumeric() && b != b'_')
            || name.contains("TOKEN") || name.contains("SECRET") || name.contains("PASSWORD") || name.ends_with("KEY")
            || value.len() > 4096 || value.chars().any(char::is_control) || safe(value)? != *value {
            return Err(BackendError::invalid("reserved, sensitive or invalid job environment; values withheld"));
        }
    }
    if request.runner.arguments.iter().any(|s| s.len() > 16_384 || s.chars().any(char::is_control) || safe(s).map_or(true, |v| v != *s)) {
        return Err(BackendError::invalid("invalid trusted-runner arguments"));
    }
    match &request.runtime.runtime {
        ExecutionRuntime::Native { interpreter } => {
            if !absolute(interpreter) { return Err(BackendError::invalid("explicit native interpreter required")); }
        }
        ExecutionRuntime::RootlessPodman { mode, executable, image, trusted_root, interpreter, memory_limit_bytes } => {
            let digest = image.rsplit_once("sha256:").map(|(_, digest)| digest);
            if !absolute(executable) || !absolute(trusted_root) || !absolute(interpreter) || !trusted_root.starts_with("/mnt/shared")
                || !digest.is_some_and(|digest| hex(digest, 64)) || request.runtime.expected_image.as_ref() != Some(image)
                || !request.environment.contains_key("HOME") || !request.environment.contains_key("XDG_RUNTIME_DIR") {
                return Err(BackendError::invalid("rootless Podman requires pinned image, explicit executable/interpreter and shared trusted inputs"));
            }
            let Some(host) = &request.runtime.host_dependency else { return Err(BackendError::invalid("rootless image/architecture availability requires a justified eligible host")); };
            if !token(&host.host) || host.reason.is_empty() || host.reason.len() > 4096 || safe(&host.reason)? != host.reason {
                return Err(BackendError::invalid("invalid image/runtime host dependency"));
            }
            if (*mode == ContainerMode::OwnScope) != memory_limit_bytes.is_some() {
                return Err(BackendError::invalid("own_scope requires explicit per-container memory cap; in_job does not infer enforcement"));
            }
        }
    }
    Ok(())
}

pub(super) fn placement(placement: &Placement) -> Result<()> {
    if placement.architecture != "x86_64" || !placement.host_dependency.as_ref().is_some_and(|host| host.host == "dl380g10" && !host.reason.is_empty() && host.reason.len() <= 4096) {
        return Err(BackendError::invalid("first-cut TL3 CPU runtime requires dl380g10/x86_64 with recorded image dependency"));
    }
    Ok(())
}

pub(super) fn preflight(request: &Request) -> Result<()> {
    placement(&request.runtime.placement())?;
    if request.reservation.memory_guarantee == MemoryGuarantee::RequireAdmission {
        return Err(BackendError::invalid("native request-derived memory admission is unqualified"));
    }
    if request.reservation.gpus > 0 {
        return Err(BackendError::invalid("GPU P-core resource-template and CDI capability not provisioned to this adapter"));
    }
    if !matches!(request.runtime.architecture.as_str(), "x86_64" | "aarch64") {
        return Err(BackendError::invalid("explicit supported runtime architecture required"));
    }
    let preflight: RuntimePreflight = serde_json::from_slice(&checked_blob(&request.runtime.preflight, Path::new("/mnt/shared"))?)?;
    if preflight.schema != "tl3.runtime_preflight.v1" || preflight.architecture != request.runtime.architecture
        || preflight.image_identity != request.runtime.expected_image || !preflight.identity_verified
        || (matches!(request.runtime.runtime, ExecutionRuntime::RootlessPodman { .. }) && !preflight.image_present) {
        return Err(BackendError::invalid("runtime/image identity or architecture is missing or unqualified"));
    }
    if let ExecutionRuntime::RootlessPodman { trusted_root, .. } = &request.runtime.runtime {
        if !trusted_root.join("runner.py").is_file() { return Err(BackendError::invalid("trusted_root must directly contain runner.py")); }
    }
    for (path, expected) in [(&request.tree.manifest, &request.tree.input_digest), (&request.runner.executable, &request.runner.executable_digest),
        (&request.runner.manifest, &request.runner.manifest_digest), (&request.runtime.manifest, &request.runtime.manifest_digest)] {
        if !std::fs::canonicalize(path)?.starts_with("/mnt/shared")
            || digest(&read(path)?) != *expected || !std::fs::metadata(path)?.permissions().readonly() {
            return Err(BackendError::invalid("immutable input/runner/runtime digest or read-only permission mismatch"));
        }
    }
    let manifest: InputManifest = serde_json::from_slice(&read(&request.tree.manifest)?)?;
    if manifest.schema != "tl3.input_manifest.v1" || manifest.head != request.tree.head || manifest.tree != request.tree.tree || manifest.files.len() > 100_000 {
        return Err(BackendError::invalid("exact-tree manifest identity mismatch"));
    }
    let root = std::fs::canonicalize(&request.tree.root)?;
    let mut paths = BTreeSet::new();
    for file in manifest.files {
        if file.path.is_absolute() || file.path.components().any(|c| !matches!(c, std::path::Component::Normal(_))) || !paths.insert(file.path.clone()) {
            return Err(BackendError::invalid("invalid or duplicate input manifest path"));
        }
        let path = root.join(file.path);
        if !std::fs::canonicalize(&path)?.starts_with(&root) || digest(&read(&path)?) != file.sha256 || !std::fs::metadata(&path)?.permissions().readonly() {
            return Err(BackendError::invalid("staged file digest/ownership mismatch"));
        }
    }
    let git = |args: &[&str]| -> Result<Vec<u8>> {
        let output = std::process::Command::new("/usr/bin/git").env_clear().env("PATH", "/usr/bin:/bin").env("GIT_OPTIONAL_LOCKS", "0")
            .arg("-C").arg(&root).args(args).output()?;
        if !output.status.success() || output.stdout.len() > MAX_BYTES { return Err(BackendError::invalid("staged Git identity unavailable")); }
        Ok(output.stdout)
    };
    if git(&["rev-parse", "HEAD"])?.strip_suffix(b"\n") != Some(request.tree.head.as_bytes())
        || git(&["rev-parse", "HEAD^{tree}"])?.strip_suffix(b"\n") != Some(request.tree.tree.as_bytes())
        || !git(&["status", "--porcelain", "--untracked-files=all"])?.is_empty() {
        return Err(BackendError::invalid("staged checkout is dirty or differs from exact head/tree"));
    }
    let tracked = git(&["ls-files", "-z"])?;
    let tracked: BTreeSet<PathBuf> = tracked.split(|b| *b == 0).filter(|s| !s.is_empty())
        .map(|s| std::str::from_utf8(s).map(PathBuf::from).map_err(|_| BackendError::invalid("non-UTF8 tracked path"))).collect::<Result<_>>()?;
    if tracked != paths { return Err(BackendError::invalid("input manifest does not cover the exact tracked tree")); }
    Ok(())
}
