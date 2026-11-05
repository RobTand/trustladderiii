use serde_json::{Value, json};
use tl3_plan_check::{
    citations::{Memory, MemoryFlag, check_citations},
    heuristics::{asks_to_edit, mentions},
    paths::full_match,
    prerequisites::{Prerequisite, check_prerequisites},
    subjects::{overlaps, subject_covers},
};

fn unpack(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(unpack).collect()),
        Value::Object(map) if map.contains_key("$tuple") => unpack(&map["$tuple"]),
        Value::Object(map) if map.contains_key("$dict") => Value::Object(
            map["$dict"]
                .as_array()
                .unwrap()
                .iter()
                .map(|pair| (pair[0].as_str().unwrap().to_owned(), unpack(&pair[1])))
                .collect(),
        ),
        _ => value.clone(),
    }
}
fn corpus() -> Value {
    serde_json::from_str(include_str!("fixtures/p2a.json")).unwrap()
}
fn list(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or(&[])
}
fn text(value: &Value) -> &str {
    value.as_str().unwrap_or("")
}

#[test]
fn p2a_globs() {
    let corpus = corpus();
    assert_eq!(list(&corpus["globs"]).len(), 26);
    for case in list(&corpus["globs"]) {
        assert_eq!(
            full_match(text(&case["path"]), text(&case["pattern"])),
            case["verdict"].as_bool().unwrap(),
            "{}",
            case["id"]
        );
    }
}

#[test]
fn p2a_english() {
    let corpus = corpus();
    assert_eq!(list(&corpus["english"]).len(), 50);
    for case in list(&corpus["english"]) {
        let found = mentions(text(&case["description"]), text(&case["test"]));
        assert_eq!(json!(found), unpack(&case["mentions"]), "{}", case["id"]);
        assert_eq!(
            found
                .iter()
                .any(|(before, after)| asks_to_edit(before, after)),
            case["verdict"].as_bool().unwrap(),
            "{}",
            case["id"]
        );
    }
}

#[test]
fn p2a_prerequisites() {
    let corpus = corpus();
    let mut count = 0;
    for case in list(&corpus["cases"])
        .iter()
        .filter(|c| c["operation"] == "prerequisites.check_prerequisites")
    {
        let args = unpack(&case["args"]);
        let issue = &args[0]["issue"];
        let mut sources = vec![text(&issue["body"]), text(&issue["title"])];
        sources.extend(list(&issue["comments"]).iter().map(|c| text(&c["body"])));
        let prerequisites = list(&args[1]["merge_prerequisites"])
            .iter()
            .map(|p| Prerequisite {
                kind: text(&p["kind"]),
                number: p["number"].as_u64().unwrap(),
                source_quote: text(&p["source_quote"]),
            })
            .collect::<Vec<_>>();
        let result = check_prerequisites(&sources, &prerequisites);
        assert_eq!(
            result.err(),
            case["outcome"]["error"]["message"].as_str(),
            "{}",
            case["id"]
        );
        count += 1;
    }
    assert_eq!(count, 46);
}

#[test]
fn p2a_citations() {
    let corpus = corpus();
    let mut count = 0;
    for case in list(&corpus["cases"])
        .iter()
        .filter(|c| c["operation"] == "recall.check_citations")
    {
        let args = unpack(&case["args"]);
        let memories = list(&args[0]["memories"])
            .iter()
            .map(|m| Memory {
                id: text(&m["id"]),
                content: text(&m["content"]),
            })
            .collect::<Vec<_>>();
        let cited = list(&args[1]["memory_ids"])
            .iter()
            .map(text)
            .collect::<Vec<_>>();
        let flags = list(&args[1]["memory_flags"])
            .iter()
            .map(|f| MemoryFlag {
                id: text(&f["id"]),
                claim: text(&f["claim"]),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            check_citations(&memories, &cited, &flags).err(),
            case["outcome"]["error"]["message"].as_str(),
            "{}",
            case["id"]
        );
        count += 1;
    }
    assert_eq!(count, 45);
}

#[test]
fn subject_boundaries_and_transitivity() {
    for (chosen, subject, expected) in [
        ("a.py", "a.py", true),
        ("a.py", "a.py::C::f", true),
        ("a.py::C", "a.py::Class", false),
        ("a.py", "ab.py", false),
        ("", "::f", true),
        ("a.py::C::f", "a.py::C", false),
    ] {
        assert_eq!(subject_covers(chosen, subject), expected);
        assert_eq!(
            overlaps(chosen, subject),
            subject_covers(chosen, subject) || subject_covers(subject, chosen)
        );
    }
    assert!(subject_covers("a.py", "a.py::C::f"));
}
