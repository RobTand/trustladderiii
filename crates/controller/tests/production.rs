#[path = "production/backend_watch.rs"]
mod backend_watch;
#[path = "production/flux_options.rs"]
mod flux_options;
#[path = "production/startup.rs"]
mod startup;
#[path = "../../store/tests/support/mod.rs"]
mod support;

use serde_json::json;
use std::{collections::BTreeMap, fs, os::unix::fs::PermissionsExt, path::Path};
use tl3_controller::production;

fn config(root: &Path) -> tl3_config::Config {
    let mut config = tl3_config::Config::parse(include_str!("../../config/example.toml")).unwrap();
    config.worktrees_root = root.join("worktrees");
    config.workers.runs = root.join("worker-runs");
    config.workers.python = root.join("app/workers/.venv/bin/python");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    config.github.webhook_listen = Some(listener.local_addr().unwrap());
    drop(listener);
    config.github.operations = Some(tl3_config::OperationsIssue {
        owner: "RobTand".into(),
        name: "trustladderiii".into(),
        number: 81,
        installation_id: 987,
    });
    config.github.repos[0].installation_id = 987;
    config.github.app_credentials_file = root.join("github-app.json");
    config.models.key_file = root.join("model-key");
    let pem = include_str!("../../github/tests/fixtures/throwaway-key.pem");
    fs::write(
        &config.github.app_credentials_file,
        serde_json::to_vec(
            &json!({"id":123,"slug":"fixture","pem":pem,"webhook_secret":"fixture-secret"}),
        )
        .unwrap(),
    )
    .unwrap();
    fs::write(&config.models.key_file, "fixture-model-key").unwrap();
    for path in [&config.github.app_credentials_file, &config.models.key_file] {
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    config
}

#[test]
fn fixture_credentials_are_fast_and_repeatable() {
    let root = tempfile::tempdir().unwrap();
    let started = std::time::Instant::now();
    let first = config(root.path());
    let before = fs::read(&first.github.app_credentials_file).unwrap();
    let second = config(root.path());
    let after = fs::read(&second.github.app_credentials_file).unwrap();
    assert!(
        started.elapsed() < std::time::Duration::from_secs(1),
        "fixture credentials took {:?}; key generation must not be test work",
        started.elapsed()
    );
    assert_eq!(
        before, after,
        "fixture credentials must not depend on entropy"
    );
}

#[test]
fn flux_activation_refusal_preserves_legacy_pb_evidence() {
    for suffix in ["job", "wait", "done"] {
        for node in ["", "0"] {
            let root = tempfile::tempdir().unwrap();
            let mut config = config(root.path());
            config.flux = tl3_config::Flux {
                instance_file: "/mnt/shared/flux-migration/INSTANCE".into(),
                activation: None,

                executable: "/opt/flux/bin/flux".into(),
                backend_cli: "/opt/tl3/bin/tl3-run-backend".into(),
                container_mode: tl3_config::ContainerMode::OwnScope,
                runtime_bundles: vec![
                    format!(
                        "/mnt/shared/flux-out/tl3/.runtime/{}/manifest.json",
                        "0".repeat(64)
                    )
                    .into(),
                ],
                environment: BTreeMap::from([
                    ("PATH".into(), "/usr/bin:/bin".into()),
                    ("HOME".into(), "/home/rob".into()),
                    ("XDG_RUNTIME_DIR".into(), "/run/user/1000".into()),
                ]),
                staging_root: "/mnt/shared/tl3-inputs".into(),
                output_root: "/mnt/shared/flux-out/tl3".into(),
                container_image: config.flux.container_image.clone(),
            };
            let directory = config
                .workers
                .runs
                .join(".controller/prismabuild")
                .join(node);
            fs::create_dir_all(&directory).unwrap();
            let binding = directory.join(format!("legacy.{suffix}"));
            fs::write(&binding, "legacy PB evidence, deliberately not a Flux ID").unwrap();
            let error = match production::register(&config) {
                Ok(_) => panic!("Flux accepted an absent activation record"),
                Err(error) => error,
            };
            assert!(error.to_string().contains("flux.activation"), "{error}");
            assert_eq!(
                fs::read_to_string(binding).unwrap(),
                "legacy PB evidence, deliberately not a Flux ID"
            );
        }
    }
}

/// PB-REMOVAL (D2): production startup composes Flux checks only, and refuses before the store
/// when the deployment-owned Flux activation is absent. No PrismaBuild adapter is ever built.
/// (Composing every adapter needs a sealed activation record, which only the PB-admitted
/// activation fixtures in adapters::flux_checks::activation::tests can provide.)
#[test]
fn production_refuses_to_start_without_flux_activation() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    assert!(config.flux.activation.is_none());
    let error = match production::register(&config) {
        Ok(_) => panic!("production started without a Flux activation"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("flux.activation"), "{error}");
}

#[test]
fn production_refuses_missing_and_invalid_credential_paths_before_opening_store() {
    let root = tempfile::tempdir().unwrap();
    let mut config = config(root.path());
    let original = config.models.key_file.clone();
    for field in ["model", "github"] {
        let path = root.path().join(format!("{field}-missing"));
        if field == "model" {
            config.models.key_file = path.clone();
        } else {
            config.models.key_file = original.clone();
            config.github.app_credentials_file = path.clone();
        }
        assert!(matches!(
            production::register(&config),
            Err(tl3_controller::Error::Config(_))
        ));
        fs::write(&path, "fixture").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            production::register(&config),
            Err(tl3_controller::Error::Config(_))
        ));
    }
}

#[test]
fn configured_writer_executes_the_writer_ladder_and_reuses_its_durable_reply() {
    use tl3_controller::adapters::workers::writer::{
        MessageKind, ProcessWriter, WriteRequest, Writer, WriterReply,
    };
    let root = tempfile::tempdir().unwrap();
    let mut config = config(root.path());
    config.workers.python = "/usr/bin/python3".into();
    let script = root.path().join("writer.py");
    fs::write(&script, r#"import json,os,sys
from pathlib import Path
r=json.load(sys.stdin)
assert r['role']=='writer'
assert r['model']['name']=='writer-choice'
assert r['model']['budget_tokens']==4321
assert sorted(p.name for p in Path(os.environ['CREDENTIALS_DIRECTORY']).iterdir())==['model-key']
print(json.dumps({'event':'reply','role':'writer','reply':{'text':'The requested change is delivered.'}}),flush=True)
"#).unwrap();
    config.workers.command = vec![script.to_str().unwrap().into()];
    config.models.writer[0].name = "writer-choice".into();
    config.models.writer[0].budget_tokens = 4321;
    let input = WriteRequest {
        kind: MessageKind::Summary,
        facts: json!({"issue_title":"A change","request":"Make a change","outcome":"Delivered", "unmet_goals": []}),
        violations: vec![],
        request: None,
    };
    let mut writer = ProcessWriter::new(&config, root.path()).unwrap();
    assert!(matches!(
        writer.draft("notice:1", &input).unwrap(),
        WriterReply::Text(_)
    ));
    fs::remove_file(script).unwrap();
    assert!(matches!(
        writer.draft("notice:1", &input).unwrap(),
        WriterReply::Text(_)
    ));
}

/// Encodes #785: the losing daemon must leave the owner's staged files, journals and
/// prompts intact. It runs the ownership claim and migration that production::register
/// runs after its configuration and Flux activation checks.
#[test]
fn second_daemon_cannot_destroy_the_owners_staged_inputs() {
    let root = tempfile::tempdir().unwrap();
    let mut config = config(root.path());
    let mut database = support::Database::start();
    config.store = database.create("startup_ownership");
    let start = || {
        let mut ownership =
            tl3_controller::Ownership::claim(tl3_store::Store::connect(&config.store)?)?;
        ownership.migrate()?;
        Ok::<_, tl3_controller::Error>(ownership)
    };
    let mut owner = start().unwrap();
    // The owner's first event: seeded rows reference a committed revision.
    let snapshot = owner.store().load().unwrap();
    owner
        .store()
        .commit(&tl3_store::Commit::new(
            "owner-opened",
            json!({}),
            &snapshot,
        ))
        .unwrap();
    let mut client = startup::raw(&config.store);
    let shared_paths = startup::seed_shared_state(&config, &mut client);
    let before_files: Vec<_> = shared_paths.iter().map(|p| fs::read(p).ok()).collect();
    let before_prompts = startup::prompt_rows(&mut client);
    let loser = start();
    assert_eq!(startup::prompt_rows(&mut client), before_prompts);
    let after_files: Vec<_> = shared_paths.iter().map(|p| fs::read(p).ok()).collect();
    assert_eq!(
        after_files, before_files,
        "loser changed completion, job, cancellation, or journal state"
    );
    drop(client);
    assert!(
        matches!(loser, Err(ref error) if error.to_string().contains("controller ownership is still held"))
    );
    drop(owner);
    database.stop();
}

#[path = "production/flux_checks.rs"]
mod flux_checks;

/// FX: explicit Flux never enters the unselected PB construction path.
#[test]
fn selected_flux_refuses_unprovisioned_runtime_without_pb_fallback() {
    let root = tempfile::tempdir().unwrap();
    let mut config = config(root.path());
    let mut flux = flux_options::flux(root.path());
    flux.staging_root = "/mnt/shared/tl3-inputs".into();
    flux.output_root = "/mnt/shared/flux-out/tl3".into();
    config.flux = flux;
    let error = match production::register(&config) {
        Ok(_) => panic!("unprovisioned Flux selection entered a backend"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("flux.activation"), "{error}");
}

/// FX: actual production composition selects Flux without dereferencing absent PB settings.
#[test]
fn selected_flux_registration_refuses_absent_activation_without_pb_or_native_launch() {
    let root = tempfile::tempdir().unwrap();
    let mut config = config(root.path());
    config.flux = flux_options::flux(root.path());
    let mut database = support::Database::start();
    let settings = database.create("flux_construction");
    let mut ownership =
        tl3_controller::Ownership::claim(tl3_store::Store::open(&settings).unwrap()).unwrap();
    let mut registry = tl3_controller::executor::Registry::default();
    let result = production::register_execution(
        &config,
        &root.path().join("records"),
        &mut registry,
        &mut ownership,
    );
    assert!(matches!(result,Err(ref error) if error.to_string() == "flux.activation"));
    assert!(!root.path().join("records/prismabuild").exists());
    drop((registry, ownership));
    database.stop();
}
