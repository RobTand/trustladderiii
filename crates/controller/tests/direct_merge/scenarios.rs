#[test]
fn reviewed_checked_direct_head_does_not_enqueue_with_mergify() {
    let mut database = support::Database::start();
    let settings = database.create("direct_delivery_red");
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let repo: tl3_config::Repository = serde_json::from_value(json!({
        "owner":"owner", "name":"sandbox", "installation_id":3, "delivery":"direct"
    })).unwrap();
    assert_eq!(repo.delivery, tl3_config::Delivery::Direct);
    let controller = queued_with(&settings, &api, root.path(), true, &[repo]);
    assert_eq!(api.state.lock().unwrap().posts, 0,
        "#830: a direct-mode repository must never enqueue with Mergify");
    assert!(matches!(controller.world().unwrap().nodes[0].phase,
        Phase::Closed(Outcome::Merged)), "#830: the reviewed checked head must merge directly");
}
