use proptest::prelude::*;
use serde_json::{Value, json};
use tl3_harness_core::node_id::{AlwaysTestsError, Mode, NodeId, parse_always_tests};

const MODES: [Mode; 3] = [Mode::Exact, Mode::SelectedFile, Mode::AlwaysTest];

/// #824/#717: each current grammar and collection operation consumes its shared vectors.
#[test]
fn core_node_id_vectors() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../harness/test-vectors/core/node-id.json");
    let corpus: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(corpus["kind"], "node-id");
    let mut counts = [0; 3];
    for case in corpus["cases"].as_array().unwrap() {
        let input = &case["input"];
        let expected = &case["expected"];
        match input["op"].as_str().unwrap() {
            "grammar" => {
                let raw = input["node_id"].as_str().unwrap();
                for (mode, accepted) in [
                    (Mode::Exact, expected["exact"].as_bool().unwrap()),
                    (Mode::SelectedFile, expected["selected"].as_bool().unwrap()),
                    (Mode::AlwaysTest, expected["always"].get("value").is_some()),
                ] {
                    assert_eq!(
                        NodeId::parse(raw, mode).is_ok(),
                        accepted,
                        "{} {mode:?}",
                        case["id"]
                    );
                }
                counts[0] += 1;
            }
            "selects" => {
                let requested =
                    NodeId::parse(input["requested"].as_str().unwrap(), Mode::SelectedFile)
                        .unwrap();
                let actual: Vec<_> = input["collected"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|node| requested.selects(node.as_str().unwrap()))
                    .collect();
                assert_eq!(json!(actual), *expected, "{}", case["id"]);
                counts[1] += 1;
            }
            "always-tests" => {
                let actual = parse_always_tests(&input["tests"]);
                assert_eq!(
                    actual.is_ok(),
                    expected.get("value").is_some(),
                    "{}",
                    case["id"]
                );
                if let Some(message) = expected["error"].as_str() {
                    let error = actual.unwrap_err();
                    match error {
                        AlwaysTestsError::Shape => assert_eq!(
                            message,
                            "always_tests must list repo-relative pytest files or node IDs"
                        ),
                        AlwaysTestsError::TooMany => {
                            assert_eq!(message, "always_tests accepts at most 20 entries")
                        }
                        AlwaysTestsError::NonString => {
                            assert_eq!(message, "always_tests must contain strings")
                        }
                        AlwaysTestsError::InvalidEntry(index) => {
                            assert!(
                                message.starts_with("not a repo-relative pytest file or node ID: ")
                            );
                            assert!(
                                NodeId::parse(
                                    input["tests"][index].as_str().unwrap(),
                                    Mode::AlwaysTest
                                )
                                .is_err()
                            );
                        }
                    }
                }
                counts[2] += 1;
            }
            op => panic!("unconsumed node-id operation {op}"),
        }
    }
    assert!(counts.into_iter().all(|count| count > 0));
    println!("node-id vector operation counts: {counts:?}");
}

/// #824 lead decision: flags preserve the existing mode differences.
#[test]
fn named_mode_rule_vectors() {
    for (rule, raw, accepted) in [
        ("require_names", "tests/test_a.py", [false, true, true]),
        (
            "reject_dot_path_segments",
            "./test_a.py::x",
            [true, true, false],
        ),
        (
            "reject_empty_path_segments",
            "tests//test_a.py::x",
            [true, true, false],
        ),
        (
            "reject_path_metacharacters:brackets",
            "tests/[x].py::x",
            [true, true, false],
        ),
        (
            "reject_path_metacharacters:star",
            "tests/test_*.py::x",
            [true, true, false],
        ),
        (
            "reject_path_metacharacters:question",
            "tests/test_?.py::x",
            [true, true, false],
        ),
        (
            "reject_path_metacharacters:colon",
            "tests/test:case.py::x",
            [true, true, false],
        ),
        (
            "reject_control_characters:C0",
            "tests/test_a.py::x\n",
            [true, true, false],
        ),
        (
            "reject_control_characters:DEL",
            "tests/test_a.py::x\u{7f}",
            [true, true, false],
        ),
        (
            "reject_control_characters:C1",
            "tests/test_a.py::x\u{9f}",
            [true, true, false],
        ),
        ("common:NUL", "tests/test_a.py::x\0", [false; 3]),
        ("common:backslash-path", "tests\\test_a.py::x", [false; 3]),
        (
            "opaque:backslash-label",
            "tests/test_a.py::x[a\\b]",
            [true; 3],
        ),
        (
            "opaque:bracket-separator",
            "tests/test_a.py::x[a::b]",
            [true; 3],
        ),
        (
            "opaque:empty-split",
            "tests/test_a.py::x[a::::b]",
            [false; 3],
        ),
        ("common:parent", "tests/../test_a.py::x", [false; 3]),
        ("unicode", "tests/test_é.py::test_𝛂[😀]", [true; 3]),
        (
            "unicode:outside-control-ranges",
            "tests/test_a.py::x[\u{200b}\u{2028}]",
            [true; 3],
        ),
    ] {
        for (mode, expected) in MODES.into_iter().zip(accepted) {
            assert_eq!(
                NodeId::parse(raw, mode).is_ok(),
                expected,
                "{rule}: {mode:?}"
            );
        }
    }
}

/// #824: pin every C0/C1 character at both path and parameter boundaries.
#[test]
fn every_control_rule_vector() {
    for code in (0u32..32).chain(127..160) {
        let control = char::from_u32(code).unwrap();
        for raw in [
            format!("tests/test_{control}.py::x"),
            format!("tests/test_a.py::x[{control}]"),
        ] {
            for mode in MODES {
                assert_eq!(
                    NodeId::parse(&raw, mode).is_ok(),
                    code != 0 && mode != Mode::AlwaysTest,
                    "control U+{code:04X}, {mode:?}"
                );
            }
        }
    }
}

/// #824: the trusted settings boundary preserves order, duplicates and error precedence.
#[test]
fn always_tests_boundary() {
    for (input, error) in [
        (Value::Null, AlwaysTestsError::Shape),
        (json!([1]), AlwaysTestsError::NonString),
        (json!(["bad.txt", 1]), AlwaysTestsError::InvalidEntry(0)),
        (json!(vec![1; 21]), AlwaysTestsError::TooMany),
    ] {
        assert_eq!(parse_always_tests(&input), Err(error));
    }
    for count in [0, 2, 20] {
        let input = json!(vec!["test_a.py"; count]);
        let nodes = parse_always_tests(&input).unwrap();
        assert_eq!(
            nodes.iter().map(|node| node.as_str()).collect::<Vec<_>>(),
            vec!["test_a.py"; count]
        );
    }
}

// Verbatim pre-extraction predicate from reproducing.rs at cdacce8f99.
fn legacy_exact_pytest_node_id(test: &str) -> bool {
    let mut parts = test.split("::");
    let path = parts.next().unwrap_or_default();
    let names = parts.collect::<Vec<_>>();
    !names.is_empty()
        && names.iter().all(|name| !name.is_empty())
        && path.ends_with(".py")
        && !path.starts_with(['/', '-'])
        && !path.split('/').any(|part| part == "..")
        && !path.contains('\\')
        && !test.contains('\0')
}

fn unicode_label() -> impl Strategy<Value = String> {
    prop::collection::vec(
        any::<char>().prop_filter("non-NUL non-colon", |ch| *ch != '\0' && *ch != ':'),
        0..128,
    )
    .prop_map(|chars| chars.into_iter().collect())
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        failure_persistence: None,
        rng_seed: proptest::test_runner::RngSeed::Fixed(824),
        .. ProptestConfig::default()
    })]

    /// #824/#717: parsing and joining preserve arbitrary admitted Unicode without normalization.
    #[test]
    fn unicode_round_trip(label in unicode_label()) {
        let raw = format!("tests/test_é.py::test_𝛂[{label}]");
        for mode in MODES {
            let parsed = NodeId::parse(&raw, mode);
            let strict_valid = !raw.chars().any(|ch| (ch as u32) < 32 || (127..=159).contains(&(ch as u32)));
            prop_assert_eq!(parsed.is_ok(), mode != Mode::AlwaysTest || strict_valid);
            if let Ok(node) = parsed {
                prop_assert_eq!(node.as_str(), raw.as_str());
                let joined = std::iter::once(node.path()).chain(node.names()).collect::<Vec<_>>().join("::");
                prop_assert_eq!(joined, raw.as_str());
                prop_assert_eq!(NodeId::parse(node.as_str(), mode), Ok(node));
            }
        }
    }

    /// #824: all forbidden controls, including every C1 value, obey each mode's rule.
    #[test]
    fn control_edges(code in prop_oneof![0u32..32, 127u32..160], in_path in any::<bool>()) {
        let control = char::from_u32(code).unwrap();
        let raw = if in_path { format!("tests/test_{control}.py::x") } else { format!("tests/test_a.py::x[{control}]") };
        for mode in MODES {
            prop_assert_eq!(NodeId::parse(&raw, mode).is_ok(), code != 0 && mode != Mode::AlwaysTest);
        }
    }

    /// #824: path backslashes and NUL cannot be hidden by Unicode parameter text.
    #[test]
    fn nul_and_backslash_edges(label in unicode_label()) {
        for mode in MODES {
            prop_assert!(NodeId::parse(&format!("test_a.py::x[{label}\0]"), mode).is_err(), "NUL in a label");
            prop_assert!(NodeId::parse(&format!("tests\\test_a.py::x[{label}]"), mode).is_err(), "backslash in a path");
        }
    }

    /// #824: :: inside parameter brackets retains literal split semantics.
    #[test]
    fn bracket_separators(label in "[a-zA-Z0-9😀]{0,64}") {
        for mode in MODES {
            let raw = format!("test_a.py::test_a[{label}::tail]");
            prop_assert!(NodeId::parse(&raw, mode).is_ok());
            prop_assert!(NodeId::parse(&format!("test_a.py::test_a[{label}::::tail]"), mode).is_err(), "empty :: component");
        }
    }

    /// #824: file collection expansion requires the :: boundary.
    #[test]
    fn file_prefix_boundaries(stem in "[a-z]{1,32}", label in "[a-z0-9]{0,64}") {
        let raw = format!("tests/test_{stem}.py");
        let requested = NodeId::parse(&raw, Mode::SelectedFile).unwrap();
        prop_assert!(requested.selects(&raw));
        prop_assert!(requested.selects(&format!("{raw}::test_a[{label}]")), "file :: boundary");
        for suffix in ["x::test_a", "_extra::test_a", "/other.py::test_a"] {
            prop_assert!(!requested.selects(&format!("{raw}{suffix}")), "file near miss");
        }
    }

    /// #824: function selection expands parameters once, and explicit labels stay exact.
    #[test]
    fn parameter_selection(label in "[a-z0-9]{0,64}") {
        let function = NodeId::parse("test_a.py::test_a", Mode::Exact).unwrap();
        let raw = format!("test_a.py::test_a[{label}]");
        prop_assert!(function.selects(&raw));
        prop_assert!(!function.selects(&format!("test_a.py::test_a_extra[{label}]")), "function near miss");
        let parameter = NodeId::parse(&raw, Mode::Exact).unwrap();
        prop_assert!(parameter.selects(&raw));
        prop_assert!(!parameter.selects(&format!("{raw}[nested]")), "explicit label stays exact");
    }

    /// #824/#717: the extracted exact parser agrees with the original predicate.
    #[test]
    fn exact_extraction_matches_reference(
        path in prop_oneof![
            Just("tests/test_a.py".to_owned()),
            Just("./test_a.py".to_owned()),
            Just("tests//test_a.py".to_owned()),
            Just("../test_a.py".to_owned()),
            Just("tests/[x].py".to_owned()),
            prop::collection::vec(any::<char>(), 0..64).prop_map(|chars| chars.into_iter().collect()),
        ],
        names in prop::collection::vec(
            prop::collection::vec(any::<char>(), 0..64).prop_map(|chars| chars.into_iter().collect::<String>()),
            0..5,
        ),
    ) {
        let raw = std::iter::once(path).chain(names).collect::<Vec<_>>().join("::");
        prop_assert_eq!(NodeId::parse(&raw, Mode::Exact).is_ok(), legacy_exact_pytest_node_id(&raw));
    }
}
