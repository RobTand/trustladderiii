//! #751 extraction equivalence: a crash must preserve the old adapter's replay inputs.
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

pub(super) fn crash_child(root: &Path) {
    let api = Server::new();
    {
        let mut state = api.state.lock().unwrap();
        state.checks = json!([]);
        state.pull["mergeable"] = json!(true);
        state.pull["mergeable_state"] = json!("clean");
    }
    let reads = AtomicUsize::new(0);
    let state = api.state.clone();
    api._runtime.block_on(async {
        Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/repos/owner/sandbox/pulls/100"))
            .respond_with(move |_: &Request| {
                let read = reads.fetch_add(1, Ordering::SeqCst) + 1;
                if read == 5 {
                    // No destructor runs: inspect the files left at final validation.
                    std::process::exit(73);
                }
                let mut state = state.lock().unwrap();
                if read == 3 {
                    state.pull["head"]["sha"] = json!("changed-after-absent");
                }
                ResponseTemplate::new(200).set_body_json(state.pull.clone())
            })
            .with_priority(1)
            .mount(&api.server)
            .await;
    });
    let mut adapter = api.adapter(root, 1);
    let result = adapter.execute(&delivery());
    panic!("did not reach the final-read crash point: {result:?}");
}

pub(super) fn stale_head_crash_preserves_intent() {
    let root = tempfile::tempdir().unwrap();
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .arg("--ordering-crash-child")
        .arg(root.path())
        .output()
        .unwrap();
    assert_eq!(
        child.status.code(),
        Some(73),
        "{}",
        String::from_utf8_lossy(&child.stderr)
    );
    let actual: BTreeMap<String, String> = std::fs::read_dir(root.path())
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            (
                path.file_name().unwrap().to_str().unwrap().to_owned(),
                std::fs::read_to_string(path).unwrap(),
            )
        })
        .collect();
    let baseline: BTreeMap<String, String> =
        serde_json::from_str(include_str!("ordering-baseline.json")).unwrap();
    println!(
        "ordering crash side files: {}",
        serde_json::to_string(&actual).unwrap()
    );
    assert_eq!(
        actual, baseline,
        "stale-head crash differs from the pre-extraction adapter's durable side files"
    );
    println!("test stale_head_crash_preserves_intent ... ok");
}
