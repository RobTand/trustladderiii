use super::*;
pub(crate) fn validate_plan_size(plan: &[Check]) -> Result<()> {
    let tests: BTreeSet<_> = plan.iter().flat_map(|check| &check.tests).collect();
    let collection = serde_json::to_vec(&serde_json::json!({"event":"collection", "tests":tests}))?;
    // Python's event encoding adds three object spaces, list separators and a newline.
    let collection_bytes = collection.len() + 4 + tests.len().saturating_sub(1);
    let oversized_phase = tests.iter().try_fold(false, |oversized, test| {
        serde_json::to_vec(test)
            .map(|bytes| oversized || bytes.len() + 8192 > MAX_REPORT_LINE_BYTES)
    })?;
    if collection_bytes > MAX_REPORT_LINE_BYTES || oversized_phase {
        return Err(Error::Invalid(
            "Check plan exceeds the 4194304-byte report limit; reduce the selected tests or parameter cases.",
        ));
    }
    let mut result_bytes =
        65536 + crate::findings::MAX_FAILED_TESTS * crate::findings::MAX_MESSAGE_CHARS * 12 + 2;
    for check in plan {
        let mut header = serde_json::json!({
            "name":check.name, "kind":check.kind, "custody":"in-process",
            "verdict":"infrastructure-failure", "tests":[],
        });
        if let Some(forcing) = &check.forcing {
            // A forced check's result echoes its forcing, and each test gains head_forced (#393).
            header["forcing"] = serde_json::to_value(forcing)?;
        }
        if matches!(check.kind, Kind::Demonstrating | Kind::Reproducing) {
            result_bytes += 65536;
            if let Some(proof) = &check.proof {
                result_bytes += ascii_json_bytes(&serde_json::to_value(proof)?)?;
            }
        }
        if let Some(symptom) = &check.declared_symptom {
            header["declared_symptom"] = serde_json::to_value(symptom)?;
            // Up to thirty reports, each with a bounded pytest traceback and identity.
            result_bytes += 384 * 1024;
        }
        result_bytes += ascii_json_bytes(&header)? + 1;
        for test in &check.tests {
            let mut case = serde_json::json!({
                "test":test, "base":"uncollected", "head":"uncollected",
                "pre_existing":false, "passed":false, "message":"",
            });
            if check.forcing.is_some() {
                case["head_forced"] = "uncollected".into();
            }
            result_bytes += ascii_json_bytes(&case)? + 1;
            // Match the harness reservation: three bounded phase facts and the concrete node per side.
            let sides = if check.forcing.is_some() { 3 } else { 2 };
            result_bytes +=
                sides * (3 * (200 * 12 + 100) + 100 + ascii_json_bytes(&serde_json::json!(test))?);
        }
        if result_bytes > super::results::MAX_RESULT_BYTES {
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
