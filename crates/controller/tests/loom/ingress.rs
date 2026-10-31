use super::*;
use crate::loom_tests::{bootstrap, model};
use loom::sync::{
    atomic::{AtomicUsize, Ordering},
    mpsc as channel,
};

struct Labels {
    applied: channel::Receiver<()>,
    first: bool,
    finished: channel::Sender<()>,
}
impl Source for Labels {
    fn run(self, sender: Sender<Input>) -> Result<()> {
        for index in 0..2 {
            let mut issue = LabelledIssue {
                delivery: format!("loom-{}-{index}", self.first),
                owner: "owner".into(),
                repo: "sandbox".into(),
                installation: 1,
                number: 42,
                title: "Loom request".into(),
                actor: 42,
                label: "tl3".into(),
            };
            if !self.first {
                issue.number = 43;
            }
            sender.send(Input::LabelledIssue(issue)).unwrap();
            self.applied.recv().unwrap();
        }
        self.finished.send(()).unwrap();
        Ok(())
    }
}
struct Finished(channel::Receiver<()>);
impl Source for Finished {
    fn run(self, _: Sender<Input>) -> Result<()> {
        self.0.recv().unwrap();
        self.0.recv().unwrap();
        Err(Error::Invalid("model source finished"))
    }
}

#[test]
fn loom_sources_run_real_controller_once_per_request() {
    model(0, |settings| {
        let root = tempfile::tempdir().unwrap();
        let (ack, applied) = channel::channel();
        let (other_ack, other_applied) = channel::channel();
        let (finished, finish) = channel::channel();
        let count = Arc::new(AtomicUsize::new(0));
        let seen = count.clone();
        let mut store = Store::open(settings).unwrap();
        store.set_boundary_hook(move |point, key, _| {
            if point == tl3_store::SavePoint::AfterCommit && key.starts_with("github:loom-") {
                seen.fetch_add(1, Ordering::Release);
                if key.starts_with("github:loom-true-") {
                    ack.send(()).unwrap();
                } else {
                    other_ack.send(()).unwrap();
                }
            }
        });
        let mut controller = Controller::open(
            store,
            crate::adapters::simulation(root.path()).unwrap(),
            bootstrap(),
        )
        .unwrap();
        let mut sources = Sources::default();
        sources.register(Labels {
            applied,
            first: true,
            finished: finished.clone(),
        });
        sources.register(Labels {
            applied: other_applied,
            first: false,
            finished,
        });
        sources.register(Finished(finish));
        assert!(matches!(
            sources.run(&mut controller),
            Err(Error::Invalid("model source finished"))
        ));
        assert_eq!(count.load(Ordering::Acquire), 4, "lost ingress input");
        assert_eq!(
            controller.world().unwrap().nodes.len(),
            2,
            "duplicate intake"
        );
        let events = tl3_store::Reader::open(settings)
            .unwrap()
            .events_after(tl3_store::Revision::ZERO, u32::MAX)
            .unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|e| e.key.starts_with("github:loom-"))
                .count(),
            4
        );
    });
}

#[test]
fn loom_webhook_control_keeps_gap_before_stop() {
    let mut builder = loom::model::Builder::new();
    builder.preemption_bound = Some(1);
    builder.check(|| {
        let server = http::Server::bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let (sender, receiver) = mpsc::channel();
        let control = WebhookControl(Arc::new(Control {
            sender,
            server: server.control(),
        }));
        let other = control.clone();
        let producer = thread::spawn(move || {
            other.gap();
            other.stop();
        });
        assert!(
            matches!(receiver.recv().unwrap(), WebhookMessage::Gap),
            "gap lost or reordered"
        );
        assert!(
            matches!(receiver.recv().unwrap(), WebhookMessage::Stop),
            "stop lost or doubled"
        );
        producer.join().unwrap();
        assert!(receiver.try_recv().is_err());
    });
}

struct DurableDelivery {
    store: Store,
    applied: channel::Receiver<()>,
    finished: channel::Sender<()>,
    number: u64,
}
impl Source for DurableDelivery {
    fn run(mut self, sender: Sender<Input>) -> Result<()> {
        use hmac::{Hmac, Mac};
        let body = serde_json::to_vec(&json!({
            "action": "labeled", "label": {"name": "tl3"},
            "issue": {"number": self.number, "title": "Durable loom request",
                "state": "open", "updated_at": "2026-10-30T00:00:00Z"},
            "repository": {"id": 1, "name": "sandbox", "owner": {"id": 42, "login": "owner"}},
            "installation": {"id": 1}, "sender": {"id": 42, "login": "owner"}
        }))
        .unwrap();
        let secret = b"loom-fixture-secret";
        let mut mac = Hmac::<sha2::Sha256>::new_from_slice(secret).unwrap();
        mac.update(&body);
        let request = http::Request::for_model(
            body,
            vec![
                (
                    "X-GitHub-Delivery".into(),
                    format!("durable-{}", self.number),
                ),
                ("X-GitHub-Event".into(), "issues".into()),
                (
                    "X-Hub-Signature-256".into(),
                    format!("sha256={}", hex::encode(mac.finalize().into_bytes())),
                ),
            ],
        );
        let (reply, response) = std::sync::mpsc::sync_channel(1);
        let notifications = webhooks::Notifications::new();
        notifications.register(sender);
        assert_eq!(
            receive_request(&mut self.store, secret, &request, &notifications, reply),
            (202, false)
        );
        assert_eq!(response.recv().unwrap(), 202);
        self.applied.recv().unwrap();
        self.finished.send(()).unwrap();
        Ok(())
    }
}

#[test]
fn loom_durable_ingress_applies_before_consuming_each_delivery_once() {
    model(0, |settings| {
        let root = tempfile::tempdir().unwrap();
        let (first_ack, first_applied) = channel::channel();
        let (second_ack, second_applied) = channel::channel();
        let (finished, finish) = channel::channel();
        let consumed = Arc::new(AtomicUsize::new(0));
        let count = consumed.clone();
        let settings_for_hook = settings.clone();
        let mut store = Store::open(settings).unwrap();
        store.set_boundary_hook(move |point, key, _| {
            if point == tl3_store::SavePoint::ConsumeIngressBeforeCommit {
                let events = tl3_store::Reader::open(&settings_for_hook)
                    .unwrap()
                    .events_after(tl3_store::Revision::ZERO, u32::MAX)
                    .unwrap();
                let delivery = key.strip_prefix("consume-ingress:github-webhook:").unwrap();
                assert_eq!(
                    events
                        .iter()
                        .filter(|event| event.key == format!("github:{delivery}"))
                        .count(),
                    1,
                    "delivery consumed before apply"
                );
            }
            if point == tl3_store::SavePoint::ConsumeIngressAfterCommit {
                count.fetch_add(1, Ordering::Relaxed);
                if key.ends_with("durable-42") {
                    first_ack.send(()).unwrap();
                } else {
                    second_ack.send(()).unwrap();
                }
            }
        });
        let mut controller = Controller::open(
            store,
            crate::adapters::simulation(root.path()).unwrap(),
            bootstrap(),
        )
        .unwrap();
        let mut sources = Sources::default();
        for (number, applied) in [(42, first_applied), (43, second_applied)] {
            let mut store = Store::open(settings).unwrap();
            store.set_boundary_hook(|point, _, _| {
                if point == tl3_store::SavePoint::ReceiveIngressBeforeCommit {
                    // Let the owner drain while the delivery is still invisible.
                    thread::yield_now();
                }
            });
            sources.register(DurableDelivery {
                store,
                number,
                applied,
                finished: finished.clone(),
            });
        }
        sources.register(Finished(finish));
        assert!(matches!(
            sources.run(&mut controller),
            Err(Error::Invalid("model source finished"))
        ));
        assert_eq!(
            consumed.load(Ordering::Relaxed),
            2,
            "delivery not consumed exactly once"
        );
        assert!(
            controller.store.pending_ingress().unwrap().is_empty(),
            "unconsumed delivery"
        );
        assert_eq!(
            controller.world().unwrap().nodes.len(),
            2,
            "delivery not applied exactly once"
        );
        let events = tl3_store::Reader::open(settings)
            .unwrap()
            .events_after(tl3_store::Revision::ZERO, u32::MAX)
            .unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.key.starts_with("github:durable-"))
                .count(),
            2
        );
    });
}
