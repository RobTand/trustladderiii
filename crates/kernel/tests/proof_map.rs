//! Gated Verus replacements remain on the proof map.

const RETIREMENT: &str = include_str!("../verus/retirement.rs");
const VERUS_GROUPS: &str = include_str!("../../../tools/bazel/verus.bzl");

// #806: these names retain their event/phase coverage after Kani retirement.
const VERUS_RETIREMENTS: &[(&str, &str)] = &[
    ("apply_dependency", "dependency_retirement_fact"),
    (
        "dependency_abandoned_never_releases",
        "dependency_retirement_abandoned",
    ),
    // #832: new semantic replies are covered by Verus; Kani stays frozen.
    ("apply_plan_check_rejected", "plan_check_witness"),
    ("apply_plan_check_needs_split", "plan_check_witness"),
    ("apply_incident_cleared", "facts_incident_cleared"),
    (
        "apply_publishing_ci_infrastructure",
        "facts_ci_infrastructure",
    ),
    ("pr_merged_ingesting", "facts_merge"),
    ("pr_merged_auditing", "facts_merge"),
    ("pr_merged_unplanned", "facts_merge"),
    ("pr_merged_planning", "facts_merge"),
    ("pr_merged_ready", "facts_merge"),
    ("pr_merged_making", "facts_merge"),
    ("pr_merged_checking", "facts_merge"),
    ("pr_merged_reviewing", "facts_merge"),
    ("pr_merged_publishing", "facts_merge"),
    ("pr_merged_awaiting_delivery", "facts_merge"),
    ("pr_merged_rebasing", "facts_merge"),
    ("pr_merged_awaiting_rob", "facts_merge"),
    ("pr_merged_stopping", "facts_merge"),
    ("pr_merged_closed", "facts_merge"),
    ("apply_priority", "facts_transition"),
    ("apply_pr_closed", "facts_transition"),
    ("apply_conflict", "facts_transition"),
    ("gate_progress_w2", "progress_gate_classification"),
    ("gate_progress_w3", "progress_gate_classification"),
    ("gate_waiting_progress_w2", "progress_gate_waiting"),
    ("gate_waiting_progress_w3", "progress_gate_waiting"),
    ("gate_maker_progress_w2", "progress_gate_maker"),
    ("gate_maker_progress_w3", "progress_gate_maker"),
    ("gate_merge_progress_w2", "progress_gate_apply_merge"),
    ("gate_merge_progress_w3", "progress_gate_apply_merge"),
    ("gate_resume_progress_w2", "progress_gate_apply_resume"),
    ("gate_resume_progress_w3", "progress_gate_apply_resume"),
    ("progress_from_ingesting", "progress_phase_witness"),
    ("progress_from_auditing", "progress_phase_witness"),
    ("progress_from_planning", "progress_phase_witness"),
    ("progress_from_ready", "progress_phase_witness"),
    ("progress_from_making", "progress_phase_witness"),
    ("progress_from_checking", "progress_phase_witness"),
    ("progress_from_reviewing", "progress_phase_witness"),
    ("progress_from_publishing", "progress_phase_late_cover"),
    (
        "progress_from_awaiting_delivery",
        "progress_phase_late_cover",
    ),
    ("progress_from_rebasing", "progress_phase_late_cover"),
    ("progress_from_awaiting_rob", "progress_phase_late_cover"),
    ("progress_from_stopping", "progress_phase_late_cover"),
    ("progress_from_closed", "progress_phase_late_cover"),
    ("progress_from_unplanned", "progress_unplanned"),
    (
        "progress_from_unplanned_delivered",
        "progress_unplanned_admission",
    ),
    (
        "progress_from_unplanned_delivered_one",
        "progress_unplanned_admission",
    ),
    (
        "progress_from_unplanned_waiting",
        "progress_unplanned_waiting",
    ),
    (
        "progress_from_unplanned_waiting_one",
        "progress_unplanned_waiting",
    ),
    ("apply_audit_disagrees", "port_t860_apply"),
];

#[test]
fn verus_replacements_are_gated() {
    assert!(RETIREMENT.contains("fn verify_selected_apply("));
    assert!(RETIREMENT.contains("fn fact_preserves_node("));
    assert!(RETIREMENT.contains("fn fact_preserves_world("));
    for &(_, group) in VERUS_RETIREMENTS {
        assert!(
            VERUS_GROUPS.contains(&format!("\"{group}\": (")),
            "ungated replacement: {group}"
        );
    }
    for function in [
        "merge_input_distance",
        "node_frame",
        "fact_preserves_node",
        "fact_preserves_world",
        "verify_selected_apply",
        "infrastructure_frame",
        "infrastructure_result",
    ] {
        assert!(VERUS_GROUPS.contains(&format!(
            "\"retirement_{function}\": (\"kernel::retirement#{function}\", 1)"
        )));
    }
    assert!(VERUS_GROUPS.contains("\"retirement_witness\": (\"kernel::retirement_witness\","));
}

#[path = "proof_map/inventory.rs"]
mod inventory;
