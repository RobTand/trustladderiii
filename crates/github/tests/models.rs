use serde_json::json;
use tl3_github::*;

/// #766: cleanup news must recover exactly one complete message marker.
#[test]
fn message_marker_keys_round_trip_and_reject_ambiguous_or_invalid_frames() {
    for key in [
        "effect:0:8",
        "console-note:766",
        "notice:766:original",
        "message-λ",
    ] {
        assert_eq!(
            GitHub::comment_effect_key(&format!("Done\n\n{}", GitHub::marker(key))),
            Some(key.into())
        );
    }
    for body in [
        "",
        "<!-- tl3:effect: -->",
        "<!-- tl3:effect:xx -->",
        "<!-- tl3:effect:ff -->",
        "prefix <!-- tl3:effect:61 -->",
        "<!-- tl3:effect:61 --> suffix",
        "<!-- tl3:effect:61 -->\n<!-- tl3:effect:62 -->",
    ] {
        assert_eq!(GitHub::comment_effect_key(body), None, "{body}");
    }
}

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
