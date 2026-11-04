use super::*;
use postgres::{Client, NoTls};
use tl3_controller::{Ownership, adapters};
use tl3_store::Store;

const CLAIM: i64 = 0x544c33434f4e5452;

pub(super) fn raw(settings: &tl3_config::Store) -> Client {
    postgres::Config::new()
        .host(&settings.host)
        .port(settings.port)
        .user(&settings.user)
        .dbname(&settings.database)
        .password("tl3-disposable-test-password")
        .connect(NoTls)
        .unwrap()
}

fn legacy(client: &mut Client) {
    let sql = include_str!("../../../store/migrations/0001_store.sql");
    client.batch_execute(sql).unwrap();
    client
        .batch_execute(
            "CREATE TABLE tl3_schema_migrations (version integer PRIMARY KEY, sql text NOT NULL)",
        )
        .unwrap();
    client
        .execute("INSERT INTO tl3_schema_migrations VALUES (1, $1)", &[&sql])
        .unwrap();
    client.batch_execute("INSERT INTO tl3_events (event_key,revision,epoch,payload,result) VALUES ('owner',1,0,'{}','{}'); INSERT INTO tl3_world VALUES ('owned','row','{\"owner\":true}',1)").unwrap();
}

fn legacy_snapshot(client: &mut Client) -> (Vec<String>, Vec<i32>) {
    (
        client
            .query(
                "SELECT row_to_json(w)::text FROM tl3_world w ORDER BY table_name,row_id",
                &[],
            )
            .unwrap()
            .iter()
            .map(|r| r.get(0))
            .collect(),
        client
            .query(
                "SELECT version FROM tl3_schema_migrations ORDER BY version",
                &[],
            )
            .unwrap()
            .iter()
            .map(|r| r.get(0))
            .collect(),
    )
}

/// #785 review: an older live owner must exclude a newer binary's data migrations.
#[test]
fn pending_migrations_cannot_change_a_live_owners_database() {
    let root = tempfile::tempdir().unwrap();
    let mut config = config(root.path());
    let mut database = support::Database::start();
    config.store = database.create_uninitialized("legacy_owner");
    let mut old_owner = raw(&config.store);
    legacy(&mut old_owner);
    assert!(
        old_owner
            .query_one("SELECT pg_try_advisory_lock($1)", &[&CLAIM])
            .unwrap()
            .get::<_, bool>(0)
    );
    let before = legacy_snapshot(&mut old_owner);
    let contender = production::register(&config);
    assert_eq!(
        legacy_snapshot(&mut old_owner),
        before,
        "a refused contender migrated the owner's data or schema"
    );
    assert!(matches!(contender, Err(ref e) if e.to_string().contains("ownership is still held")));
    drop(old_owner);
    database.stop();
}

/// #785 review: auxiliary writer sessions must reject an old schema without upgrading it.
#[test]
fn auxiliary_open_does_not_apply_pending_migrations() {
    let mut database = support::Database::start();
    let settings = database.create_uninitialized("legacy_auxiliary");
    let mut client = raw(&settings);
    legacy(&mut client);
    let before = legacy_snapshot(&mut client);
    assert!(tl3_store::Reader::open(&settings).is_err());
    assert!(tl3_store::Listener::open(&settings).is_err());
    assert!(tl3_store::ConsoleCommands::open(&settings).is_err());
    assert!(tl3_store::Prompts::open(&settings).is_err());
    let auxiliary = Store::open(&settings);
    assert_eq!(
        legacy_snapshot(&mut client),
        before,
        "an auxiliary open migrated the database"
    );
    assert!(
        auxiliary.is_err(),
        "auxiliary open must explicitly reject an old schema"
    );
    drop(auxiliary);
    drop(client);
    database.stop();
}

fn terminate_claim(settings: &tl3_config::Store) {
    let mut client = raw(settings);
    let pid: i32 = client.query_one("SELECT pid FROM pg_locks WHERE locktype='advisory' AND classid::bigint=1414280003 AND objid::bigint=1330533458 AND objsubid=1 AND database=(SELECT oid FROM pg_database WHERE datname=current_database()) AND granted", &[]).unwrap().get(0);
    assert!(
        client
            .query_one("SELECT pg_terminate_backend($1, 5000)", &[&pid])
            .unwrap()
            .get::<_, bool>(0)
    );
}

/// #785 review: a disconnected capability must not admit filesystem cleanup.
#[test]
fn lost_claim_prevents_prismabuild_cleanup() {
    let root = tempfile::tempdir().unwrap();
    let mut database = support::Database::start();
    let settings = database.create("lost_pb_claim");
    let mut ownership = Ownership::claim(Store::open(&settings).unwrap()).unwrap();
    let (client, pb) = prismabuild_fixture::settings(root.path(), "done", "http://127.0.0.1:9/");
    let staged = pb.records.join("active.input/harness/check.py");
    fs::create_dir_all(staged.parent().unwrap()).unwrap();
    fs::write(&staged, b"owner bytes").unwrap();
    terminate_claim(&settings);
    let successor = Ownership::claim(Store::open(&settings).unwrap()).unwrap();
    let result = adapters::prismabuild::Adapter::new(client, pb, &mut ownership);
    assert_eq!(
        fs::read(&staged).ok().as_deref(),
        Some(b"owner bytes".as_slice()),
        "cleanup ran after claim loss"
    );
    assert!(result.is_err(), "lost ownership must abort startup");
    drop((result, successor, ownership));
    database.stop();
}

/// #785 review: a disconnected capability must not seed or activate prompts.
#[test]
fn lost_claim_prevents_worker_initialization() {
    let root = tempfile::tempdir().unwrap();
    let mut config = config(root.path());
    let mut database = support::Database::start();
    config.store = database.create("lost_worker_claim");
    let mut ownership = Ownership::claim(Store::open(&config.store).unwrap()).unwrap();
    terminate_claim(&config.store);
    let successor = Ownership::claim(Store::open(&config.store).unwrap()).unwrap();
    let result =
        adapters::workers::Supervisor::new(&config, &root.path().join("records"), &mut ownership);
    let count: i64 = raw(&config.store)
        .query_one("SELECT count(*) FROM tl3_prompt_versions", &[])
        .unwrap()
        .get(0);
    assert_eq!(count, 0, "prompt seeding ran after claim loss");
    assert!(result.is_err(), "lost ownership must abort startup");
    drop((result, successor, ownership));
    database.stop();
}

pub(super) fn seed_shared_state(
    config: &tl3_config::Config,
    client: &mut Client,
) -> Vec<std::path::PathBuf> {
    let records = config.workers.runs.join(".controller/prismabuild");
    let delivery: tl3_controller::executor::Delivery = serde_json::from_value(json!({
        "key":"worker-owner", "epoch":1, "output":{"Comment":{"node":0,"kind":"Classify"}},
        "generation":1,"repository":{"owner":"RobTand","name":"tl3-sandbox","installation":987},
        "issue":1,"paths":[],"attempt":1,"round":0
    }))
    .unwrap();
    let job = json!({"action":"f".repeat(64),"delivery":delivery,"run":{"repository":"/fixture","base_sha":"a".repeat(40),"plan":[]},"changed_paths":[]});
    let job_path = records.join("legacy.job");
    fs::write(&job_path, serde_json::to_vec(&job).unwrap()).unwrap();
    let done = records.join("legacy.done");
    fs::write(&done, b"owner completion").unwrap();
    let orphan = records.join("orphan.done");
    fs::write(&orphan, b"owner orphan").unwrap();
    client.execute("INSERT INTO tl3_world (table_name,row_id,value,version) SELECT 'watches','worker-owner',$1,revision FROM tl3_meta", &[&serde_json::to_value(&delivery).unwrap()]).unwrap();
    client.execute("INSERT INTO tl3_effects (effect_key,epoch,kind,payload,state,created_revision,updated_revision) SELECT 'worker-owner',epoch,'Workers',$1,'Sent',revision,revision FROM tl3_meta", &[&serde_json::to_value(&delivery).unwrap()]).unwrap();
    let encoded: String = delivery.key.bytes().map(|b| format!("{b:02x}")).collect();
    let attempt = config.workers.runs.join(&encoded).join("attempt-00000000");
    fs::create_dir_all(&attempt).unwrap();
    // Both absent markers must remain absent. A constructor inspection would create them.
    let mut paths = vec![
        job_path,
        done,
        orphan,
        records.join("0").join(format!("{encoded}.job")),
        records.join("0").join(format!("{encoded}.done")),
        attempt.join("claim.json"),
        attempt.join("process.json"),
    ];
    for repo in &config.github.repos {
        let journal = config
            .worktrees_root
            .join(".repositories")
            .join(repo.owner.to_ascii_lowercase())
            .join(format!("{}.git", repo.name.to_ascii_lowercase()))
            .join("tl3-controller");
        assert!(journal.join("repository.json").exists());
        paths.extend([journal.join("repository.json"), journal.join("epoch.json")]);
    }
    paths
}

pub(super) fn prompt_rows(client: &mut Client) -> Vec<String> {
    client
        .query(
            "SELECT row_to_json(v)::text FROM tl3_prompt_versions v ORDER BY id",
            &[],
        )
        .unwrap()
        .iter()
        .map(|r| r.get(0))
        .chain(
            client
                .query(
                    "SELECT row_to_json(a)::text FROM tl3_prompt_active a ORDER BY role",
                    &[],
                )
                .unwrap()
                .iter()
                .map(|r| r.get::<_, String>(0))
                .collect::<Vec<_>>(),
        )
        .collect()
}
