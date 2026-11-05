use proptest::prelude::*;
use serde_json::{Value, json};
use tl3_plan_check::{
    heuristics::{asks_to_edit, mentions},
    paths::full_match,
    subjects::{overlaps, subject_covers},
};

fn corpus() -> &'static Value {
    static CORPUS: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    CORPUS.get_or_init(|| serde_json::from_str(include_str!("fixtures/generated.json")).unwrap())
}
fn check_glob(case: &Value) {
    assert_eq!(
        full_match(
            case["path"].as_str().unwrap(),
            case["pattern"].as_str().unwrap()
        ),
        case["verdict"].as_bool().unwrap(),
        "{case}"
    );
}
fn check_english(case: &Value) {
    let found = mentions(
        case["description"].as_str().unwrap(),
        case["test"].as_str().unwrap(),
    );
    assert_eq!(json!(found), case["mentions"], "{case}");
    assert_eq!(
        found.iter().any(|(b, a)| asks_to_edit(b, a)),
        case["verdict"].as_bool().unwrap(),
        "{case}"
    );
}
#[test]
fn all_python_generated_globs() {
    for case in corpus()["globs"].as_array().unwrap() {
        check_glob(case);
    }
}
#[test]
fn all_python_generated_english() {
    for case in corpus()["english"].as_array().unwrap() {
        check_english(case);
    }
}
#[test]
fn all_python_generated_subjects() {
    for case in corpus()["subjects"].as_array().unwrap() {
        let chosen = case["chosen"].as_str().unwrap();
        let subject = case["subject"].as_str().unwrap();
        assert_eq!(
            subject_covers(chosen, subject),
            case["covers"].as_bool().unwrap(),
            "{case}"
        );
        assert_eq!(
            overlaps(chosen, subject),
            case["overlaps"].as_bool().unwrap(),
            "{case}"
        );
    }
}
proptest! {
    #[test]
    fn python_glob_vectors(index in 0usize..9336) {check_glob(&corpus()["globs"][index]);}
    #[test]
    fn python_english_vectors(index in 0usize..4000) {check_english(&corpus()["english"][index]);}
    #[test]
    fn literal_globs_roundtrip(path in "[a-zé中][a-zé中_]{0,25}(/[a-zé中_]{1,20}){0,3}") {prop_assert!(full_match(&path,&path));}
    #[test]
    fn component_star_never_crosses_separator(a in "[a-z]{1,20}",b in "[a-z]{1,20}") {
        prop_assert!(!full_match(&[a.as_str(), b.as_str()].join("/"),"*"));
        prop_assert!(full_match(&[a.as_str(), b.as_str()].join("/"),"**"));
    }
    #[test]
    fn subject_transitivity(a in "[a-zé_]{0,20}",b in "[a-zé_]{0,20}",c in "[a-zé_]{0,20}") {
        let b=format!("{a}::{b}");let c=format!("{b}::{c}");
        prop_assert!(subject_covers(&a,&b));prop_assert!(subject_covers(&b,&c));prop_assert!(subject_covers(&a,&c));
    }
}
