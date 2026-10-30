use serde_json::json;
use tl3_github::*;

#[test]
fn check_run_accepts_a_null_app() {
    let mut check: serde_json::Value =
        serde_json::from_slice(include_bytes!("fixtures/check-run-completed.json")).unwrap();
    check["check_run"]["app"] = json!(null);
    let parsed: CheckRun = serde_json::from_value(check["check_run"].clone()).unwrap();
    assert_eq!(serde_json::to_value(parsed).unwrap()["app"], json!(null));
}

#[test]
fn unfinished_checks_omit_conclusions_and_completed_checks_send_them() {
    for status in [
        CheckStatus::Queued,
        CheckStatus::InProgress,
        CheckStatus::Completed,
    ] {
        let completed = matches!(status, CheckStatus::Completed);
        let check = NewCheckRun {
            name: "approval".into(),
            head_sha: "abc1234".into(),
            external_id: "approval-key".into(),
            status,
            conclusion: completed.then_some(CheckConclusion::Success),
        };
        let value = serde_json::to_value(check).unwrap();
        assert_eq!(
            value.get("conclusion"),
            completed.then_some(&json!("success"))
        );
    }
}

#[test]
fn actions_check_accepts_a_null_output_title() {
    let check: CheckRun =
        serde_json::from_slice(include_bytes!("fixtures/actions-check-null-title.json")).unwrap();
    assert_eq!(check.name, "Sandbox tests");
    assert_eq!(check.app.unwrap().slug, "github-actions");
}
