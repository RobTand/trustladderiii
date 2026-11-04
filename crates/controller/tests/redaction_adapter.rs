//! #753 security review: run the production regex translator against Python predicates.
#[path = "../src/redaction/compatibility.rs"]
mod compatibility;

use fancy_regex::Regex;
use serde_json::{Value, json};
use std::{collections::BTreeSet, process::Command, sync::LazyLock};

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

fn cases() -> &'static Value {
    static DATA: LazyLock<Value> = LazyLock::new(|| {
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
        let output = command.args(["-I", "-c", "import sys; sys.path.insert(0,sys.argv.pop(1)); import redaction_branches; redaction_branches.main()"])
            .arg(root.join("../../workers/tests"))
            .arg(root.join("src/redaction/policy.json"))
            .output().unwrap();
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
    for case in cases()[section].as_array().unwrap() {
        let translated =
            compatibility::translate(case["pattern"].as_str().unwrap(), &classes).unwrap();
        let pattern = match case["mode"].as_str().unwrap() {
            "full" => format!(r"\A(?:{translated})\z"),
            "match" => format!(r"\A(?:{translated})"),
            _ => translated,
        };
        let regex = Regex::new(&pattern).unwrap();
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
