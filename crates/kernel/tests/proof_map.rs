//! Every event variant and every phase has a Kani or gated Verus proof, and every proof is
//! on the map. `proof_of_event` and `proofs_of_phase` are exhaustive matches,
//! so a new variant or phase without an entry doesn't compile. This test
//! checks that each entry names a proof that exists.

use std::collections::BTreeSet;

const ADDITIONAL_PROOFS: &[&str] = &[
    "progress_from_unplanned_delivered",
    "progress_from_unplanned_delivered_one",
    "progress_from_unplanned_waiting",
    "progress_from_unplanned_waiting_one",
    "apply_dependency_registration",
    "dependency_no_cycle",
    "dependency_waiting_holds_nothing",
    "dependency_delivered_releases",
    "dependency_abandoned_never_releases",
    "apply_publishing_ci_tests",
    "apply_publishing_ci_infrastructure",
    "apply_publishing_ci_earlier_step",
    "fail_fast_plan_cant_meet",
    "fail_fast_maker_needs_files",
    "fail_fast_maker_cant_meet",
    "fail_fast_maker_out_of_room",
    "fail_fast_checks_done",
    "fail_fast_changes_requested",
    "fail_fast_reviewer_out_of_room",
    "fail_fast_ci_failed_queued",
    "fail_fast_ci_failed_publishing",
    "gate_leaf_progress_w2",
    "gate_leaf_progress_w3",
    "gate_grant_w2",
    "gate_grant_w3",
    "gate_planning_w2",
    "gate_planning_w3",
    "gate_progress_w2",
    "gate_progress_w3",
    "gate_waiting_progress_w2",
    "gate_waiting_progress_w3",
    "gate_maker_progress_w2",
    "gate_maker_progress_w3",
    "gate_merge_progress_w2",
    "gate_merge_progress_w3",
    "gate_resume_progress_w2",
    "gate_resume_progress_w3",
    // Q39: strictly priority-first agent-pool grants.
    "pool_grant_is_priority_ordered",
    "pool_order_is_strict_priority",
];

const LIB: &str = include_str!("../src/lib.rs");
const PROOFS: &str = include_str!("../src/proofs.rs");
const RETIREMENT: &str = include_str!("../verus/retirement.rs");
const VERUS_GROUPS: &str = include_str!("../../../tools/bazel/verus.bzl");

// #806: these names retain their event/phase coverage after Kani retirement.
const VERUS_RETIREMENTS: &[(&str, &str)] = &[
    // #832: new semantic replies are covered by Verus; Kani stays frozen.
    ("apply_plan_check_rejected", "plan_check_witness"),
    ("apply_plan_check_needs_split", "plan_check_witness"),
    ("apply_incident_cleared", "facts_incident_cleared"),
    ("apply_pr_dequeued", "facts_pr_dequeued"),
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
    ("pr_merged_queued", "facts_merge"),
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
    ("progress_from_queued", "progress_phase_late_cover"),
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
    // #806 W1: real apply, complete checker and executable original-domain covers.
    ("apply_ingest_done", "port_replies_witness_cover"),
    ("apply_ingest_early_end", "port_replies_witness_cover"),
    ("apply_protected_paths", "port_replies_witness_cover"),
    ("apply_audit_agrees", "port_replies_witness_cover"),
    ("apply_audit_disagrees", "port_replies_witness_cover"),
    ("apply_plan_leaf", "port_replies_witness_cover"),
    ("apply_plan_split", "port_replies_witness_cover"),
    ("apply_plan_cant_meet", "port_replies_witness_cover"),
    // #806 W1 slice 2: all four maker rows use real apply and returned-world covers.
    ("apply_maker_done", "port_replies_maker_witness_cover"),
    ("apply_maker_cant_meet", "port_replies_maker_witness_cover"),
    ("apply_maker_out_of_room", "port_replies_maker_witness_cover"),
    ("apply_maker_needs_files", "port_replies_maker_witness_cover"),
];

/// Quoted names between the proof-map markers in lib.rs.
fn mapped() -> BTreeSet<&'static str> {
    let start = LIB.find("// proof map start").expect("start marker");
    let end = LIB.find("// proof map end").expect("end marker");
    LIB[start..end]
        .split('"')
        .skip(1)
        .step_by(2)
        .filter(|s| {
            ["apply_", "advance_", "progress_from_", "pr_merged_"]
                .iter()
                .any(|p| s.starts_with(p))
        })
        .collect()
}

/// Names of the per-variant and per-phase proofs in proofs.rs. Only calls
/// that start a line count, so a commented-out proof is missing, not proven.
fn proven() -> BTreeSet<&'static str> {
    PROOFS
        .split("\nproof!(")
        .skip(1)
        .filter_map(|call| call.split(',').next())
        .map(str::trim)
        .collect()
}

/// A mapped name ending in `_by_phase` stands for one proof per phase, named
/// like the per-phase advance proofs: `pr_merged_by_phase` means
/// `pr_merged_ingesting`, `pr_merged_auditing`, and so on.
fn expand(mapped: BTreeSet<&'static str>) -> BTreeSet<String> {
    let phases: Vec<&str> = mapped
        .iter()
        .filter_map(|m| m.strip_prefix("advance_"))
        .collect();
    let mut names = BTreeSet::new();
    for m in &mapped {
        match m.strip_suffix("_by_phase") {
            Some(family) => names.extend(phases.iter().map(|p| format!("{family}_{p}"))),
            None => {
                names.insert(m.to_string());
            }
        }
    }
    names
}

#[test]
fn every_event_and_phase_has_a_proof() {
    let mut mapped = expand(mapped());
    mapped.extend(ADDITIONAL_PROOFS.iter().map(|name| name.to_string()));
    let mut proven: BTreeSet<String> = proven().into_iter().map(String::from).collect();
    assert!(RETIREMENT.contains("fn verify_selected_apply("));
    assert!(RETIREMENT.contains("fn fact_preserves_node("));
    assert!(RETIREMENT.contains("fn fact_preserves_world("));
    for &(retired, group) in VERUS_RETIREMENTS {
        assert!(
            !proven.contains(retired),
            "retired harness remains: {retired}"
        );
        assert!(
            VERUS_GROUPS.contains(&format!("\"{group}\": (")),
            "ungated replacement: {group}"
        );
        proven.insert(retired.to_string());
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
    let missing: Vec<_> = mapped.difference(&proven).collect();
    let unmapped: Vec<_> = proven.difference(&mapped).collect();
    assert!(missing.is_empty(), "mapped but not proven: {missing:?}");
    assert!(
        unmapped.is_empty(),
        "proven but not on the map: {unmapped:?}"
    );
    println!(
        "{} proofs, covering every event variant and phase",
        proven.len()
    );
}

#[path = "proof_map/inventory.rs"]
mod inventory;
