// The real source models live in tests/loom and run with verify-loom.sh.
use tl3_controller::{LabelledIssue, cli::Input};

fn webhook() -> Input {
    Input::LabelledIssue(LabelledIssue {
        delivery: "channel-disconnect".into(),
        owner: "owner".into(),
        repo: "sandbox".into(),
        installation: 1,
        number: 42,
        title: "fixture".into(),
        actor: 42,
        label: "tl3".into(),
    })
}

#[test]
fn disconnected_controller_is_reported_to_each_source() {
    // loom 0.7 does not model receiver disconnection. Exercise the actual
    // standard channel here instead of asserting an unsupported loom behavior.
    let (sender, receiver) = std::sync::mpsc::channel::<Input>();
    drop(receiver);
    let other = sender.clone();
    let worker = std::thread::spawn(move || assert!(other.send(webhook()).is_err()));
    assert!(sender.send(webhook()).is_err());
    worker.join().unwrap();
}
