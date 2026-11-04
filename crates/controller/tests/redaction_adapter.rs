//! #753 security review: run the production regex translator against Python predicates.
#[path = "../src/redaction/compatibility.rs"]
mod compatibility;

use fancy_regex::Regex;
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, HashMap},
    process::Command,
    sync::LazyLock,
};

fn policy() -> Value {
    serde_json::from_str(include_str!("../src/redaction/policy.json")).unwrap()
}

fn classes() -> [String; 3] {
    let policy = policy();
    ["alnum", "decimal", "space"].map(|name| {
        let mut class = if name == "alnum" {
            "_".to_owned()
        } else {
            String::new()
        };
        for range in policy["characters"][name].as_str().unwrap().split(',') {
            let (start, end) = range.split_once('-').unwrap();
            class.push_str(&format!(r"\x{{{start}}}-\x{{{end}}}"));
        }
        class
    })
}

fn oracle(path: &std::path::Path) -> std::process::Output {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut command = Command::new(root.join("../../workers/.venv/bin/python"));
    command.env_clear();
    for key in [
        "PATH",
        "HOME",
        "TMPDIR",
        "TEST_TMPDIR",
        "TEST_SRCDIR",
        "TEST_WORKSPACE",
        "RUNFILES_DIR",
        "RUNFILES_MANIFEST_FILE",
        "JAVA_RUNFILES",
        "PYTHON_RUNFILES",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command.args(["-I", "-c", "import sys; sys.path.insert(0,sys.argv.pop(1)); import redaction_branches; redaction_branches.main()"])
        .arg(root.join("../../workers/tests")).arg(path).output().unwrap()
}

fn cases() -> &'static Value {
    static DATA: LazyLock<Value> = LazyLock::new(|| {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let output = oracle(&root.join("src/redaction/policy.json"));
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    });
    &DATA
}

fn check_predicates(section: &str) {
    let classes = classes();
    let mut failures = Vec::new();
    let mut cache = HashMap::new();
    for case in cases()[section].as_array().unwrap() {
        let key = (
            case["pattern"].as_str().unwrap(),
            case["mode"].as_str().unwrap(),
        );
        let regex = cache.entry(key).or_insert_with(|| {
            let translated = compatibility::translate(key.0, &classes).unwrap();
            let pattern = match key.1 {
                "full" => format!(r"\A(?:{translated})\z"),
                "match" => format!(r"\A(?:{translated})"),
                _ => translated,
            };
            Regex::new(&pattern).unwrap()
        });
        let capture = regex.captures(case["text"].as_str().unwrap()).unwrap();
        let actual = capture.map_or(Value::Null, |capture| {
            let mut result = serde_json::Map::new();
            result.insert("0".into(), json!(capture.get(0).unwrap().as_str()));
            for name in regex.capture_names().flatten() {
                result.insert(
                    name.into(),
                    json!(capture.name(name).map(|value| value.as_str())),
                );
            }
            Value::Object(result)
        });
        if actual != case["expected"] {
            failures.push(case["label"].as_str().unwrap().to_owned());
        }
    }
    assert!(
        failures.is_empty(),
        "Python predicate mismatches: {failures:?}"
    );
}

/// #753 security review: all nine named rules and affected pattern/check expressions fold like Python.
#[test]
fn case_folding_matches_python() {
    check_predicates("folding");
}

/// #753 security review: strict \Z and multiline/scoped $ honor Python newline boundaries.
#[test]
fn anchors_match_python() {
    check_predicates("anchors");
}

/// #753 security review: no provider or other pattern alternative can remain unexercised.
#[test]
fn every_pattern_alternative_is_hit_in_both_engines() {
    let classes = classes();
    let mut failures = Vec::new();
    for case in cases()["coverage"].as_array().unwrap() {
        assert_eq!(
            case["matched"], true,
            "unexercised pattern {}",
            case["index"]
        );
        let required = case["branches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|name| name.as_str().unwrap())
            .collect::<BTreeSet<_>>();
        let witnesses = case["witnesses"].as_object().unwrap();
        let python = witnesses
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        let regex = Regex::new(
            &compatibility::translate(case["pattern"].as_str().unwrap(), &classes).unwrap(),
        )
        .unwrap();
        let mut rust = BTreeSet::new();
        for text in witnesses.values() {
            for capture in regex.captures_iter(text.as_str().unwrap()) {
                let capture = capture.unwrap();
                for name in &required {
                    if capture.name(name).is_some() {
                        rust.insert(*name);
                    }
                }
            }
        }
        if python != required || rust != required {
            failures.push(format!(
                "pattern {}: Python missing {:?}; Rust missing {:?}",
                case["index"],
                required.difference(&python).collect::<Vec<_>>(),
                required.difference(&rust).collect::<Vec<_>>()
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// #753 re-review: unsupported class constructs must stop generation, without a text fallback.
#[test]
fn unsupported_class_syntax_is_rejected() {
    for pattern in [
        r"[a&&b]",
        r"[a--b]",
        r"[a[b]]",
        r"[\p{Greek}]",
        r"(?i)(a)\1",
        r"x\z",
    ] {
        assert!(
            compatibility::translate(pattern, &classes()).is_err(),
            "{pattern}"
        );
    }
}

macro_rules! grammar_regression {
    ($test:ident, $pattern:expr, $name:expr) => {
        /// #788: reject unsupported Python syntax before fancy-regex can silently rewrite it.
        #[test]
        fn $test() {
            let result = compatibility::translate($pattern, &classes());
            assert!(
                result.is_err(),
                "{} unexpectedly translated: {result:?}",
                $name
            );
            assert!(
                result.unwrap_err().contains($name),
                "missing named grammar error"
            );
        }
    };
}
grammar_regression!(
    named_unicode_escape_rejected,
    r"\N{LATIN SMALL LETTER I}",
    "named Unicode escape"
);
grammar_regression!(
    verbose_vertical_tab_rejected,
    "(?x:a\u{b}b)",
    "literal U+000B"
);
grammar_regression!(verbose_form_feed_rejected, "(?x:a\u{c}b)", "literal U+000C");
grammar_regression!(non_python_h_escape_rejected, r"\h", "escape \\h");
grammar_regression!(non_python_e_escape_rejected, r"\e", "escape \\e");
grammar_regression!(bare_n_escape_rejected, r"\N", "escape \\N");
grammar_regression!(non_python_ungreedy_flag_rejected, r"(?U)a+", "flag U");
grammar_regression!(brace_hex_escape_rejected, r"\x{69}", "hex escape \\x");

/// #788: CI must compile every policy expression with Python, including unused helpers.
#[test]
fn python_oracle_rejects_an_invalid_unused_helper() {
    let mut policy = policy();
    policy["helpers"]["HEX_PREFIX"] = json!(r"\h");
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), serde_json::to_vec(&policy).unwrap()).unwrap();
    let output = oracle(file.path());
    assert!(
        !output.status.success(),
        "Python oracle accepted helpers:HEX_PREFIX containing \\h"
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("helpers:HEX_PREFIX"));
}

/// #788: the Python compilation check must enumerate all 90 policy expressions.
#[test]
fn every_policy_expression_compiles_with_python() {
    let policy = policy();
    let mut required = BTreeSet::new();
    for section in ["regex", "helpers", "patterns", "checked"] {
        if let Some(entries) = policy[section].as_object() {
            required.extend(entries.keys().map(|key| format!("{section}:{key}")));
        } else {
            required.extend(
                (0..policy[section].as_array().unwrap().len())
                    .map(|index| format!("{section}:{index}")),
            );
        }
    }
    let compiled = cases()["compiled"]
        .as_array()
        .expect("Python oracle must compile every expression")
        .iter()
        .map(|name| name.as_str().unwrap().to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(required.len(), 90);
    assert_eq!(compiled, required);
}
