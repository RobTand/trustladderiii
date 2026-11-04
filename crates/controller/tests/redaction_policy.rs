use serde_json::Value;
use tl3_controller::redaction;

#[test]
fn generated_policy_matches_committed_file() {
    let committed: Value =
        serde_json::from_str(include_str!("../src/redaction/policy.json")).unwrap();
    let generated: Value = serde_json::from_str(&redaction::generated_policy()).unwrap();
    assert_eq!(generated, committed, "generated policy drift");
}

#[test]
fn generated_policy_retains_every_corpus_rule() {
    let committed: Value =
        serde_json::from_str(include_str!("../src/redaction/policy.json")).unwrap();
    let generated: Value = serde_json::from_str(&redaction::generated_policy()).unwrap();
    for key in ["patterns", "checked", "regex", "words", "characters"] {
        assert_eq!(
            generated[key], committed[key],
            "missing corpus rules: {key}"
        );
    }
}
