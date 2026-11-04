//! #753 re-review: parsed-class membership and corpus-derived production-redactor conformance.
#[path = "../src/redaction/compatibility.rs"]
mod compatibility;
mod redaction_support;

use fancy_regex::Regex;
use redaction_support::{python_command, python_redact};
use serde_json::Value;
use std::{collections::HashMap, sync::LazyLock};

fn data() -> &'static Value {
    static DATA: LazyLock<Value> = LazyLock::new(|| {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let output = python_command()
            .args(["-I", "-c", "import sys; sys.path.insert(0,sys.argv.pop(1)); import redaction_generated; redaction_generated.main()"])
            .arg(root.join("../../workers/tests"))
            .arg(root.join("src/redaction/policy.json"))
            .arg(root.join("src/redaction/matrix.json"))
            .arg(root.join("src/redaction/corpus.json"))
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

fn classes() -> [String; 3] {
    let policy: Value = serde_json::from_str(include_str!("../src/redaction/policy.json")).unwrap();
    ["alnum", "decimal", "space"].map(|name| {
        let mut body = if name == "alnum" {
            "_".to_owned()
        } else {
            String::new()
        };
        for range in policy["characters"][name].as_str().unwrap().split(',') {
            let (start, end) = range.split_once('-').unwrap();
            body.push_str(&format!(r"\x{{{start}}}-\x{{{end}}}"));
        }
        body
    })
}

fn check(shard: usize) {
    assert!(redaction_support::shared_inputs().len() > 700);
    let data = data();
    assert!(data["class_occurrences"].as_u64().unwrap() > 200);
    let cases = &data["shards"][shard];
    let classes = classes();
    let mut cache = HashMap::new();
    let records = cases["classes"].as_array().unwrap();
    assert!(!records.is_empty());
    for case in records {
        let pattern = case["pattern"].as_str().unwrap();
        let regex = cache.entry(pattern).or_insert_with(|| {
            let pattern = compatibility::translate(pattern, &classes).unwrap();
            Regex::new(&format!(r"\A(?:{pattern})\z")).unwrap()
        });
        for (text, expected) in case["texts"]
            .as_array()
            .unwrap()
            .iter()
            .zip(case["expected"].as_array().unwrap())
        {
            assert_eq!(
                regex.is_match(text.as_str().unwrap()).unwrap(),
                expected.as_bool().unwrap(),
                "{}: {pattern} {text}",
                case["label"]
            );
        }
    }
    let inputs: Vec<String> = serde_json::from_value(cases["inputs"].clone()).unwrap();
    assert!(inputs.len() > 100);
    for (input, expected) in inputs.iter().zip(python_redact(&inputs)) {
        assert_eq!(
            tl3_controller::redaction::text(input).unwrap(),
            expected,
            "{input:?}"
        );
    }
    println!(
        "shard {shard}: {} class/flag records, {} full-redactor inputs",
        records.len(),
        inputs.len()
    );
}

macro_rules! shard {
    ($name:ident, $index:expr) => {
        /// #753: each admitted Bazel shard compares both engines and retains every class occurrence.
        #[test]
        fn $name() {
            check($index);
        }
    };
}
shard!(generated_0, 0);
shard!(generated_1, 1);
shard!(generated_2, 2);
shard!(generated_3, 3);
shard!(generated_4, 4);
shard!(generated_5, 5);
shard!(generated_6, 6);
shard!(generated_7, 7);
