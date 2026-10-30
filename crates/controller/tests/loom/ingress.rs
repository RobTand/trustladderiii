use super::*;
use crate::loom_tests::{bootstrap, model};
use loom::sync::{
    atomic::{AtomicUsize, Ordering},
    mpsc as channel,
};

struct Labels {
    applied: channel::Receiver<()>,
    first: bool,
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
        Err(Error::Invalid("model source finished"))
    }
}

#[test]
fn loom_sources_run_real_controller_once_per_request() {
    model(|settings| {
        let root = tempfile::tempdir().unwrap();
        let (ack, applied) = channel::channel();
        let count = Arc::new(AtomicUsize::new(0));
        let seen = count.clone();
        let mut store = Store::open(settings).unwrap();
        store.set_boundary_hook(move |point, key, _| {
            if point == tl3_store::SavePoint::AfterCommit && key.starts_with("github:loom-") {
                seen.fetch_add(1, Ordering::Release);
                ack.send(()).unwrap();
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
        });
        assert!(matches!(
            sources.run(&mut controller),
            Err(Error::Invalid("model source finished"))
        ));
        assert_eq!(count.load(Ordering::Acquire), 2, "lost ingress input");
        assert_eq!(
            controller.world().unwrap().nodes.len(),
            1,
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
            2
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
