//! Immutable pre-submit code bundles. No container execution or image mutation.
use super::durable::*;
use super::*;
use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

const ROOT: &str = "/mnt/shared/flux-out/tl3/.runtime";
const MAX_BUNDLE_BYTES: u64 = 64 * 1024 * 1024;

fn arguments_valid(arguments: &[String]) -> bool {
    arguments.len() <= 128
        && arguments.iter().all(|argument| {
            argument.len() <= 4096
                && !argument.chars().any(char::is_control)
                && safe(argument).is_ok_and(|value| value == *argument)
        })
}

fn relative(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)))
        && path
            .to_str()
            .is_some_and(|text| text.len() <= 4096 && !text.chars().any(char::is_control))
}

fn files(root: &Path, sealed: bool) -> Result<BTreeSet<PathBuf>> {
    fn walk(
        root: &Path,
        path: &Path,
        sealed: bool,
        depth: usize,
        result: &mut BTreeSet<PathBuf>,
    ) -> Result<()> {
        let info = std::fs::symlink_metadata(path)?;
        if depth > 32
            || info.file_type().is_symlink()
            || (sealed
                && (info.uid() != 1000
                    || info.mode() & 0o777
                        != if info.is_dir() {
                            0o500
                        } else {
                            info.mode() & 0o555
                        }))
        {
            return Err(BackendError::invalid(
                "runtime bundle path/owner/mode is not sealed",
            ));
        }
        if info.is_dir() {
            for child in std::fs::read_dir(path)? {
                walk(root, &child?.path(), sealed, depth + 1, result)?;
            }
        } else if info.is_file() && info.nlink() == 1 {
            let path = path
                .strip_prefix(root)
                .map_err(|_| BackendError::invalid("runtime file escaped root"))?
                .to_owned();
            if !relative(&path) || !result.insert(path) || result.len() > 4096 {
                return Err(BackendError::invalid("runtime inventory exceeds bounds"));
            }
        } else {
            return Err(BackendError::invalid(
                "runtime inventory requires regular single-link files",
            ));
        }
        Ok(())
    }
    let mut result = BTreeSet::new();
    walk(root, root, sealed, 0, &mut result)?;
    let mut bytes = 0u64;
    for path in &result {
        bytes = bytes
            .checked_add(std::fs::symlink_metadata(root.join(path))?.len())
            .ok_or_else(|| BackendError::invalid("runtime bundle exceeds byte bound"))?;
        if bytes > MAX_BUNDLE_BYTES {
            return Err(BackendError::invalid("runtime bundle exceeds byte bound"));
        }
    }
    Ok(result)
}

// Only removes the unpublished directory created by this invocation, including on failure.
struct PendingBundle(PathBuf);

impl Drop for PendingBundle {
    fn drop(&mut self) {
        fn writable(path: &Path) -> std::io::Result<()> {
            if !std::fs::symlink_metadata(path)?.is_dir() {
                return Ok(());
            }
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
            for child in std::fs::read_dir(path)? {
                writable(&child?.path())?;
            }
            Ok(())
        }
        if writable(&self.0).is_ok() {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

fn reference(path: &Path) -> Result<BlobRef> {
    let bytes = read(path)?;
    Ok(BlobRef {
        sha256: digest(&bytes),
        bytes: bytes.len() as u64,
        path: path.to_owned(),
    })
}

/// Validate every byte/mode and refuse traversal, symlinks, extra modules and writable publication.
pub fn read_bundle(path: &Path) -> Result<(RuntimeBundleManifest, BlobRef)> {
    if !absolute(path) {
        return Err(BackendError::invalid(
            "absolute runtime bundle manifest required",
        ));
    }
    let manifest_ref = reference(path)?;
    let root = path
        .parent()
        .ok_or_else(|| BackendError::invalid("bundle root missing"))?;
    if std::fs::canonicalize(root)? != root
        || root != Path::new(ROOT).join(&manifest_ref.sha256)
        || path.file_name().and_then(|name| name.to_str()) != Some("manifest.json")
    {
        return Err(BackendError::invalid(
            "runtime bundle content-address does not match manifest",
        ));
    }
    let manifest: RuntimeBundleManifest = serde_json::from_slice(&read(path)?)?;
    if manifest.schema != "tl3.runtime_bundle.v1"
        || manifest.owner != "rob"
        || manifest.uid != 1000
        || !hex(&manifest.producer_commit, 40)
        || manifest.interpreter != Path::new("/usr/bin/python3.14")
        || manifest.podman != Path::new("/usr/bin/podman")
        || !absolute(&manifest.container_interpreter)
        || !arguments_valid(&manifest.cleanup_arguments)
        || manifest.files.len() > 4096
    {
        return Err(BackendError::invalid(
            "runtime bundle producer/owner/interpreter mismatch",
        ));
    }
    let actual = files(root, true)?;
    let mut expected = BTreeSet::from([PathBuf::from("manifest.json")]);
    for file in &manifest.files {
        let executable = file.path == manifest.collector || file.path == manifest.cleanup;
        if !relative(&file.path)
            || !expected.insert(file.path.clone())
            || file.mode != if executable { 0o500 } else { 0o400 }
        {
            return Err(BackendError::invalid(
                "runtime inventory path/mode mismatch",
            ));
        }
        let path = root.join(&file.path);
        let info = std::fs::symlink_metadata(&path)?;
        let bytes = read(&path)?;
        if info.mode() & 0o777 != file.mode
            || bytes.len() as u64 != file.bytes
            || digest(&bytes) != file.sha256
        {
            return Err(BackendError::invalid(
                "runtime imported file bytes/mode changed",
            ));
        }
    }
    if actual != expected
        || std::fs::metadata(path)?.mode() & 0o777 != 0o400
        || !relative(&manifest.trusted_root)
        || !expected.contains(&manifest.collector)
        || !expected.contains(&manifest.cleanup)
        || !expected.contains(&manifest.trusted_root.join("runner.py"))
    {
        return Err(BackendError::invalid(
            "runtime bundle has missing/unlisted executable modules",
        ));
    }
    Ok((manifest, manifest_ref))
}

/// FXR stages raw code/inspection bytes; Rust validates, copies and seals their identity once.
pub fn prepare_bundle(input: &PrepareBundleInput) -> Result<BlobRef> {
    if !hex(&input.producer_commit, 40)
        || !absolute(&input.source_root)
        || !input.source_root.starts_with("/mnt/shared")
        || [
            &input.collector,
            &input.cleanup,
            &input.trusted_root,
            &input.code_inventory,
        ]
        .iter()
        .any(|path| !relative(path))
        || !token(&input.profile)
        || !absolute(&input.container_interpreter)
        || !arguments_valid(&input.cleanup_arguments)
    {
        return Err(BackendError::invalid(
            "invalid prepared runtime source paths/producer",
        ));
    }
    if std::fs::canonicalize(&input.source_root)? != input.source_root {
        return Err(BackendError::invalid(
            "runtime staging root cannot traverse symlinks",
        ));
    }
    let source_files = files(&input.source_root, false)?;
    for path in [&input.image_inventory, &input.image_inspect] {
        if !absolute(path) || !std::fs::canonicalize(path)?.starts_with("/mnt/shared") {
            return Err(BackendError::invalid(
                "runtime image facts must be retained under shared storage",
            ));
        }
    }
    let code: CodeInventory =
        serde_json::from_slice(&read(&input.source_root.join(&input.code_inventory))?)?;
    if code.interpreter != Path::new("/usr/bin/python3.14") {
        return Err(BackendError::invalid(
            "host runtime interpreter must be explicitly provisioned Python 3.14",
        ));
    }
    let mut contents = BTreeMap::<PathBuf, Vec<u8>>::new();
    for file in &code.files {
        if !relative(&file.path) || contents.contains_key(&file.path) {
            return Err(BackendError::invalid(
                "invalid/duplicate imported module inventory",
            ));
        }
        let raw = read(&input.source_root.join(&file.path))?;
        if raw.len() as u64 != file.bytes || digest(&raw) != file.sha256 {
            return Err(BackendError::invalid(
                "staged imported module digest mismatch",
            ));
        }
        contents.insert(file.path.clone(), raw);
    }
    for path in [&input.collector, &input.cleanup, &input.code_inventory] {
        contents.insert(path.clone(), read(&input.source_root.join(path))?);
    }
    if source_files != contents.keys().cloned().collect() {
        return Err(BackendError::invalid(
            "staged runtime contains unlisted files/modules",
        ));
    }
    let inventory = reference(&input.image_inventory)?;
    let map: serde_json::Value = serde_json::from_slice(&read(&input.image_inventory)?)?;
    let profiles = map
        .get("profiles")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| BackendError::invalid("image inventory profiles missing"))?;
    let profiles: Vec<_> = profiles
        .iter()
        .filter(|profile| {
            profile.get("profile").and_then(serde_json::Value::as_str)
                == Some(input.profile.as_str())
        })
        .collect();
    if profiles.len() != 1 {
        return Err(BackendError::invalid("image profile missing or ambiguous"));
    }
    let field = |key| {
        profiles[0]
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| BackendError::invalid("image profile identity incomplete"))
    };
    let image = ImageIdentity {
        inventory,
        profile: input.profile.clone(),
        reference: field("registry_ref")?,
        index_digest: profiles[0]
            .get("index_digest")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        manifest_digest: field("manifest_digest")?,
        config_digest: field("config_digest")?,
        platform: field("platform")?,
    };
    let inspected: serde_json::Value = serde_json::from_slice(&read(&input.image_inspect)?)?;
    let inspected = if let Some(array) = inspected.as_array() {
        if array.len() != 1 {
            return Err(BackendError::invalid("ambiguous native image inspection"));
        }
        &array[0]
    } else {
        &inspected
    };
    if inspected
        .get("Id")
        .and_then(serde_json::Value::as_str)
        .map(|id| id.strip_prefix("sha256:").unwrap_or(id))
        != image.config_digest.strip_prefix("sha256:")
        || inspected
            .get("Architecture")
            .and_then(serde_json::Value::as_str)
            != Some("amd64")
        || inspected.get("Os").and_then(serde_json::Value::as_str) != Some("linux")
        || image.platform != "linux/amd64"
    {
        return Err(BackendError::invalid(
            "native image config/platform differs from ops map",
        ));
    }
    let preflight = RuntimePreflight {
        schema: "tl3.runtime_preflight.v1".into(),
        architecture: "x86_64".into(),
        image_identity: Some(image.reference.clone()),
        image_present: true,
        identity_verified: true,
        image: Some(image),
        mode: Some(input.mode),
        no_cgroups_supported: input.prerequisites.no_cgroups_supported,
        init_available: input.prerequisites.init_available,
        user_scope_supported: input.prerequisites.user_scope_supported,
    };
    for (name, bytes) in [
        ("image-map.json", read(&input.image_inventory)?),
        ("image-inspect.json", read(&input.image_inspect)?),
        ("preflight.json", serde_json::to_vec(&preflight)?),
    ] {
        if contents.insert(name.into(), bytes).is_some() {
            return Err(BackendError::invalid("reserved runtime bundle file"));
        }
    }
    let manifest = RuntimeBundleManifest {
        schema: "tl3.runtime_bundle.v1".into(),
        owner: "rob".into(),
        uid: 1000,
        producer_commit: input.producer_commit.clone(),
        interpreter: code.interpreter,
        podman: "/usr/bin/podman".into(),
        container_interpreter: input.container_interpreter.clone(),
        cleanup_arguments: input.cleanup_arguments.clone(),
        collector: input.collector.clone(),
        cleanup: input.cleanup.clone(),
        trusted_root: input.trusted_root.clone(),
        files: contents
            .iter()
            .map(|(path, bytes)| BundleFile {
                path: path.clone(),
                sha256: digest(bytes),
                bytes: bytes.len() as u64,
                mode: if *path == input.collector || *path == input.cleanup {
                    0o500
                } else {
                    0o400
                },
            })
            .collect(),
    };
    let raw = serde_json::to_vec(&manifest)?;
    let total = contents
        .values()
        .try_fold(raw.len() as u64, |sum, bytes| {
            sum.checked_add(bytes.len() as u64)
        })
        .ok_or_else(|| BackendError::invalid("runtime bundle exceeds byte bound"))?;
    if total > MAX_BUNDLE_BYTES {
        return Err(BackendError::invalid("runtime bundle exceeds byte bound"));
    }
    let root = Path::new(ROOT);
    std::fs::create_dir_all(root)?;
    let destination = root.join(digest(&raw));
    if destination.exists() {
        return read_bundle(&destination.join("manifest.json")).map(|(_, reference)| reference);
    }
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let pending = root.join(format!(
        ".pending-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&pending)?;
    let _pending = PendingBundle(pending.clone());
    for (path, bytes) in &contents {
        let path = pending.join(path);
        std::fs::create_dir_all(
            path.parent()
                .ok_or_else(|| BackendError::invalid("bundle file parent missing"))?,
        )?;
        atomic(&path, bytes)?;
    }
    atomic(&pending.join("manifest.json"), &raw)?;
    for file in &manifest.files {
        std::fs::set_permissions(
            pending.join(&file.path),
            std::fs::Permissions::from_mode(file.mode),
        )?;
    }
    std::fs::set_permissions(
        pending.join("manifest.json"),
        std::fs::Permissions::from_mode(0o400),
    )?;
    fn seal(path: &Path) -> Result<()> {
        for child in std::fs::read_dir(path)? {
            let child = child?.path();
            if child.is_dir() {
                seal(&child)?;
            }
        }
        std::fs::File::open(path)?.sync_all()?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o500))?;
        Ok(())
    }
    seal(&pending)?;
    match std::fs::rename(&pending, &destination) {
        Ok(()) => {}
        Err(_) if destination.exists() => {
            return read_bundle(&destination.join("manifest.json")).map(|(_, reference)| reference);
        }
        Err(error) => return Err(error.into()),
    }
    std::fs::File::open(root)?.sync_all()?;
    read_bundle(&destination.join("manifest.json")).map(|(_, reference)| reference)
}

/// Reconstruct canonical identities once, including the exact dynamic container cap.
pub fn prepare_runtime(
    bundle: &Path,
    reservation: &Reservation,
    arguments: Vec<String>,
) -> Result<PreparedRuntime> {
    let (manifest, reference) = read_bundle(bundle)?;
    let root = bundle
        .parent()
        .ok_or_else(|| BackendError::invalid("bundle root missing"))?;
    let preflight_ref = self::reference(&root.join("preflight.json"))?;
    let preflight: RuntimePreflight = serde_json::from_slice(&read(&preflight_ref.path)?)?;
    let mode = preflight
        .mode
        .ok_or_else(|| BackendError::invalid("prepared mode missing"))?;
    let image = preflight
        .image
        .clone()
        .ok_or_else(|| BackendError::invalid("prepared image missing"))?;
    checked_blob(&image.inventory, Path::new("/mnt/shared"))?;
    let runner_path = root.join(&manifest.collector);
    let cleanup_path = root.join(&manifest.cleanup);
    let runner = RunnerIdentity {
        executable: runner_path.clone(),
        executable_digest: self::reference(&runner_path)?.sha256,
        arguments,
        manifest: bundle.to_owned(),
        manifest_digest: reference.sha256.clone(),
    };
    let runtime = RuntimeIdentity {
        manifest: bundle.to_owned(),
        manifest_digest: reference.sha256.clone(),
        runtime: ExecutionRuntime::RootlessPodman {
            mode,
            executable: manifest.podman,
            image: image.reference.clone(),
            trusted_root: root.join(manifest.trusted_root),
            interpreter: manifest.container_interpreter,
            memory_limit_bytes: (mode == ContainerMode::OwnScope)
                .then_some(reservation.memory_bytes),
        },
        architecture: preflight.architecture,
        expected_image: Some(image.reference.clone()),
        preflight: preflight_ref,
        host_dependency: Some(HostDependency {
            host: "dl380g10".into(),
            reason: format!(
                "ops inventory profile {} is provisioned linux/amd64 on dl380g10",
                image.profile
            ),
        }),
        image: Some(image),
        cleanup: Some(CleanupProgram {
            executable: cleanup_path.clone(),
            executable_digest: self::reference(&cleanup_path)?.sha256,
            arguments: manifest.cleanup_arguments,
        }),
    };
    Ok(PreparedRuntime {
        bundle: reference,
        runner,
        runtime,
        scratch_root: "/var/tmp/flux-vanilla".into(),
    })
}
