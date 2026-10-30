use super::*;
use crate::executor::Executor;
use crate::loom_tests::delivery;
use crate::sync::mpsc;
#[path = "../fixtures/waits/mod.rs"]
mod waits;

fn model_bell(outage: bool) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let server = runtime.block_on(wiremock::MockServer::start());
    runtime.block_on(wiremock::Mock::given(wiremock::matchers::method("GET"))
        .respond_with(wiremock::ResponseTemplate::new(if outage {503} else {200})
            .set_body_json(serde_json::json!({"epoch":"0123456789abcdef0123456789abcdef", "seq":0, "reset":true, "events":[]})))
        .mount(&server));
    let url = server.uri();
    crate::loom_tests::bounded(move || {
        let root = tempfile::tempdir().unwrap();
        let mut adapter = waits::adapter_with_bell(root.path(), &url);
        let mut source = adapter.source();
        source.rebuild().unwrap();
        let path = root.path().to_owned();
        let d = delivery();
        let producer = crate::loom_tests::spawn(move || {
            let wait = tl3_store::ExternalWait {
                upstream: "prismabuild".into(),
                key: waits::key(),
            };
            adapter.watch_wait(&d, Some(&wait)).unwrap();
            adapter
        });
        // Registration can race the durable write. Its queued node notification
        // must repair any snapshot taken before that write.
        source.register(NodeId(0)).unwrap();
        let mut adapter = producer.join().unwrap();
        let (sender, receiver) = mpsc::channel();
        while !source.keys.is_empty() {
            runtime
                .block_on(source.poll(&sender, Duration::from_secs(1)))
                .unwrap();
        }
        assert_eq!(source.waits.len(), 1, "declared wait registration lost");
        waits::record(&path, "claimed");
        assert_eq!(
            runtime
                .block_on(source.poll(&sender, Duration::from_secs(1)))
                .unwrap(),
            0,
            "live reset/outage is not an ending"
        );
        assert!(receiver.try_recv().is_err());
        waits::record(&path, "done");
        assert_eq!(
            runtime
                .block_on(source.poll(&sender, Duration::from_secs(1)))
                .unwrap(),
            1,
            "reset/outage lost ending"
        );
        assert!(matches!(receiver.try_recv().unwrap(), Input::Look {key} if key == delivery().key));
        assert!(
            receiver.try_recv().is_err(),
            "ending doubled within one bell batch"
        );
        adapter.watch_wait(&delivery(), None).unwrap();
        while !source.keys.is_empty() {
            runtime
                .block_on(source.poll(&sender, Duration::from_secs(1)))
                .unwrap();
        }
        assert!(source.waits.is_empty(), "retired wait remains awaited");
    });
}
#[test]
fn loom_bell_reset_and_declared_wait_registration() {
    model_bell(false);
}
#[test]
fn loom_bell_outage_rereads_awaited_waits() {
    model_bell(true);
}
