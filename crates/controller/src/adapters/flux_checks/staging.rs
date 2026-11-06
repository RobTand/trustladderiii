//! Immutable controller input construction, shared with the existing trusted check payload.
use super::*;
use crate::adapters::prismabuild::checks;
use std::os::unix::fs::PermissionsExt;
use tl3_worktrees::Worktrees;

pub struct PreparedInput {
    pub tree: backend::TreeIdentity,
    pub run: CheckRun,
    pub changed_paths: Vec<String>,
    pub selected_tests: Option<u32>,
    pub always_tests: Vec<String>,
}

/// `root` is a fresh private shared destination, outside the served checkout.
/// The trusted harness comes from the provisioned immutable release, never the served tree.
pub fn stage_input(
    delivery: &Delivery,
    root: &Path,
    trusted_harness: &Path,
    image: &str,
    protected_paths: &[String],
    generated_paths: &[String],
    selected_tests: Option<u32>,
    always_tests: &[String],
) -> Result<PreparedInput> {
    if !backend::absolute(root) {
        return Err(Error::Invalid("Flux input requires a new absolute destination"));
    }
    // The shared stage function supports PB replacement. Flux publication never replaces
    // a retained input, including a partial publication or a dangling symlink.
    let run = checks::descended_base(delivery)?;
    let served = run.repository.canonicalize()?;
    let parent = root.parent().ok_or(Error::Invalid("input requires a parent directory"))?;
    if parent.canonicalize()?.starts_with(&served) || trusted_harness.canonicalize()?.starts_with(&served) {
        return Err(Error::Invalid("trusted Flux inputs must stay outside the served checkout"));
    }
    for ancestor in root.ancestors() {
        if ancestor.is_symlink() { return Err(Error::Invalid("Flux input ancestors cannot be symlinks")); }
    }
    fs::create_dir(root)?;
    fs::set_permissions(root, fs::Permissions::from_mode(0o700))?;
    let tree_root = root.join("tree");
    let mut staged = delivery.clone();
    staged.checks = Some(run.clone());
    let tests = crate::adapters::images::AdditionalTests { selected: selected_tests, always: always_tests.to_vec(), generated: generated_paths.to_vec() };
    let changed_paths = checks::stage(&tree_root, trusted_harness, &staged, image, protected_paths, &tests)?;
    let repository = Worktrees::open(&tree_root, &tree_root)?;
    let head = repository.head()?;
    let tree = repository.tree(&head)?;
    let mut files = Vec::new();
    inventory(&tree_root, &tree_root, &mut files)?;
    files.sort_by(|left, right| left.path.cmp(&right.path));
    let manifest = backend::InputManifest { schema: "tl3.input_manifest.v1".into(), head: head.as_str().into(), tree: tree.clone(), files };
    let bytes = serde_json::to_vec(&manifest)?;
    if bytes.len() > durable::MAX_BYTES { return Err(Error::Invalid("Flux input manifest exceeds byte bound")); }
    let manifest_path = root.join("input-manifest.json");
    durable::atomic(&manifest_path, &bytes).map_err(adapter_error)?;
    fs::set_permissions(&manifest_path, fs::Permissions::from_mode(0o400))?;
    fs::set_permissions(root, fs::Permissions::from_mode(0o500))?;
    File::open(root)?.sync_all()?;
    if let Some(parent) = root.parent() { File::open(parent)?.sync_all()?; }
    let mut seen = std::collections::BTreeSet::new();
    Ok(PreparedInput {
        tree: backend::TreeIdentity { head: head.as_str().into(), tree, input_digest: durable::digest(&bytes), root: tree_root, manifest: manifest_path },
        run, changed_paths, selected_tests,
        always_tests: always_tests.iter().filter(|test| seen.insert((*test).clone())).cloned().collect(),
    })
}

fn inventory(root: &Path, directory: &Path, files: &mut Vec<backend::InputFile>) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if directory == root && entry.file_name() == ".git" { continue; }
        let kind = entry.file_type()?;
        if kind.is_dir() {
            inventory(root, &path, files)?;
        } else if kind.is_file() {
            if files.len() >= 100_000 { return Err(Error::Invalid("Flux input inventory exceeds file bound")); }
            let bytes = durable::read(&path).map_err(adapter_error)?;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o400))?;
            File::open(&path)?.sync_all()?;
            files.push(backend::InputFile { path: path.strip_prefix(root).map_err(|_| Error::Invalid("input outside staged root"))?.to_owned(), sha256: durable::digest(&bytes) });
        } else {
            return Err(Error::Invalid("Flux inputs must contain regular files, without symlinks"));
        }
    }
    fs::set_permissions(directory, fs::Permissions::from_mode(0o500))?;
    File::open(directory)?.sync_all()?;
    Ok(())
}

/// Select exactly one already-prepared image/mode identity. The shared producer owns
/// inventory resolution, original map provenance and all bundle byte/mode checks.
pub fn select_runtime(
    bundles: &[PathBuf],
    image: &str,
    mode: tl3_config::ContainerMode,
    reservation: &backend::Reservation,
    arguments: Vec<String>,
) -> Result<backend::PreparedRuntime> {
    let mut selected = None;
    for bundle in bundles {
        let prepared = backend::prepared::prepare_runtime(bundle, reservation, arguments.clone()).map_err(adapter_error)?;
        let Some(identity) = &prepared.runtime.image else { return Err(Error::Invalid("prepared runtime has no immutable image identity")); };
        if image != identity.reference && image != identity.config_digest { continue; }
        if !matches!(&prepared.runtime.runtime, backend::ExecutionRuntime::RootlessPodman { mode: prepared_mode, .. } if *prepared_mode == mode) { continue; }
        if selected.replace(prepared).is_some() {
            return Err(Error::Invalid("configured image and mode match multiple immutable runtime bundles"));
        }
    }
    selected.ok_or(Error::Invalid("configured immutable image and selected mode have no prepared runtime bundle"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Read-only publication is the production contract; fixture teardown restores owner write access.
    fn writable(directory: &Path) {
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).unwrap();
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() { writable(&entry.path()); }
        }
    }

    /// FX / #750: transport identity covers committed served input and the external trusted harness.
    #[test]
    fn staged_transport_preserves_served_identity_and_manifests_every_payload_file() {
        let root = tempfile::tempdir().unwrap();
        let fixture = crate::git_tests::Repo::new();
        let d = fixture.delivery();
        fs::write(fixture.path.join("README.md"), "dirty maker data").unwrap();
        let harness = root.path().join("trusted-release");
        fs::create_dir(&harness).unwrap();
        fs::write(harness.join("runner.py"), "# trusted fixture\n").unwrap();
        fs::write(harness.join("__init__.py"), "").unwrap();
        let destination = root.path().join("input");
        let result = stage_input(&d, &destination, &harness, "immutable-image", &["protected/**".into()], &["generated/**".into()], Some(3), &["tests/test_always.py".into()]);
        if result.is_err() { writable(root.path()); }
        let staged = result.unwrap();
        let manifest: backend::InputManifest = serde_json::from_slice(&fs::read(&staged.tree.manifest).unwrap()).unwrap();
        assert_eq!(manifest.head, staged.tree.head);
        assert_eq!(manifest.tree, staged.tree.tree);
        assert_ne!(staged.tree.head, d.head_sha.as_ref().unwrap().as_str());
        assert_eq!(manifest.files.iter().map(|file| file.path.to_str().unwrap()).collect::<Vec<_>>(),
            ["action.py", "request.json", "served.bundle", "trusted/tl3_harness/__init__.py", "trusted/tl3_harness/runner.py"]);
        for file in &manifest.files {
            let path = staged.tree.root.join(&file.path);
            assert!(fs::metadata(&path).unwrap().permissions().readonly());
            assert_eq!(durable::digest(&fs::read(path).unwrap()), file.sha256);
        }
        let request: serde_json::Value = serde_json::from_slice(&fs::read(staged.tree.root.join("request.json")).unwrap()).unwrap();
        assert_eq!(request["head"], fixture.head);
        assert_eq!(request["base"], fixture.base);
        assert_eq!(request["protected_paths"], serde_json::json!(["protected/**"]));
        assert_eq!(request["footprint"], serde_json::json!(d.paths));
        assert_eq!(request["generated_paths"], serde_json::json!(["generated/**"]));
        assert_eq!(request["selected_tests"], 3);
        assert_eq!(request["always_tests"], serde_json::json!(["tests/test_always.py"]));
        assert_eq!(staged.changed_paths, ["README.md"]);
        assert!(stage_input(&d, &destination, &harness, "changed-image", &[], &[], None, &[]).is_err());
        assert_eq!(durable::digest(&fs::read(&staged.tree.manifest).unwrap()), staged.tree.input_digest);
        writable(root.path());
    }
}
