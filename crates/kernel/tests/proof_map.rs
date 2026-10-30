//! Every event variant and every phase has a Kani proof, and every proof is
//! on the map. `proof_of_event` and `proofs_of_phase` are exhaustive matches,
//! so a new variant or phase without an entry doesn't compile. This test
//! checks that each entry names a proof that exists.

use std::collections::BTreeSet;

const LIB: &str = include_str!("../src/lib.rs");
const PROOFS: &str = include_str!("../src/proofs.rs");

/// Quoted names between the proof-map markers in lib.rs.
fn mapped() -> BTreeSet<&'static str> {
    let start = LIB.find("// proof map start").expect("start marker");
    let end = LIB.find("// proof map end").expect("end marker");
    LIB[start..end]
        .split('"')
        .skip(1)
        .step_by(2)
        .filter(|s| ["apply_", "advance_", "progress_from_", "pr_merged_"].iter().any(|p| s.starts_with(p)))
        .collect()
}

/// Names of the per-variant and per-phase proofs in proofs.rs.
fn proven() -> BTreeSet<&'static str> {
    PROOFS
        .lines()
        .filter_map(|l| l.strip_prefix("proof!("))
        .filter_map(|l| l.split(',').next())
        .collect()
}

/// A mapped name ending in `_by_phase` stands for one proof per phase, named
/// like the per-phase advance proofs: `pr_merged_by_phase` means
/// `pr_merged_ingesting`, `pr_merged_auditing`, and so on.
fn expand(mapped: BTreeSet<&'static str>) -> BTreeSet<String> {
    let phases: Vec<&str> = mapped.iter().filter_map(|m| m.strip_prefix("advance_")).collect();
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
    let mapped = expand(mapped());
    let proven: BTreeSet<String> = proven().into_iter().map(String::from).collect();
    let missing: Vec<_> = mapped.difference(&proven).collect();
    let unmapped: Vec<_> = proven.difference(&mapped).collect();
    assert!(missing.is_empty(), "mapped but not proven: {missing:?}");
    assert!(unmapped.is_empty(), "proven but not on the map: {unmapped:?}");
    println!("{} proofs, one per event variant and two per phase", proven.len());
}
