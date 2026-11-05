use super::*;
pub(crate) fn validate_plan_size(plan: &[Check]) -> Result<()> {
    const {
        assert!(MAX_REPORT_LINE_BYTES as u64 == tl3_harness_core::limits::MAX_REPORT_LINE_BYTES);
        assert!(
            super::results::MAX_RESULT_BYTES as u64 == tl3_harness_core::limits::MAX_RESULT_BYTES
        );
        assert!(
            (65536
                + crate::findings::MAX_FAILED_TESTS * crate::findings::MAX_MESSAGE_CHARS * 12
                + 2) as u64
                == tl3_harness_core::limits::INITIAL_RESULT_BYTES
        );
    }
    use tl3_harness_core::limits::{
        add_cost, check_cost, measured_bytes, plan_line_fits, result_fits, test_cost,
    };
    let tests: BTreeSet<_> = plan.iter().flat_map(|check| &check.tests).collect();
    let collection = serde_json::to_vec(&serde_json::json!({"event":"collection", "tests":tests}))?;
    // Preserve this caller's UTF-8 collection and compact ASCII result costs.
    let collection_bytes = add_cost(
        measured_bytes(collection.len() as u128),
        add_cost(4, measured_bytes(tests.len().saturating_sub(1) as u128)),
    );
    let largest_phase = tests.iter().try_fold(0, |largest: u64, test| {
        serde_json::to_vec(test)
            .map(|bytes| largest.max(add_cost(measured_bytes(bytes.len() as u128), 8192)))
    })?;
    if !plan_line_fits(collection_bytes, largest_phase) {
        return Err(Error::Invalid(
            "Check plan exceeds the 4194304-byte report limit; reduce the selected tests or parameter cases.",
        ));
    }
    let mut result_bytes = tl3_harness_core::limits::INITIAL_RESULT_BYTES;
    for check in plan {
        let mut header = serde_json::json!({
            "name":check.name, "kind":check.kind, "custody":"in-process",
            "verdict":"infrastructure-failure", "tests":[],
        });
        if let Some(forcing) = &check.forcing {
            header["forcing"] = serde_json::to_value(forcing)?;
        }
        let proof_bytes = if matches!(check.kind, Kind::Demonstrating | Kind::Reproducing) {
            Some(if let Some(proof) = &check.proof {
                measured_bytes(ascii_json_bytes(&serde_json::to_value(proof)?)? as u128)
            } else {
                0
            })
        } else {
            None
        };
        if let Some(symptom) = &check.declared_symptom {
            header["declared_symptom"] = serde_json::to_value(symptom)?;
        }
        result_bytes = add_cost(
            result_bytes,
            check_cost(
                measured_bytes(ascii_json_bytes(&header)? as u128),
                proof_bytes,
                check.declared_symptom.is_some(),
            ),
        );
        for test in &check.tests {
            let mut case = serde_json::json!({
                "test":test, "base":"uncollected", "head":"uncollected",
                "pre_existing":false, "passed":false, "message":"",
            });
            if check.forcing.is_some() {
                case["head_forced"] = "uncollected".into();
            }
            let case_bytes = measured_bytes(ascii_json_bytes(&case)? as u128);
            let node_bytes = measured_bytes(ascii_json_bytes(&serde_json::json!(test))? as u128);
            result_bytes = add_cost(
                result_bytes,
                test_cost(case_bytes, node_bytes, check.forcing.is_some()),
            );
        }
        if !result_fits(result_bytes) {
            return Err(Error::Invalid(
                "Check plan exceeds the 8388608-byte result limit; reduce the selected tests or checks.",
            ));
        }
    }
    Ok(())
}

fn ascii_json_bytes(value: &serde_json::Value) -> Result<usize> {
    Ok(serde_json::to_string(value)?
        .chars()
        .map(|ch| if ch.is_ascii() { 1 } else { 6 * ch.len_utf16() })
        .sum())
}
