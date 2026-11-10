use super::*;
use std::collections::BTreeSet;

/// The single host the first-cut TL3 CPU runtime may be placed on.
pub(super) const FIRST_CUT_HOST: &str = "dl380g10";

pub(super) fn options(options: &Options) -> Result<()> {
    if !absolute(&options.executable)
        || !absolute(&options.journal_root)
        || !absolute(&options.bootstrap.cli)
        || !absolute(&options.bootstrap.config)
        || options
            .instance_file
            .as_ref()
            .is_some_and(|path| !absolute(path))
        || (options.instance_file.is_none() && !options.uri.starts_with("local:///"))
        || options.uri.chars().any(char::is_control)
        || options.arguments.len() > 32
        || !options.environment.contains_key("PATH")
    {
        return Err(BackendError::invalid(
            "explicit executable, PATH, runtime locator and journal root required",
        ));
    }
    for (name, value) in &options.environment {
        if !matches!(
            name.as_str(),
            "PATH"
                | "LD_LIBRARY_PATH"
                | "FLUX_URI"
                | "FLUX_VANILLA_RUN"
                | "LANG"
                | "HOME"
                | "XDG_RUNTIME_DIR"
        ) || value.len() > 4096
            || value.chars().any(char::is_control)
            || safe(value)? != *value
        {
            return Err(BackendError::invalid(
                "unsupported or sensitive access environment; values withheld",
            ));
        }
    }
    if options
        .arguments
        .iter()
        .any(|s| s.len() > 4096 || s.chars().any(char::is_control))
    {
        return Err(BackendError::invalid("invalid provisioned CLI arguments"));
    }
    Ok(())
}

pub(super) fn reservation(reservation: &Reservation) -> Result<()> {
    let demand = reservation
        .parallelism
        .get()
        .checked_mul(reservation.native_threads.get());
    if demand.is_none_or(|demand| demand > reservation.cpus.get())
        || reservation.cpus.get() > 65_536
        || reservation.gpus > 256
    {
        return Err(BackendError::invalid(
            "parallelism times native threads exceeds aggregate CPU declaration",
        ));
    }
    if reservation.cpus.get() > 10 || reservation.memory_bytes.get() > (24_u64 << 30) {
        return Err(BackendError::invalid(
            "request exceeds dl380g10 plain limit of 10 cores / 24 GiB",
        ));
    }
    Ok(())
}

pub(super) fn request(
    request: &Request,
    intent: &SubmissionIntent,
    _options: &Options,
) -> Result<()> {
    reservation(&request.reservation)?;
    if !generation(&intent.instance)
        || !token(&intent.owner)
        || !token(&intent.correlation)
        || !hex(&intent.request_digest, 64)
        || intent.request_digest != digest(&serde_json::to_vec(request)?)
        || intent.output_dir != Path::new("/mnt/shared/flux-out/tl3").join(&intent.correlation)
    {
        return Err(BackendError::invalid(
            "invalid request/owner/instance/output binding",
        ));
    }
    if !hex(&request.tree.head, 40)
        || !hex(&request.tree.tree, 40)
        || !hex(&request.tree.input_digest, 64)
        || !hex(&request.runner.executable_digest, 64)
        || !hex(&request.runner.manifest_digest, 64)
        || !hex(&request.runtime.manifest_digest, 64)
        || request.repo.is_empty()
        || request.repo.len() > 256
        || safe(&request.repo)? != request.repo
    {
        return Err(BackendError::invalid(
            "invalid exact-tree or trusted-runner identity",
        ));
    }
    for path in [
        &request.tree.root,
        &request.tree.manifest,
        &request.runner.executable,
        &request.runner.manifest,
        &request.runtime.manifest,
        &request.scratch_root,
    ] {
        if !absolute(path) {
            return Err(BackendError::invalid(
                "input and runtime paths must be absolute",
            ));
        }
    }
    for path in [
        &request.tree.root,
        &request.tree.manifest,
        &request.runner.executable,
        &request.runner.manifest,
        &request.runtime.manifest,
    ] {
        if !path.starts_with("/mnt/shared") {
            return Err(BackendError::invalid(
                "immutable inputs must reside under approved shared input root",
            ));
        }
    }
    if request.scratch_root.starts_with("/mnt/shared") {
        return Err(BackendError::invalid(
            "working scratch must be local, outside shared storage",
        ));
    }
    match &request.suite {
        Suite::Full { membership_digest } | Suite::SelectedCases { membership_digest } => {
            if !hex(membership_digest, 64) {
                return Err(BackendError::invalid("membership digest required"));
            }
        }
        Suite::WorkerTests { input_digest } | Suite::Generator { input_digest } => {
            if !hex(input_digest, 64) {
                return Err(BackendError::invalid(
                    "worker input protocol digest required",
                ));
            }
        }
        Suite::DaytimeChecks {
            plan_digest,
            base,
            head,
            reviewed,
        } => {
            if !hex(plan_digest, 64)
                || !hex(base, 40)
                || !hex(head, 40)
                || reviewed.as_ref().is_some_and(|reviewed| !hex(reviewed, 40))
            {
                return Err(BackendError::invalid(
                    "daytime plan/base/head and optional reviewed identity required",
                ));
            }
        }
    }
    if request.environment.len() > 64
        || !request.environment.contains_key("PATH")
        || request.runner.arguments.len() > 128
    {
        return Err(BackendError::invalid(
            "explicit bounded job environment and runner argv required",
        ));
    }
    validate_request_environment(&request.environment)?;
    if request.pass_flux_uri
        && !request
            .repo
            .rsplit('/')
            .next()
            .is_some_and(|name| name.eq_ignore_ascii_case("prismaquant"))
    {
        return Err(BackendError::invalid(
            "pass_flux_uri is supported only for PrismaQuant",
        ));
    }

    if request.runner.arguments.iter().any(|s| {
        s.len() > 16_384 || s.chars().any(char::is_control) || safe(s).map_or(true, |v| v != *s)
    }) {
        return Err(BackendError::invalid("invalid trusted-runner arguments"));
    }
    if let Some(cleanup) = &request.runtime.cleanup
        && (!absolute(&cleanup.executable)
            || !cleanup.executable.starts_with("/mnt/shared")
            || !hex(&cleanup.executable_digest, 64)
            || cleanup.arguments.len() > 128
            || cleanup.arguments.iter().any(|argument| {
                argument.len() > 4096 || safe(argument).map_or(true, |value| value != *argument)
            }))
    {
        return Err(BackendError::invalid(
            "independent cleanup must be an immutable trusted shared executable",
        ));
    }
    match &request.runtime.runtime {
        ExecutionRuntime::Native { interpreter } => {
            if !absolute(interpreter) {
                return Err(BackendError::invalid(
                    "explicit native interpreter required",
                ));
            }
        }
        ExecutionRuntime::RootlessPodman {
            mode,
            executable,
            image,
            trusted_root,
            interpreter,
            memory_limit_bytes,
        } => {
            let digest = image.rsplit_once("sha256:").map(|(_, digest)| digest);
            if !absolute(executable)
                || !absolute(trusted_root)
                || !absolute(interpreter)
                || !trusted_root.starts_with("/mnt/shared")
                || !digest.is_some_and(|digest| hex(digest, 64))
                || request.runtime.expected_image.as_ref() != Some(image)
                || request.environment.get("HOME").map(String::as_str) != Some("/home/rob")
                || request
                    .environment
                    .get("XDG_RUNTIME_DIR")
                    .map(String::as_str)
                    != Some("/run/user/1000")
                || request.runtime.image.is_none()
                || request.runtime.cleanup.is_none()
            {
                return Err(BackendError::invalid(
                    "rootless Podman requires pinned image, explicit executable/interpreter and shared trusted inputs",
                ));
            }
            let Some(host) = &request.runtime.host_dependency else {
                return Err(BackendError::invalid(
                    "rootless image/architecture availability requires a justified eligible host",
                ));
            };
            if !token(&host.host)
                || host.reason.is_empty()
                || host.reason.len() > 4096
                || safe(&host.reason)? != host.reason
            {
                return Err(BackendError::invalid(
                    "invalid image/runtime host dependency",
                ));
            }
            if (*mode == ContainerMode::OwnScope) != memory_limit_bytes.is_some()
                || memory_limit_bytes.is_some_and(|limit| limit != request.reservation.memory_bytes)
            {
                return Err(BackendError::invalid(
                    "own_scope container cap must equal declared reservation memory; in_job does not infer enforcement",
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn placement(placement: &Placement) -> Result<()> {
    if placement.architecture != "x86_64"
        || !placement.host_dependency.as_ref().is_some_and(|host| {
            host.host == FIRST_CUT_HOST && !host.reason.is_empty() && host.reason.len() <= 4096
        })
    {
        return Err(BackendError::invalid(
            "first-cut TL3 CPU runtime requires dl380g10/x86_64 with recorded image dependency",
        ));
    }
    Ok(())
}

pub(super) fn preflight(request: &Request) -> Result<()> {
    trusted_artifacts(request)?;
    placement(&request.runtime.placement())?;
    if request.reservation.memory_guarantee == MemoryGuarantee::RequireAdmission {
        return Err(BackendError::invalid(
            "native request-derived memory admission is unqualified",
        ));
    }
    if request.reservation.gpus > 0 {
        return Err(BackendError::invalid(
            "GPU P-core resource-template and CDI capability not provisioned to this adapter",
        ));
    }
    if !matches!(request.runtime.architecture.as_str(), "x86_64" | "aarch64") {
        return Err(BackendError::invalid(
            "explicit supported runtime architecture required",
        ));
    }
    runtime_preflight(&request.runtime)?;
    for (path, expected) in [
        (&request.tree.manifest, &request.tree.input_digest),
        (
            &request.runner.executable,
            &request.runner.executable_digest,
        ),
        (&request.runner.manifest, &request.runner.manifest_digest),
        (&request.runtime.manifest, &request.runtime.manifest_digest),
    ] {
        if !std::fs::canonicalize(path)?.starts_with("/mnt/shared")
            || digest(&read(path)?) != *expected
            || !std::fs::metadata(path)?.permissions().readonly()
        {
            return Err(BackendError::invalid(
                "immutable input/runner/runtime digest or read-only permission mismatch",
            ));
        }
    }
    let manifest: InputManifest = serde_json::from_slice(&read(&request.tree.manifest)?)?;
    if manifest.schema != "tl3.input_manifest.v1"
        || manifest.head != request.tree.head
        || manifest.tree != request.tree.tree
        || manifest.files.len() > 100_000
    {
        return Err(BackendError::invalid(
            "exact-tree manifest identity mismatch",
        ));
    }
    let root = std::fs::canonicalize(&request.tree.root)?;
    let mut paths = BTreeSet::new();
    for file in manifest.files {
        if file.path.is_absolute()
            || file
                .path
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
            || !paths.insert(file.path.clone())
        {
            return Err(BackendError::invalid(
                "invalid or duplicate input manifest path",
            ));
        }
        let path = root.join(file.path);
        if !std::fs::canonicalize(&path)?.starts_with(&root)
            || digest(&read(&path)?) != file.sha256
            || !std::fs::metadata(&path)?.permissions().readonly()
        {
            return Err(BackendError::invalid(
                "staged file digest/ownership mismatch",
            ));
        }
    }
    let git = |args: &[&str]| -> Result<Vec<u8>> {
        let output = std::process::Command::new("/usr/bin/git")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .arg("-C")
            .arg(&root)
            .args(args)
            .output()?;
        if !output.status.success() || output.stdout.len() > MAX_BYTES {
            return Err(BackendError::invalid("staged Git identity unavailable"));
        }
        Ok(output.stdout)
    };
    if git(&["rev-parse", "HEAD"])?.strip_suffix(b"\n") != Some(request.tree.head.as_bytes())
        || git(&["rev-parse", "HEAD^{tree}"])?.strip_suffix(b"\n")
            != Some(request.tree.tree.as_bytes())
        || !git(&["status", "--porcelain", "--untracked-files=all"])?.is_empty()
    {
        return Err(BackendError::invalid(
            "staged checkout is dirty or differs from exact head/tree",
        ));
    }
    let tracked = git(&["ls-files", "-z"])?;
    let tracked: BTreeSet<PathBuf> = tracked
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| {
            std::str::from_utf8(s)
                .map(PathBuf::from)
                .map_err(|_| BackendError::invalid("non-UTF8 tracked path"))
        })
        .collect::<Result<_>>()?;
    if tracked != paths {
        return Err(BackendError::invalid(
            "input manifest does not cover the exact tracked tree",
        ));
    }
    Ok(())
}

pub(super) fn trusted_artifacts(request: &Request) -> Result<()> {
    if let ExecutionRuntime::RootlessPodman {
        trusted_root,
        interpreter,
        executable,
        ..
    } = &request.runtime.runtime
    {
        let (manifest, reference) =
            crate::run_backend::prepared::read_bundle(&request.runner.manifest)?;
        let root = request
            .runner
            .manifest
            .parent()
            .ok_or_else(|| BackendError::invalid("bundle parent missing"))?;
        let cleanup = request
            .runtime
            .cleanup
            .as_ref()
            .ok_or_else(|| BackendError::invalid("trusted cleanup missing"))?;
        if request.runner.manifest != request.runtime.manifest
            || reference.sha256 != request.runner.manifest_digest
            || reference.sha256 != request.runtime.manifest_digest
            || root.join(manifest.collector) != request.runner.executable
            || root.join(manifest.cleanup) != cleanup.executable
            || root.join(manifest.trusted_root) != *trusted_root
            || manifest.container_interpreter != *interpreter
            || manifest.podman != *executable
            || manifest.cleanup_arguments != cleanup.arguments
            || request.runtime.preflight.path != root.join("preflight.json")
        {
            return Err(BackendError::invalid(
                "runtime/collector/cleanup imports differ from sealed bundle",
            ));
        }
    }
    Ok(())
}

/// Same offline image/mode/cleanup checks used by request admission.
pub(super) fn runtime_preflight(runtime: &RuntimeIdentity) -> Result<()> {
    let preflight: RuntimePreflight =
        serde_json::from_slice(&checked_blob(&runtime.preflight, Path::new("/mnt/shared"))?)?;
    if preflight.schema != "tl3.runtime_preflight.v1"
        || preflight.architecture != runtime.architecture
        || preflight.image_identity != runtime.expected_image
        || !preflight.identity_verified
        || preflight.image != runtime.image
        || (matches!(runtime.runtime, ExecutionRuntime::RootlessPodman { .. })
            && !preflight.image_present)
    {
        return Err(BackendError::invalid(
            "runtime/image identity or architecture is missing or unqualified",
        ));
    }
    if let Some(cleanup) = &runtime.cleanup
        && (digest(&read(&cleanup.executable)?) != cleanup.executable_digest
            || !std::fs::metadata(&cleanup.executable)?
                .permissions()
                .readonly())
    {
        return Err(BackendError::invalid(
            "trusted cleanup executable changed or is mutable",
        ));
    }
    if let ExecutionRuntime::RootlessPodman { mode, .. } = &runtime.runtime {
        if preflight.mode != Some(*mode)
            || match mode {
                ContainerMode::InJob => {
                    !preflight.no_cgroups_supported || !preflight.init_available
                }
                ContainerMode::OwnScope => !preflight.user_scope_supported,
            }
        {
            return Err(BackendError::invalid(
                "selected container mode prerequisites are missing; no fallback",
            ));
        }
        let image = runtime
            .image
            .as_ref()
            .ok_or_else(|| BackendError::invalid("prepared image identity missing"))?;
        if runtime.expected_image.as_ref() != Some(&image.reference)
            || image.platform != "linux/amd64"
            || !image.reference.ends_with(&format!(
                "@{}",
                image
                    .index_digest
                    .as_ref()
                    .unwrap_or(&image.manifest_digest)
            ))
            || [&image.manifest_digest, &image.config_digest]
                .iter()
                .any(|pin| {
                    !pin.strip_prefix("sha256:")
                        .is_some_and(|hash| hex(hash, 64))
                })
        {
            return Err(BackendError::invalid(
                "image index/manifest/config/platform identity mismatch",
            ));
        }
        let inventory: serde_json::Value =
            serde_json::from_slice(&checked_blob(&image.inventory, Path::new("/mnt/shared"))?)?;
        let profiles = inventory
            .get("profiles")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| BackendError::invalid("ops inventory profiles unavailable"))?;
        let matches: Vec<_> = profiles
            .iter()
            .filter(|profile| {
                profile.get("profile").and_then(serde_json::Value::as_str)
                    == Some(image.profile.as_str())
            })
            .collect();
        if matches.len() != 1
            || [
                ("registry_ref", &image.reference),
                ("manifest_digest", &image.manifest_digest),
                ("config_digest", &image.config_digest),
                ("platform", &image.platform),
            ]
            .iter()
            .any(|(key, expected)| {
                matches[0].get(*key).and_then(serde_json::Value::as_str) != Some(expected.as_str())
            })
        {
            return Err(BackendError::invalid(
                "prepared image identity differs from digest-checked ops inventory",
            ));
        }
    }
    if let ExecutionRuntime::RootlessPodman { trusted_root, .. } = &runtime.runtime
        && !trusted_root.join("runner.py").is_file()
    {
        return Err(BackendError::invalid(
            "trusted_root must directly contain runner.py",
        ));
    }
    Ok(())
}
