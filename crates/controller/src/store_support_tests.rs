//! #731: the library's tests share one Postgres per process, never a database.
use crate::store_support::Database;
use serde_json::json;
use tl3_store::Store;

#[test]
fn tests_share_one_server_but_never_a_database() {
    let mut first = Database::start();
    let mut second = Database::start();
    let a = first.create("shared_isolation");
    let b = second.create("shared_isolation");
    assert_eq!(
        (a.host.as_str(), a.port),
        (b.host.as_str(), b.port),
        "one server per test process"
    );
    assert_ne!(
        a.database, b.database,
        "a reused name still gets its own database"
    );
    let mut store = Store::open(&a).unwrap();
    store
        .put_worker_materials("0", &json!({"owner": "first"}))
        .unwrap();
    assert_eq!(
        Store::open(&b).unwrap().worker_materials("0").unwrap(),
        None,
        "one test's rows never reach another's database"
    );
    drop(store);
    first.stop();
    assert!(Store::open(&a).is_err(), "stop ends the test's databases");
    assert!(
        Store::open(&b).is_ok(),
        "and leaves other tests' databases alone"
    );
}
