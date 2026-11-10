use super::*;
use tl3_controller::{Ownership, run_backend::Options};
use tl3_store::Store;

pub(super) fn flux(root: &Path) -> tl3_config::Flux {
    let instance_file = root.join("INSTANCE");
    let resolver = root.join("flux-uri.sh");
    if !instance_file.exists() {
        fs::write(&instance_file, "local:///fixture\n").unwrap();
    }
    if !resolver.exists() {
        fs::write(
            &resolver,
            "#!/bin/sh\nexec cat \"$(dirname \"$0\")/INSTANCE\"\n",
        )
        .unwrap();
        fs::set_permissions(&resolver, fs::Permissions::from_mode(0o500)).unwrap();
    }
    let cli = root.join("mock-flux-cli");
    if !cli.exists() {
        fs::write(&cli, "#!/bin/sh\nexit 97\n").unwrap();
        fs::set_permissions(&cli, fs::Permissions::from_mode(0o500)).unwrap();
    }
    tl3_config::Flux {
        activation: None,
        instance_file,

        executable: cli.clone(),
        backend_cli: cli,
        container_mode: tl3_config::ContainerMode::OwnScope,
        runtime_bundles: vec![
            format!(
                "/mnt/shared/flux-out/tl3/.runtime/{}/manifest.json",
                "0".repeat(64)
            )
            .into(),
        ],
        environment: BTreeMap::from([
            ("PATH".into(), format!("{}:/usr/bin:/bin", root.display())),
            ("HOME".into(), root.to_str().unwrap().into()),
            ("XDG_RUNTIME_DIR".into(), root.to_str().unwrap().into()),
        ]),
        staging_root: root.join("inputs"),
        output_root: root.join("outputs"),
        container_image: format!("fixture.invalid/tl3/harness@sha256:{}", "a".repeat(64)),
    }
}

/// FX: one provisioned access profile is shared by controller and host CLI.
#[test]
fn bootstrap_preserves_uri_generation_and_explicit_environment() {
    let root = tempfile::tempdir().unwrap();
    let config = flux(root.path());
    let options = production::flux_options(&config);
    assert_eq!(options.instance_file, config.instance_file);
    assert_eq!(options.executable, config.executable);
    assert_eq!(options.bootstrap.cli, config.backend_cli);
    assert_eq!(
        options.bootstrap.config,
        config.output_root.join(".run-backend/options.json")
    );
    assert_eq!(
        options.journal_root,
        config.output_root.join(".run-backend")
    );
    assert!(options.arguments.is_empty());
    let expected = config.environment.clone();
    assert_eq!(options.environment, expected);
    assert_eq!(options.resolved_uri().unwrap(), "local:///fixture");
}

#[test]
fn fixture_resolver_preserves_product_uri_validation() {
    let root = tempfile::tempdir().unwrap();
    let options = production::flux_options(&flux(root.path()));
    for uri in [
        "local:///fixture",
        "local:///fixture\n",
        "local:///fixture-1/a_b.c\n",
    ] {
        fs::write(&options.instance_file, uri).unwrap();
        assert_eq!(options.resolved_uri().unwrap(), uri.trim_end_matches('\n'));
    }
    for output in [
        b"".as_slice(),
        b"local://relative",
        b"local:///",
        b"local:///fixture/",
        b"local:///fixture//socket",
        b"local:///fixture/./socket",
        b"local:///fixture/../socket",
        b"local:///fixture socket",
        b"local:///fixture\n\n",
        b"ssh:///fixture",
        b"\xff",
    ] {
        fs::write(&options.instance_file, output).unwrap();
        let error = options.resolved_uri().unwrap_err();
        assert!(matches!(
            error.code,
            tl3_controller::run_backend::ErrorCode::InvalidRequest
        ));
        assert_eq!(error.message, "invalid runtime Flux URI; value withheld");
    }
    let maximum = format!("local:///{}", "a".repeat(4087));
    fs::write(&options.instance_file, &maximum).unwrap();
    assert_eq!(options.resolved_uri().unwrap(), maximum);
    let reopened = production::flux_options(&flux(root.path()));
    assert_eq!(reopened.resolved_uri().unwrap(), maximum);
    fs::write(&options.instance_file, vec![b'a'; 4097]).unwrap();
    assert_eq!(
        options.resolved_uri().unwrap_err().message,
        "runtime Flux URI resolver refused; value withheld"
    );
    let resolver = options.instance_file.with_file_name("flux-uri.sh");
    fs::set_permissions(&resolver, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(
        &resolver,
        "#!/bin/sh\nprintf 'local:///fixture\\n'\nexit 1\n",
    )
    .unwrap();
    assert_eq!(
        options.resolved_uri().unwrap_err().message,
        "runtime Flux URI resolver refused; value withheld"
    );
}

/// #785 / FX: access settings are private, durable, and fenced by the writer claim.
#[test]
fn bootstrap_publication_is_private_and_refuses_a_lost_claim() {
    let root = tempfile::tempdir().unwrap();
    let config = flux(root.path());
    let mut database = support::Database::start();
    let settings = database.create("flux_bootstrap");
    let mut ownership = Ownership::claim(Store::open(&settings).unwrap()).unwrap();
    let options = production::provision_flux_options(&config, &mut ownership).unwrap();
    let bytes = fs::read(&options.bootstrap.config).unwrap();
    let retained: Options = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        serde_json::to_value(&retained).unwrap(),
        serde_json::to_value(&options).unwrap()
    );
    assert_eq!(
        fs::metadata(&options.bootstrap.config)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let environment_path = options.journal_root.join("job-environment.json");
    let environment: BTreeMap<String, String> =
        serde_json::from_slice(&fs::read(&environment_path).unwrap()).unwrap();
    assert_eq!(environment, production::flux_job_environment());
    assert_eq!(
        environment,
        BTreeMap::from([
            ("HOME".into(), "/home/rob".into()),
            ("XDG_RUNTIME_DIR".into(), "/run/user/1000".into()),
            ("PATH".into(), "/usr/bin:/bin".into())
        ])
    );
    assert_ne!(environment.get("PATH"), config.environment.get("PATH"));
    assert_eq!(
        fs::metadata(&environment_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    // Repeat publication preserves the exact shared profile.
    production::provision_flux_options(&config, &mut ownership).unwrap();
    assert_eq!(fs::read(&options.bootstrap.config).unwrap(), bytes);

    let outside = root.path().join("outside");
    fs::create_dir(&outside).unwrap();
    let linked = root.path().join("linked");
    std::os::unix::fs::symlink(&outside, &linked).unwrap();
    let mut changed = config.clone();
    changed.output_root = linked;
    assert_eq!(
        production::provision_flux_options(&changed, &mut ownership)
            .unwrap_err()
            .to_string(),
        "Flux bootstrap directories must be real directories"
    );
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);

    startup::terminate_claim(&settings);
    let successor = Ownership::claim(Store::open(&settings).unwrap()).unwrap();
    changed.output_root = config.output_root;
    changed.instance_file = root.path().join("replacement-INSTANCE");
    fs::write(&changed.instance_file, "local:///replacement\n").unwrap();
    assert_eq!(
        production::provision_flux_options(&changed, &mut ownership)
            .unwrap_err()
            .to_string(),
        "controller ownership session was lost; abort startup"
    );
    assert_eq!(fs::read(&options.bootstrap.config).unwrap(), bytes);
    drop((successor, ownership));
    database.stop();
}
