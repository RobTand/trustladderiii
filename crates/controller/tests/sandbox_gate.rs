//! Replay immutable sandbox head evidence through the shared check classifier.
use std::path::PathBuf;
use tl3_github::checks::{self, required};

fn evidence(name: &str, head_override: Option<&str>) -> Vec<bool> {
    let root=PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../deploy/sandbox/evidence").join(name);
    let load=|part:&str| -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(root.join(format!("{part}.json"))).unwrap()).unwrap()
    };
    let pull=load("pull");
    let head=head_override.unwrap_or(pull["head"]["sha"].as_str().unwrap());
    let runs:Vec<tl3_github::CheckRun>=serde_json::from_value(load("head-checks")).unwrap();
    let views=checks::views(&runs,head,|run| run.name == "Sandbox tests");
    // Historical queue verdicts have no target authority. Exact-head required
    // CI and the own-app approval remain separate, conjunctive evidence.
    let required:Vec<_>=runs.iter().zip(&views).filter(|(run,_)| run.name == "Sandbox tests").collect();
    assert!(!required.is_empty(),"the recorded required check must be exercised");
    let passed=required.iter().all(|(run,view)| run.head_sha == head
        && required::required_check(**view,false) == required::RequiredState::Passed);
    let approvals:Vec<_>=runs.iter().filter(|run| run.name == "TL3 approval").collect();
    let approved=!approvals.is_empty() && approvals.iter().all(|run| run.head_sha == head
        && run.status == "completed" && run.conclusion.as_deref() == Some("success")
        && run.app.as_ref().is_some_and(|app| app.id == 5542171));
    vec![passed && approved]
}

#[test]
fn bootstrap_merge_has_no_approved_delivery_evidence() {
    assert_eq!(evidence("bootstrap",None),[false]);
}

#[test]
fn approved_merge_has_exact_head_required_ci_evidence() {
    assert_eq!(evidence("queue-trials/approved/after",None),[true]);
    assert_eq!(evidence("queue-trials/approved/after",Some(&"f".repeat(40))),[false]);
}
