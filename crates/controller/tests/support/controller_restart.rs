pub fn wait_for_owner_release(connection: &mut postgres::Client) {
    // Match Store::claim_controller; reaping its client does not reap the server session.
    let ownership = 0x544c33434f4e5452_i64;
    connection
        .batch_execute("SET lock_timeout = '90s'")
        .unwrap();
    connection
        .query_one("SELECT pg_advisory_lock($1)", &[&ownership])
        .unwrap();
    assert!(
        connection
            .query_one("SELECT pg_advisory_unlock($1)", &[&ownership])
            .unwrap()
            .get::<_, bool>(0)
    );
}
