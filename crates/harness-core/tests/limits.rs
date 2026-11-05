//! #835: active plan-only consumer. The other 54 rows have different owners.
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::Path;
use tl3_harness_core::limits::*;

const LINE_ERROR: &str = "Check plan exceeds the 4194304-byte report limit; reduce the selected tests or parameter cases.";
const RESULT_ERROR: &str =
    "Check plan exceeds the 8388608-byte result limit; reduce the selected tests or checks.";

fn input_path(relative: &str) -> std::path::PathBuf {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative);
    if path.is_file() {
        return path;
    }
    Path::new(&std::env::var("TEST_SRCDIR").unwrap())
        .join(std::env::var("TEST_WORKSPACE").unwrap())
        .join(relative)
}

fn points(value: &Value) -> Vec<u32> {
    if let Some(text) = value.as_str() {
        text.chars().map(u32::from).collect()
    } else {
        value["python_codepoints"]
            .as_array()
            .unwrap()
            .iter()
            .map(|point| point.as_u64().unwrap() as u32)
            .collect()
    }
}

// Python's string serializer and backslashreplace at this test-only boundary.
fn string_bytes(points: &[u32], ascii: bool) -> u64 {
    2 + points
        .iter()
        .map(|&point| match point {
            8 | 9 | 10 | 12 | 13 | 34 | 92 => 2,
            0..=31 | 0xd800..=0xdfff => 6,
            127..=0xffff if ascii => 6,
            0x10000.. if ascii => 12,
            _ => char::from_u32(point).unwrap().len_utf8() as u64,
        })
        .sum::<u64>()
}

fn collection(tests: &[Vec<u32>]) -> (u64, u64) {
    let unique: BTreeSet<_> = tests.iter().collect();
    let bytes = 37
        + unique
            .iter()
            .map(|test| string_bytes(test, false))
            .sum::<u64>()
        + unique.len().saturating_sub(1) as u64 * 2;
    let phase = unique
        .iter()
        .map(|test| string_bytes(test, false) + 8192)
        .max()
        .unwrap_or(0);
    (bytes, phase)
}

fn compact_ascii(value: &Value) -> u64 {
    serde_json::to_string(value)
        .unwrap()
        .chars()
        .map(|ch| {
            if ch.is_ascii() {
                1
            } else {
                6 * ch.len_utf16() as u64
            }
        })
        .sum()
}

fn regression(count: usize) -> bool {
    let header = json!({"name":"vector", "kind":"regression", "custody":"in-process",
        "verdict":"infrastructure-failure", "tests":[]});
    let mut size = add_cost(
        INITIAL_RESULT_BYTES,
        check_cost(compact_ascii(&header), None, false),
    );
    for index in 0..count {
        let test = format!("tests/test_x.py::test_{index}");
        let case = json!({"test":test,"base":"uncollected","head":"uncollected",
            "pre_existing":false,"passed":false,"message":""});
        size = add_cost(
            size,
            test_cost(compact_ascii(&case), compact_ascii(&json!(test)), false),
        );
    }
    result_fits(size)
}

#[test]
fn fifteen_plan_rows_and_explicit_coverage_partition() {
    let root = input_path("harness/test-vectors/core/limits.json");
    let corpus: Value = serde_json::from_slice(&std::fs::read(root).unwrap()).unwrap();
    let (mut plans, mut options, mut forcing) = (0, 0, 0);
    for row in corpus["cases"].as_array().unwrap() {
        let input = &row["input"];
        let op = input["op"].as_str().unwrap();
        if op == "options" {
            options += 1;
            continue;
        }
        if op == "forcing" {
            forcing += 1;
            continue;
        }
        plans += 1;
        let expected = &row["expected"];
        let actual = if op == "check-plan" {
            if regression(input["count"].as_u64().unwrap() as usize) {
                json!({"value":null})
            } else {
                json!({"error":RESULT_ERROR})
            }
        } else {
            let tests = match op {
                "plan-size" => input["tests"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(points)
                    .collect(),
                "plan-size-repeat" => {
                    vec![points(&input["char"]).repeat(input["count"].as_u64().unwrap() as usize)]
                }
                "plan-size-collection" => {
                    let count = input["count"].as_u64().unwrap() as usize;
                    let mut tests: Vec<Vec<u32>> = (0..count)
                        .map(|index| {
                            format!("tests/test_many.py::test_{index:04}[]")
                                .chars()
                                .map(u32::from)
                                .collect()
                        })
                        .collect();
                    let empty = collection(&tests).0;
                    let padding = input["total_bytes"].as_u64().unwrap() - empty;
                    for (index, test) in tests.iter_mut().enumerate() {
                        test.pop();
                        test.extend(std::iter::repeat_n(
                            u32::from('x'),
                            (padding / count as u64) as usize
                                + usize::from((index as u64) < padding % count as u64),
                        ));
                        test.push(u32::from(']'));
                    }
                    tests
                }
                _ => panic!("unowned operation: {op}"),
            };
            let (bytes, phase) = collection(&tests);
            let validation = if plan_line_fits(bytes, phase) {
                json!({"value":null})
            } else {
                json!({"error":LINE_ERROR})
            };
            let mut actual = json!({"collection_bytes":bytes,"validation":validation});
            if op == "plan-size-collection" {
                actual["largest_phase_bytes"] = json!(phase);
            }
            actual
        };
        assert_eq!(actual, *expected, "limits/{}", row["id"]);
        println!("limits/{}: {actual}", row["id"]);
    }
    assert_eq!((plans, options, forcing), (15, 42, 12));
    println!(
        "coverage: 15 plan rows executed; 42 options rows owned by #823; 12 forcing rows owned by planner-contracts"
    );
}

#[test]
fn per_occurrence_costs_preserve_boundary_conventions() {
    assert_eq!(check_cost(100, None, false), 101);
    // Python's absent declaration is None encoded as four bytes; Rust's is zero.
    assert_eq!(check_cost(100, Some(4), false), 65_641);
    assert_eq!(check_cost(100, Some(0), false), 65_637);
    assert_eq!(check_cost(100, Some(23), true), 458_876);
    assert_eq!(test_cost(100, 20, false), 15_341);
    assert_eq!(test_cost(100, 20, true), 22_961);
    let one = test_cost(100, 20, false);
    assert_eq!(
        add_cost(one, one),
        2 * one,
        "duplicate result occurrences are charged twice"
    );
    assert_eq!(
        collection(&[vec![120], vec![120]]),
        collection(&[vec![120]])
    );
    assert_eq!(string_bytes(&[0x1f600, 34, 92, 10, 0xd800], true), 26);
}

#[test]
fn strict_limits_and_fixed_width_overflow() {
    assert!(plan_line_fits(MAX_REPORT_LINE_BYTES, MAX_REPORT_LINE_BYTES));
    assert!(!plan_line_fits(MAX_REPORT_LINE_BYTES + 1, 0));
    assert!(!plan_line_fits(0, MAX_REPORT_LINE_BYTES + 1));
    assert!(result_fits(MAX_RESULT_BYTES));
    assert!(!result_fits(MAX_RESULT_BYTES + 1));
    assert_eq!(measured_bytes(u128::MAX), COST_CAP);
    assert_eq!(add_cost(u64::MAX, u64::MAX), COST_CAP);
    assert_eq!(multiply_cost(u64::MAX, u64::MAX), COST_CAP);
    assert_eq!(multiply_cost(u64::MAX, 0), 0);
    assert_eq!(add_cost(MAX_RESULT_BYTES, 0), MAX_RESULT_BYTES);
}

#[test]
fn delay_receipt_boundary_fixtures() {
    let fixture: Value = serde_json::from_str(
        &std::fs::read_to_string(input_path(
            "crates/harness-core/tests/fixtures/limits/delay.json",
        ))
        .unwrap(),
    )
    .unwrap();
    let mut mismatches = Vec::new();
    for row in fixture.as_array().unwrap() {
        let accepted = serde_json::from_str::<Value>(row["json"].as_str().unwrap())
            .ok()
            .and_then(|value| decode_delay_budget(&value))
            .is_some_and(|(bits, exhausted)| delay_budget_consistent(bits, exhausted));
        let expected = row["valid"].as_bool().unwrap();
        if accepted != expected {
            mismatches.push((row["id"].as_str().unwrap(), accepted, expected));
        }
    }
    assert!(mismatches.is_empty(), "delay parity: {mismatches:?}");
    for bits in [
        0,
        1,
        0x8000000000000000,
        9.5f64.to_bits(),
        10f64.to_bits(),
        10f64.to_bits() + 1,
        u64::MAX,
        f64::INFINITY.to_bits(),
        f64::NAN.to_bits(),
    ] {
        let number = f64::from_bits(bits);
        assert_eq!(
            number.to_bits(),
            bits,
            "conversion round trip retains fractional bits"
        );
        for exhausted in [false, true] {
            let expected = (0.0..=10.0).contains(&number) && exhausted == (number == 10.0);
            assert_eq!(delay_budget_consistent(bits, exhausted), expected);
        }
    }
}

#[test]
fn inferred_receipt_gap_is_preserved_not_an_admission_change() {
    // #835 / Q27: 6000 inferred subject identities yielded a 1,530,499-byte
    // receipt. Admission measures only the declaration plus 64 KiB today.
    // This does NOT prove a global result_json bound; actual serialization is
    // guarded at 8 MiB minus 64 KiB. A new admission refusal needs lead review.
    let observed_receipt_bytes = 1_530_499;
    let reservation = check_cost(120, Some(40), false);
    assert!(observed_receipt_bytes > reservation);
    assert!(result_fits(add_cost(INITIAL_RESULT_BYTES, reservation)));
}
