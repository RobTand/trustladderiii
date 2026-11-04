//! #734: each harness contract vector, parsed by the controller, projected into the
//! reviewer's and the maker's inputs and validated against the packaged schemas. #717
//! reached production because a new receipt key only failed here, at review setup.

use super::*;
use crate::adapters::prismabuild::contract_vectors;
use crate::findings;
use crate::git_tests::{Repo, documents, support::Database};

#[test]
fn reviewer_input_accepts_every_harness_vector() {
    let f = Repo::new();
    let mut db = Database::start();
    for (index, parsed) in contract_vectors::parsed().into_iter().enumerate() {
        let name = &parsed.name;
        let mut store = Store::open(&db.create(&format!("contract_vector_{index}"))).unwrap();
        // A forced check's review also carries the issue (#393).
        store
            .put_worker_materials(
                "0",
                &json!({"issue": {"title":"Contract vector", "body":"Shared harness report"}}),
            )
            .unwrap();
        let mut snapshot = documents(
            &mut store,
            &[
                (
                    "goals",
                    json!([{"description":"Contract vector", "verification":"test"}]),
                ),
                ("check_reports", json!({})),
            ],
        );
        let report = snapshot
            .rows
            .iter_mut()
            .find(|row| row.table == "check_reports")
            .unwrap();
        report.id = format!("0:{}", f.head);
        report.value = json!({"base_sha": f.base, "results": parsed.results});
        let inputs =
            review::inputs(&f.delivery(), &mut store, &snapshot, None).unwrap_or_else(|error| {
                panic!("{name}: reviewer-input.json rejects the vector: {error}")
            });
        assert_eq!(
            inputs["checks"], parsed.vector["results"],
            "{name}: review must see the harness results unchanged"
        );
    }
}

#[test]
fn maker_feedback_accepts_every_failing_harness_vector() {
    let feedback = protocol::input_property_validator("maker", "feedback");
    let mut failing = 0;
    for parsed in contract_vectors::parsed() {
        if parsed.pass {
            continue;
        }
        failing += 1;
        // The failed-check finding the controller records for a failed round (lib.rs).
        let finding = findings::Finding {
            id: None,
            reviewed_sha: None,
            kind: findings::Kind::FailedCheck,
            report: findings::clipped(
                &findings::check_report(false, true, &parsed.failed_tests, &[], 0),
                findings::MAX_REPORT_CHARS,
            ),
            goal_ids: vec![],
            failed_tests: parsed.failed_tests,
            outside_paths: vec![],
            outside_paths_total: 0,
            ci_checks: vec![],
        };
        let items = findings::bounded_feedback(vec![serde_json::to_value(finding).unwrap()], None);
        let value = json!(items);
        let errors: Vec<_> = feedback
            .iter_errors(&value)
            .map(|error| format!("{} at {}", error, error.instance_path()))
            .collect();
        assert!(
            errors.is_empty(),
            "{}: maker-input.json rejects the vector's feedback: {errors:?}",
            parsed.name
        );
    }
    assert!(
        failing > 0,
        "no vector exercises the maker's failed-check feedback"
    );
}
