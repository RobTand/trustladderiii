// This channel model uses fixture sources; production-code models live in
// tests/loom and run with verify-loom.sh.
use loom::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};
use tl3_controller::{LabelledIssue, cli::Input, executor::Observation};
use tl3_kernel::{Event, NodeId, Reply};

fn webhook() -> Input {
    Input::LabelledIssue(LabelledIssue {
        delivery: "loom-webhook".into(),
        owner: "RobTand".into(),
        repo: "tl3-sandbox".into(),
        installation: 1,
        number: 42,
        title: "Loom ingress fixture".into(),
        actor: 24923402,
        label: "tl3".into(),
    })
}
fn worker_reply(index: usize) -> Input {
    Input::Observation {
        key: format!("worker-{index}"),
        observation: Observation::Kernel(Event::Reply {
            node: NodeId(index as u64),
            generation: 0,
            reply: Reply::IngestDone { size: 1 },
        }),
    }
}

#[test]
fn sources_publish_evidence_before_one_owner_consumes_it() {
    loom::model(|| {
        let (sender, receiver) = mpsc::channel();
        let durable = Arc::new(AtomicBool::new(false));
        let exited = Arc::new([AtomicBool::new(false), AtomicBool::new(false)]);
        let webhook = {
            let sender = sender.clone();
            let durable = durable.clone();
            thread::spawn(move || {
                durable.store(true, Ordering::Release);
                assert!(sender.send(webhook()).is_ok());
            })
        };
        let worker = {
            let exited = exited.clone();
            thread::spawn(move || {
                for index in 0..2 {
                    exited[index].store(true, Ordering::Release);
                    assert!(sender.send(worker_reply(index)).is_ok());
                }
            })
        };
        let mut seen = [false; 3];
        for _ in 0..3 {
            match receiver.recv().unwrap() {
                Input::Wake
                | Input::GitHubWake
                | Input::Look { .. }
                | Input::ProposedIssue { .. }
                | Input::AdoptionClarification(_) => {
                    panic!("fixture sources send only issue labels and worker replies")
                }
                Input::LabelledIssue(_) => {
                    assert!(durable.load(Ordering::Acquire));
                    assert!(!seen[0]);
                    seen[0] = true;
                }
                Input::Observation { key, .. } => {
                    let index = match key.as_str() {
                        "worker-0" => 0,
                        "worker-1" => 1,
                        _ => unreachable!(),
                    };
                    assert!(exited[index].load(Ordering::Acquire));
                    if index == 1 {
                        assert!(seen[1]);
                    }
                    assert!(!seen[index + 1]);
                    seen[index + 1] = true;
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
    // loom 0.7 does not model receiver disconnection. Exercise the actual
    // standard channel here instead of asserting an unsupported loom behavior.
    let (sender, receiver) = std::sync::mpsc::channel::<Input>();
    drop(receiver);
    let other = sender.clone();
    let worker = std::thread::spawn(move || assert!(other.send(worker_reply(0)).is_err()));
    assert!(sender.send(webhook()).is_err());
    worker.join().unwrap();
}
