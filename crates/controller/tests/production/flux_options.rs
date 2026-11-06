use super::*;
use tl3_controller::{Ownership, run_backend::Options};
use tl3_store::Store;

fn flux(root: &Path) -> tl3_config::Flux {
    tl3_config::Flux {
        instance_generation: "fv1-model/61f47a7f95af46d794ca89c3a71f0654".into(),
        executable: "/opt/flux/bin/flux".into(),
        backend_cli: "/opt/tl3/bin/tl3-run-backend".into(),
        container_mode: tl3_config::ContainerMode::OwnScope,
        uri: "local:///run/flux/local-0".into(),
        environment: BTreeMap::from([("PATH".into(), "/opt/flux/bin:/usr/bin:/bin".into()), ("HOME".into(), "/home/rob".into()), ("XDG_RUNTIME_DIR".into(), "/run/user/1000".into())]),
        staging_root: root.join("inputs"),
        output_root: root.join("outputs"),
        container_image: format!("dl380g10:5000/tl3/harness@sha256:{}", "a".repeat(64)),
    }
}

/// FX: one provisioned access profile is shared by controller and host CLI.
#[test]
fn bootstrap_preserves_uri_generation_and_explicit_environment() {
    let root = tempfile::tempdir().unwrap();
    let config = flux(root.path());
    let options = production::flux_options(&config);
    assert_eq!(options.uri, config.uri);
    assert_eq!(options.instance_generation, config.instance_generation);
    assert_eq!(options.executable, config.executable);
    assert_eq!(options.bootstrap.cli, config.backend_cli);
    assert_eq!(options.bootstrap.config, config.output_root.join(".run-backend/options.json"));
    assert_eq!(options.journal_root, config.output_root.join(".run-backend"));
    assert!(options.arguments.is_empty());
    let mut expected = config.environment.clone();
    expected.insert("FLUX_URI".into(), config.uri.clone());
    assert_eq!(options.environment, expected);
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
    assert_eq!(serde_json::to_value(&retained).unwrap(), serde_json::to_value(&options).unwrap());
    assert_eq!(fs::metadata(&options.bootstrap.config).unwrap().permissions().mode() & 0o777, 0o600);
    // Repeat publication preserves the exact shared profile.
    production::provision_flux_options(&config, &mut ownership).unwrap();
    assert_eq!(fs::read(&options.bootstrap.config).unwrap(), bytes);

    let outside = root.path().join("outside");
    fs::create_dir(&outside).unwrap();
    let linked = root.path().join("linked");
    std::os::unix::fs::symlink(&outside, &linked).unwrap();
    let mut changed = config.clone();
    changed.output_root = linked;
    assert!(production::provision_flux_options(&changed, &mut ownership).is_err());
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);

    startup::terminate_claim(&settings);
    let successor = Ownership::claim(Store::open(&settings).unwrap()).unwrap();
    changed.output_root = config.output_root;
    changed.uri = "local:///other/run/local-0".into();
    assert!(production::provision_flux_options(&changed, &mut ownership).is_err());
    assert_eq!(fs::read(&options.bootstrap.config).unwrap(), bytes);
    drop((successor, ownership));
    database.stop();
}
