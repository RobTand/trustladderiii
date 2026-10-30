// #9 and #10 have not landed. This models their agreed Sender<Input> shape,
// not production source code. Replace the model with those sources on integration.
use loom::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};

#[derive(Debug)]
enum Input {
    Webhook,
    Progress,
    WorkerReply,
}

#[test]
fn sources_publish_evidence_before_one_owner_consumes_it() {
    loom::model(|| {
        let (sender, receiver) = mpsc::channel();
        let durable = Arc::new(AtomicBool::new(false));
        let exited = Arc::new(AtomicBool::new(false));
        let webhook = {
            let sender = sender.clone();
            let durable = durable.clone();
            thread::spawn(move || {
                durable.store(true, Ordering::Release);
                sender.send(Input::Webhook).unwrap();
            })
        };
        let worker = {
            let exited = exited.clone();
            thread::spawn(move || {
                sender.send(Input::Progress).unwrap();
                exited.store(true, Ordering::Release);
                sender.send(Input::WorkerReply).unwrap();
            })
        };
        let mut seen = [false; 3];
        for _ in 0..3 {
            match receiver.recv().unwrap() {
                Input::Webhook => {
                    assert!(durable.load(Ordering::Acquire));
                    assert!(!seen[0]);
                    seen[0] = true;
                }
                Input::Progress => {
                    assert!(!seen[1]);
                    seen[1] = true;
                }
                Input::WorkerReply => {
                    assert!(exited.load(Ordering::Acquire));
                    assert!(seen[1]);
                    assert!(!seen[2]);
                    seen[2] = true;
                }
            }
        }
        webhook.join().unwrap();
        worker.join().unwrap();
        assert_eq!(seen, [true; 3]);
    });
}

#[test]
fn disconnected_controller_is_reported_to_each_source() {
    loom::model(|| {
        let (sender, receiver) = mpsc::channel::<Input>();
        drop(receiver);
        let other = sender.clone();
        let worker = thread::spawn(move || assert!(other.send(Input::WorkerReply).is_err()));
        assert!(sender.send(Input::Webhook).is_err());
        worker.join().unwrap();
    });
}
