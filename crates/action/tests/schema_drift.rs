//! Compare the Rust action's authoritative generated contracts with committed artifacts.
const COMMITTED: &[(&str, &str)] = &[
    ("workers/src/tl3_workers/schemas/action-policy-request.json", include_str!("../../../workers/src/tl3_workers/schemas/action-policy-request.json")),
    ("workers/src/tl3_workers/schemas/action-policy-response.json", include_str!("../../../workers/src/tl3_workers/schemas/action-policy-response.json")),
    ("workers/src/tl3_workers/schemas/plugin-fact.json", include_str!("../../../workers/src/tl3_workers/schemas/plugin-fact.json")),
    ("workers/src/tl3_workers/schemas/plugin-response.json", include_str!("../../../workers/src/tl3_workers/schemas/plugin-response.json")),
];
#[test]
fn generated_action_schemas_match_committed() {
    let generated = tl3_action::generated();
    assert_eq!(generated.len(), COMMITTED.len());
    for (actual, (path, expected)) in generated.iter().zip(COMMITTED) {
        assert_eq!(&actual.path, path);
        assert_eq!(&actual.text, expected, "Regenerate with tl3-action schemas: {path}");
    }
}
