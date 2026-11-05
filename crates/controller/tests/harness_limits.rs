//! #835: pure extraction equivalence; the validator has no recorded I/O calls.
use proptest::prelude::*;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use tl3_controller::{
    Error, Result,
    contracts::{Check, CheckKind as Kind},
    findings,
};
use tl3_harness_core::limits;

const MAX_REPORT_LINE_BYTES: usize = 4 * 1024 * 1024;
mod results {
    pub const MAX_RESULT_BYTES: usize = 8 * 1024 * 1024;
}

// BEGIN BASE VALIDATOR
#[rustfmt::skip]
mod baseline {
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

}
// END BASE VALIDATOR

// BEGIN CURRENT VALIDATOR
#[rustfmt::skip]
mod current {
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

    pub(super) fn ascii_json_bytes_for_test(value: &Value) -> u64 { ascii_json_bytes(value).unwrap() as u64 }
}
// END CURRENT VALIDATOR

#[test]
fn compiled_extraction_is_the_production_validator_verbatim() {
    let source = include_str!("../src/adapters/prismabuild/checks.rs");
    let production = source
        .split_once("pub(crate) fn validate_plan_size(")
        .unwrap()
        .1
        .split_once("\n/// The plan with its base")
        .unwrap()
        .0
        .trim_end();
    let test = include_str!("harness_limits.rs")
        .split_once("// BEGIN CURRENT VALIDATOR\n")
        .unwrap()
        .1
        .split_once("// END CURRENT VALIDATOR")
        .unwrap()
        .0;
    let compiled = test
        .split_once("pub(crate) fn validate_plan_size(")
        .unwrap()
        .1
        .split_once("    pub(super) fn ascii_json_bytes_for_test")
        .unwrap()
        .0
        .trim_end();
    // Keep both compiled copies verbatim, including string literals.
    assert_eq!(
        compiled, production,
        "refresh the compiled production extraction after edits"
    );
    assert_eq!(
        limits::INITIAL_RESULT_BYTES as usize,
        65536 + findings::MAX_FAILED_TESTS * findings::MAX_MESSAGE_CHARS * 12 + 2
    );
}

fn check(
    name: String,
    text: String,
    count: usize,
    kind: u8,
    proof: bool,
    forced: bool,
    symptom: bool,
) -> Check {
    let kind = [
        "regression",
        "demonstrating",
        "reproducing",
        "behavior-preserving",
    ][kind as usize % 4];
    let node = format!("tests/test_x.py::test_x[{text}]");
    let mut value = json!({"name":name,"kind":kind,"tests":vec![node.clone();count]});
    if proof && matches!(kind, "demonstrating" | "reproducing") {
        value["proof"] = json!({"subjects":[],"observer_support":[],"red_message":"expected"});
    }
    if forced && kind == "demonstrating" {
        value["forcing"] = json!({"symptom":"wait", "environment":{"REPRODUCE":"1"},"patches":[]});
    }
    if symptom && kind == "demonstrating" {
        value["declared_symptom"] = json!({"node":node,"exception":"builtins.TimeoutError","phase":"call","repeat":1,"parallel":0});
    }
    serde_json::from_value(value).unwrap()
}

fn verdict(result: Result<()>) -> std::result::Result<(), String> {
    result.map_err(|error| error.to_string())
}

#[test]
fn controller_admission_and_errors_equal_the_base_on_large_repeated_plans() {
    for (count, nodes) in [(0, 0), (1, 1), (2, 500), (20, 32), (30, 100), (40, 100)] {
        for kind in 0..4 {
            for (proof, forced, symptom) in [
                (false, false, false),
                (true, false, false),
                (true, true, false),
                (true, false, true),
            ] {
                let plan = vec![
                    check(
                        "\u{1f680}\"\\\n".into(),
                        "é\u{1f600}\"\\\n".into(),
                        nodes,
                        kind,
                        proof,
                        forced,
                        symptom
                    );
                    count
                ];
                assert_eq!(
                    verdict(current::validate_plan_size(&plan)),
                    verdict(baseline::validate_plan_size(&plan))
                );
            }
        }
    }
    for length in [4_186_110, 4_186_111] {
        let plan = [check(
            "boundary".into(),
            "x".repeat(length),
            1,
            0,
            false,
            false,
            false,
        )];
        assert_eq!(
            verdict(current::validate_plan_size(&plan)),
            verdict(baseline::validate_plan_size(&plan))
        );
    }
}

#[test]
fn proof_json_spacing_and_absent_costs_remain_caller_specific() {
    let proof = json!({"subjects":[],"observer_support":[],"red_message":"expected"});
    let rust = current::ascii_json_bytes_for_test(&proof);
    let python =
        r#"{"subjects": [], "observer_support": [], "red_message": "expected"}"#.len() as u64;
    assert_eq!(python, rust + 5);
    assert_eq!(
        limits::check_cost(100, Some(python), false),
        limits::check_cost(100, Some(rust), false) + 5
    );
    assert_eq!(
        limits::check_cost(100, Some(4), false),
        limits::check_cost(100, Some(0), false) + 4
    );
    for number in [
        -0.0,
        0.0,
        f64::from_bits(1),
        9.5,
        9.999999999999998,
        10.0,
        10.000000000000002,
    ] {
        let wire =
            serde_json::to_string(&json!({"applied_seconds":number,"exhausted":false})).unwrap();
        let value: Value = serde_json::from_str(&wire).unwrap();
        let (bits, exhausted) = limits::decode_delay_budget(&value).unwrap();
        assert_eq!(bits, number.to_bits(), "{wire}");
        assert!(!exhausted);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]
    #[test]
    fn controller_admission_matches_pre_extraction(
        fields in proptest::collection::vec((proptest::collection::vec(any::<char>(),0..30),
            0usize..32, 0u8..4, any::<bool>(), any::<bool>(), any::<bool>()), 0..20)) {
        let plan: Vec<_> = fields.into_iter().map(|(text,count,kind,proof,forced,symptom)|
            check("generated check".into(), text.into_iter().collect(),count,kind,proof,forced,symptom)).collect();
        prop_assert_eq!(verdict(current::validate_plan_size(&plan)), verdict(baseline::validate_plan_size(&plan)));
    }

    #[test]
    fn ieee_boundary_conversion_and_predicate(bits in any::<u64>(), exhausted in any::<bool>()) {
        let decoded = f64::from_bits(bits);
        prop_assert_eq!(decoded.to_bits(),bits);
        let expected = (0.0..=10.0).contains(&decoded) && exhausted == (decoded == 10.0);
        prop_assert_eq!(limits::delay_budget_consistent(bits,exhausted),expected);
    }

    #[test]
    fn conditional_model_covers_real_bounded_result_serialization(
        points in proptest::collection::vec(any::<char>(),1..1001), count in 1usize..30, forced in any::<bool>()) {
        // This model explicitly bounds messages and excludes proof/selection/symptom
        // receipts. Their costs must be measured independently before extending it.
        let text: String = points.into_iter().collect();
        let mut result = json!({"name":"generated", "kind":"regression", "custody":"in-process",
            "verdict":"infrastructure-failure", "tests":[]});
        let header = current::ascii_json_bytes_for_test(&result);
        let node = "tests/test_x.py::test_x[\u{1f680}\"\\\n]";
        let mut case = json!({"test":node,"base":"uncollected","head":"uncollected",
            "pre_existing":false,"passed":false,"message":""});
        if forced { case["head_forced"] = json!("uncollected"); }
        let mut reservation = limits::add_cost(limits::INITIAL_RESULT_BYTES, limits::check_cost(header,None,false));
        let charge = limits::test_cost(current::ascii_json_bytes_for_test(&case),
            current::ascii_json_bytes_for_test(&json!(node)),forced);
        case["message"] = json!(text);
        result["tests"] = json!(vec![case;count]);
        let typed: tl3_controller::contracts::CheckResult = serde_json::from_value(result).unwrap();
        let encoded = serde_json::to_vec(&typed).unwrap();
        for _ in 0..count { reservation = limits::add_cost(reservation,charge); }
        prop_assert!((encoded.len() as u64) <= reservation);
        prop_assert!(limits::result_fits(reservation));
    }
}
