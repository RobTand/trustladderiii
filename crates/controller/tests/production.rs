mod prismabuild_fixture;
#[path = "production/startup.rs"]
mod startup;
#[path = "../../store/tests/support/mod.rs"]
mod support;

use serde_json::json;
use std::{collections::BTreeMap, fs, os::unix::fs::PermissionsExt, path::Path};
use tl3_controller::{executor::Upstream, production};

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
    let pem = include_str!("../../mergify/tests/fixtures/throwaway-key.pem");
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
fn production_registers_every_real_adapter_and_source() {
    let root = tempfile::tempdir().unwrap();
    let mut config = config(root.path());
    let mut database = support::Database::start();
    config.store = database.create("production");
    let registered = production::register(&config).unwrap();
    let expected = BTreeMap::from([
        (
            Upstream::GitHub,
            "tl3_controller::adapters::github::Adapter",
        ),
        (
            Upstream::Mergify,
            "tl3_controller::adapters::mergify::Adapter",
        ),
        (
            Upstream::PrismaBuild,
            "tl3_controller::adapters::prismabuild::Adapter",
        ),
        (
            Upstream::Workers,
            "tl3_controller::adapters::workers::Supervisor",
        ),
        (
            Upstream::Worktrees,
            "tl3_controller::adapters::worktrees::Real",
        ),
    ]);
    assert_eq!(registered.adapters.registered_types(), expected);
    assert!(std::any::type_name_of_val(&registered.workers).ends_with("WorkerOutputSource"));
    assert!(std::any::type_name_of_val(&registered.bell).contains("prismabuild::source::Source"));
    assert!(
        std::any::type_name_of_val(&registered.mergify)
            .ends_with("direct_merge::events::Translator")
    );
    assert!(registered.webhooks.address().ip().is_loopback());
    drop(registered);
    database.stop();
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

/// Encodes #785: the losing daemon must leave the owner's staging and harness
/// publication intact, through the same registration/open path used by tl3d.
#[test]
fn second_daemon_cannot_destroy_the_owners_staged_inputs() {
    use tl3_controller::{Bootstrap, Controller};

    let root = tempfile::tempdir().unwrap();
    let mut config = config(root.path());
    let mut database = support::Database::start();
    config.store = database.create("startup_ownership");
    // Sources are dropped after registration, releasing the webhook listener.
    // Ownership must reject the contender. No upstream is contacted.
    let start = || {
        let registration = production::register(&config)?;
        let controller = Controller::open_owned(
            registration.ownership,
            registration.adapters,
            Bootstrap::from_config(&config)?,
        )?;
        Ok::<_, tl3_controller::Error>(controller)
    };
    let owner = start().unwrap();
    let records = config.workers.runs.join(".controller/prismabuild");
    let paths = [
        records.join("active.input/harness/check.py"),
        records.join("0/active.input/harness/check.py"),
        records
            .with_file_name("prismabuild-harness")
            .join("owner/check.py"),
    ];
    for path in &paths {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"owner's staged bytes").unwrap();
    }
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
    for path in &paths {
        assert_eq!(
            fs::read(path).ok().as_deref(),
            Some(b"owner's staged bytes".as_slice()),
            "the losing daemon damaged {}",
            path.display()
        );
    }
    assert!(
        matches!(loser, Err(ref error) if error.to_string().contains("controller ownership is still held"))
    );
    drop(owner);
    database.stop();
}
