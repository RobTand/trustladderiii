use super::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;

pub(crate) struct Fixture {
    pub root: tempfile::TempDir,
    pub flux: tl3_config::Flux,
    pub record: DeploymentRecord,
    pub input: backend::PrepareBundleInput,
    bundles: Vec<PathBuf>,
}

impl Fixture {
    pub(crate) fn new() -> Self {
        let root = tempfile::Builder::new()
            .prefix("fx-activation-test-")
            .tempdir_in("/mnt/shared")
            .unwrap();
        let source = root.path().join("source");
        let checkout = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let fixtures = checkout.join("crates/controller/qualification/fx-exact42b/fixtures");
        let qualification: Qualification = serde_json::from_slice(PIN).unwrap();
        for file in &qualification.files {
            let path = source.join(&file.path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let name = file.path.file_name().unwrap();
            let original = if fixtures.join(name).is_file() {
                fixtures.join(name)
            } else {
                checkout.join("harness/src/tl3_harness").join(name)
            };
            let raw = fs::read(original).unwrap();
            assert_eq!(
                durable::digest(&raw),
                file.sha256,
                "qualified source {} changed",
                file.path.display()
            );
            fs::write(path, raw).unwrap();
        }
        let image = format!("fixture.invalid/activation@sha256:{}", "a".repeat(64));
        let config_digest = format!("sha256:{}", "b".repeat(64));
        let inventory = root.path().join("image-map.json");
        fs::write(&inventory, serde_json::to_vec(&serde_json::json!({"profiles":[{
            "profile":"activation-fixture", "registry_ref":image, "manifest_digest":format!("sha256:{}", "a".repeat(64)),
            "config_digest":config_digest,"platform":"linux/amd64"
        }]})).unwrap()).unwrap();
        let inspect = root.path().join("image-inspect.json");
        fs::write(
            &inspect,
            serde_json::to_vec(
                &serde_json::json!({"Id":config_digest,"Os":"linux","Architecture":"amd64"}),
            )
            .unwrap(),
        )
        .unwrap();
        let cli = root.path().join("synthetic-cli");
        fs::write(&cli, b"synthetic identity only; never executed").unwrap();
        let input = backend::PrepareBundleInput {
            source_root: source,
            code_inventory: "collector-manifest.json".into(),
            collector: "host-collector".into(),
            cleanup: "host-cleanup".into(),
            trusted_root: "lib/tl3_harness".into(),
            producer_commit: "f".repeat(40),
            container_interpreter: "/opt/tl3/bin/python".into(),
            cleanup_arguments: vec![],
            image_inventory: inventory,
            image_inspect: inspect,
            profile: "activation-fixture".into(),
            mode: ContainerMode::InJob,
            prerequisites: backend::ModePrerequisites {
                no_cgroups_supported: true,
                init_available: true,
                user_scope_supported: false,
            },
        };
        let bundle = backend::prepared::prepare_bundle(&input).unwrap();
        let prepared =
            backend::prepared::prepare_runtime(&bundle.path, &registration_reservation(), vec![])
                .unwrap();
        let flux = tl3_config::Flux {
            activation: Some(FluxActivation {
                record: root.path().join("activation.json"),
                sha256: String::new(),
            }),
            instance_generation: "fixture/00000000000000000000000000000000".into(),
            executable: cli.clone(),
            backend_cli: cli,
            container_mode: ContainerMode::InJob,
            runtime_bundles: vec![bundle.path.clone()],
            uri: "local:///fixture-endpoint".into(),
            environment: BTreeMap::from([
                ("PATH".into(), "/usr/bin:/bin".into()),
                ("HOME".into(), "/home/rob".into()),
                ("XDG_RUNTIME_DIR".into(), "/run/user/1000".into()),
            ]),
            staging_root: root.path().join("staging"),
            output_root: root.path().join("outputs"),
            container_image: image,
        };
        let options = crate::production::flux_options(&flux);
        let record = DeploymentRecord {
            schema: "tl3.fx_activation.v1".into(),
            qualification_sha256: qualification_sha256(),
            producer_commit: input.producer_commit.clone(),
            instance_generation_sha256: identity(&options.instance_generation).unwrap(),
            endpoint_sha256: identity(&options.uri).unwrap(),
            backend_options_sha256: identity(&options).unwrap(),
            backend_cli_sha256: executable_sha256(&options.bootstrap.cli).unwrap(),
            executable_sha256: executable_sha256(&options.executable).unwrap(),
            staging_root_sha256: identity(&flux.staging_root).unwrap(),
            container_mode: flux.container_mode,
            acceptance_scope: AcceptanceScope::PlannedFreshInit,
            profiles: vec![ProfileBinding {
                manifest: bundle.path.clone(),
                manifest_sha256: bundle.sha256,
                image: prepared.runtime.image.unwrap(),
                preflight: prepared.runtime.preflight,
                required_native_success_runs: 1.try_into().unwrap(),
                required_cleanup: CleanupRequirement::Verified,
            }],
        };
        let mut fixture = Self {
            root,
            flux,
            record,
            input,
            bundles: vec![bundle.path],
        };
        fixture.seal_record();
        fixture
    }

    pub(crate) fn seal_record(&mut self) {
        let raw = serde_json::to_vec(&self.record).unwrap();
        let pin = self.flux.activation.as_mut().unwrap();
        fs::write(&pin.record, &raw).unwrap();
        pin.sha256 = durable::digest(&raw);
    }

    fn replace_bundle(&mut self) {
        let bundle = backend::prepared::prepare_bundle(&self.input).unwrap();
        self.bundles.push(bundle.path.clone());
        let prepared =
            backend::prepared::prepare_runtime(&bundle.path, &registration_reservation(), vec![])
                .unwrap();
        self.flux.runtime_bundles = vec![bundle.path.clone()];
        self.record.profiles[0].manifest = bundle.path;
        self.record.profiles[0].manifest_sha256 = bundle.sha256;
        self.record.profiles[0].preflight = prepared.runtime.preflight;
        self.record.profiles[0].image = prepared.runtime.image.unwrap();
        self.seal_record();
    }

    fn update_code_inventory(&self, module: &str) {
        let code = self.input.source_root.join("collector-manifest.json");
        let mut inventory: backend::CodeInventory =
            serde_json::from_slice(&fs::read(&code).unwrap()).unwrap();
        let file = inventory
            .files
            .iter_mut()
            .find(|file| file.path == Path::new(module))
            .unwrap();
        let bytes = fs::read(self.input.source_root.join(module)).unwrap();
        file.bytes = bytes.len() as u64;
        file.sha256 = durable::digest(&bytes);
        fs::write(code, serde_json::to_vec(&inventory).unwrap()).unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fn writable(path: &Path) {
            if path.is_dir() {
                fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
                for child in fs::read_dir(path).unwrap() {
                    writable(&child.unwrap().path());
                }
            }
        }
        writable(self.root.path());
        for bundle in &self.bundles {
            let root = bundle.parent().unwrap();
            writable(root);
            fs::remove_dir_all(root).unwrap();
        }
    }
}

#[test]
#[ignore = "PB fixture: canonical shared bundle paths and retained qualification inputs"]
fn qualified_identity_and_exact_planned_deployment_pass_staging_boundary() {
    let fixture = Fixture::new();
    let activation = RuntimeActivation::load(&fixture.flux).unwrap();
    let reservation = registration_reservation();
    let prepared = crate::adapters::flux_checks::staging::select_runtime(
        &fixture.flux.runtime_bundles,
        &fixture.flux.container_image,
        fixture.flux.container_mode,
        &reservation,
        vec!["--fixture".into()],
    )
    .unwrap();
    activation
        .validate_prepared(&prepared, &reservation)
        .unwrap();
    assert_eq!(
        fixture.record.profiles[0]
            .required_native_success_runs
            .get(),
        1
    );
    let raw = fs::read(&fixture.flux.activation.as_ref().unwrap().record).unwrap();
    let text = String::from_utf8(raw).unwrap();
    assert!(text.contains("planned_fresh_init"));
    for private in [
        &fixture.flux.uri,
        &fixture.flux.instance_generation,
        "/usr/bin:/bin",
    ] {
        assert!(!text.contains(private));
    }
}

#[test]
#[ignore = "PB fixture: canonical shared bundle paths and retained qualification inputs"]
fn absent_wrong_hash_unknown_qualification_and_generation_refuse_with_fields_only() {
    let mut fixture = Fixture::new();
    let original = fixture.flux.activation.clone();
    fixture.flux.activation = None;
    assert_eq!(
        RuntimeActivation::load(&fixture.flux)
            .err()
            .unwrap()
            .to_string(),
        "flux.activation"
    );
    fixture.flux.activation = original;
    fixture.flux.activation.as_mut().unwrap().sha256 = "0".repeat(64);
    assert!(RuntimeActivation::load(&fixture.flux).is_err());
    fixture.seal_record();
    fixture.record.qualification_sha256 = "0".repeat(64);
    fixture.seal_record();
    assert!(RuntimeActivation::load(&fixture.flux).is_err());
    fixture.record.qualification_sha256 = qualification_sha256();
    fixture.seal_record();
    fixture.flux.instance_generation = "private-generation-do-not-print".into();
    let error = RuntimeActivation::load(&fixture.flux)
        .err()
        .unwrap()
        .to_string();
    assert!(
        error.ends_with("flux.activation.instance_generation_sha256"),
        "{error}"
    );
    assert!(!error.contains(&fixture.flux.instance_generation));
}

#[test]
#[ignore = "PB fixture: canonical shared bundle paths and retained qualification inputs"]
fn planned_scope_requires_explicit_success_and_cleanup_and_never_completed_claims() {
    let mut fixture = Fixture::new();
    let original = serde_json::to_value(&fixture.record).unwrap();
    for field in [
        "acceptance_scope",
        "required_native_success_runs",
        "required_cleanup",
    ] {
        let mut value = original.clone();
        match field {
            "acceptance_scope" => value[field] = "completed-private-claim".into(),
            "required_native_success_runs" => value["profiles"][0][field] = 0.into(),
            _ => {
                value["profiles"][0].as_object_mut().unwrap().remove(field);
            }
        }
        let raw = serde_json::to_vec(&value).unwrap();
        let pin = fixture.flux.activation.as_mut().unwrap();
        fs::write(&pin.record, &raw).unwrap();
        pin.sha256 = durable::digest(&raw);
        let error = RuntimeActivation::load(&fixture.flux)
            .err()
            .unwrap()
            .to_string();
        assert_eq!(error, "flux.activation.record");
        assert!(!error.contains("private-claim"));
    }
}

#[test]
#[ignore = "PB fixture: canonical shared bundle paths and retained qualification inputs"]
fn bundle_set_hash_profile_preflight_options_and_cli_substitution_refuse() {
    let mut fixture = Fixture::new();
    fixture.record.profiles[0].manifest_sha256 = "0".repeat(64);
    fixture.seal_record();
    assert!(RuntimeActivation::load(&fixture.flux).is_err());
    fixture.record.profiles[0].manifest_sha256 = fixture.flux.runtime_bundles[0]
        .parent()
        .unwrap()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .into();
    fixture.record.profiles[0].preflight.sha256 = "0".repeat(64);
    fixture.seal_record();
    assert!(RuntimeActivation::load(&fixture.flux).is_err());
    fixture.record.profiles[0].preflight.sha256 =
        durable::digest(&fs::read(&fixture.record.profiles[0].preflight.path).unwrap());
    fixture.seal_record();
    let mut options = crate::production::flux_options(&fixture.flux);
    let activation = RuntimeActivation::load(&fixture.flux).unwrap();
    options
        .environment
        .insert("PATH".into(), "/private/options-value".into());
    let error = activation
        .validate_options(&options)
        .unwrap_err()
        .to_string();
    assert!(error.ends_with("flux.activation.backend_options_sha256"));
    assert!(!error.contains("/private/options-value"));
    fs::write(&fixture.flux.backend_cli, b"changed CLI").unwrap();
    assert!(RuntimeActivation::load(&fixture.flux).is_err());
    fixture
        .flux
        .runtime_bundles
        .push(fixture.flux.runtime_bundles[0].clone());
    assert!(RuntimeActivation::load(&fixture.flux).is_err());
}

#[test]
#[ignore = "PB fixture: canonical shared bundle paths and retained qualification inputs"]
fn sealed_but_unqualified_full_closure_and_fault_interpreter_refuse() {
    let mut fixture = Fixture::new();
    let helper = "lib/tl3_harness/runtime_host.py";
    fs::write(
        fixture.input.source_root.join(helper),
        b"changed helper, still sealed",
    )
    .unwrap();
    fixture.update_code_inventory(helper);
    fixture.replace_bundle();
    assert!(RuntimeActivation::load(&fixture.flux).is_err());
    drop(fixture);
    let mut fixture = Fixture::new();
    fixture.input.container_interpreter = "/opt/tl3/bin/python-canary2-absent".into();
    fixture.replace_bundle();
    assert!(RuntimeActivation::load(&fixture.flux).is_err());
}

#[test]
#[ignore = "PB fixture: canonical shared bundle paths and retained qualification inputs"]
fn missing_changed_cleanup_and_extra_unlisted_module_refuse() {
    for change in ["missing", "changed", "extra"] {
        let fixture = Fixture::new();
        let root = fixture.flux.runtime_bundles[0].parent().unwrap();
        let cleanup = root.join("host-cleanup");
        match change {
            "missing" => {
                fs::set_permissions(root, fs::Permissions::from_mode(0o700)).unwrap();
                fs::remove_file(cleanup).unwrap();
            }
            "changed" => {
                fs::set_permissions(&cleanup, fs::Permissions::from_mode(0o700)).unwrap();
                fs::write(cleanup, b"changed").unwrap();
            }
            _ => {
                fs::set_permissions(root, fs::Permissions::from_mode(0o700)).unwrap();
                fs::write(root.join("extra-helper.py"), b"extra").unwrap();
            }
        }
        assert!(RuntimeActivation::load(&fixture.flux).is_err());
    }
}

#[test]
#[ignore = "PB fixture: canonical shared bundle paths and retained qualification inputs"]
fn tags_runtime_substitution_and_environment_override_cannot_grant_activation() {
    let fixture = Fixture::new();
    let activation = RuntimeActivation::load(&fixture.flux).unwrap();
    for image in [
        "fixture.invalid/activation:latest",
        "fixture.invalid/activation",
        "sha256:abc",
    ] {
        assert!(activation.validate_image(image).is_err());
    }
    let reservation = registration_reservation();
    let mut prepared =
        backend::prepared::prepare_runtime(&fixture.flux.runtime_bundles[0], &reservation, vec![])
            .unwrap();
    prepared.runtime.cleanup.as_mut().unwrap().executable_digest = "0".repeat(64);
    assert!(
        activation
            .validate_prepared(&prepared, &reservation)
            .is_err()
    );
    // The bypass attempt runs in a child, leaving this process's environment untouched.
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "adapters::flux_checks::activation::tests::environment_override_child",
            "--ignored",
            "--nocapture",
        ])
        .env("TL3_FLUX_RUNTIME_ACTIVATED", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
}

#[test]
#[ignore = "subprocess only: isolates environment override attempt"]
fn environment_override_child() {
    let mut fixture = Fixture::new();
    fixture.flux.activation = None;
    assert!(RuntimeActivation::load(&fixture.flux).is_err());
}

#[test]
#[ignore = "PB fixture: canonical shared bundle paths and retained qualification inputs"]
fn qualification_regeneration_is_byte_identical() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = std::process::Command::new("/usr/bin/python3")
        .args([
            "-I",
            root.join("tools/generate_fx_qualification.py")
                .to_str()
                .unwrap(),
            "--output",
            "/dev/stdout",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, PIN);
}

#[test]
#[ignore = "PB fixture: canonical shared bundle paths and retained qualification inputs"]
fn qualified_binding_passes_real_controller_capacity_and_startup_activation() {
    use crate::adapters::flux_checks::Preparation;
    let mut fixture = Fixture::new();
    // Match the production config contract, without writing any backend/runtime state there.
    fixture.flux.output_root = "/mnt/shared/flux-out/tl3".into();
    let options = crate::production::flux_options(&fixture.flux);
    fixture.record.backend_options_sha256 = identity(&options).unwrap();
    fixture.seal_record();
    let mut config =
        tl3_config::Config::parse(include_str!("../../../../../config/example.toml")).unwrap();
    config.execution_backend = tl3_config::ExecutionBackend::Flux;
    config.flux = Some(fixture.flux.clone());
    for repo in &mut config.github.repos {
        repo.container_image = None;
    }
    config.github.app_credentials_file = fixture.root.path().join("missing-synthetic-github.json");
    config.models.key_file = fixture.root.path().join("missing-synthetic-model-key");
    let repo = crate::git_tests::Repo::new();
    let stager = crate::adapters::flux_checks::staging::Stager::new(&config).unwrap();
    let (_, placement) = stager.capacity(&repo.delivery()).unwrap();
    assert_eq!(placement.host_dependency.unwrap().host, "dl380g10");
    let mut stager = stager;
    stager.prepare(&repo.delivery()).unwrap();
    let error = crate::production::register(&config)
        .err()
        .expect("synthetic credential absence must stop before store");
    assert!(matches!(error, Error::Config(_)), "{error}");
    fs::write(&fixture.flux.backend_cli, b"substituted after registration").unwrap();
    assert_eq!(
        stager.capacity(&repo.delivery()).unwrap_err().to_string(),
        "flux.activation.backend_cli_sha256"
    );
    assert!(stager.prepare(&repo.delivery()).is_err());
}
