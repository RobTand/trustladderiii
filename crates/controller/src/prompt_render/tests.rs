use super::render;
use proptest::prelude::*;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
struct Vector {
    kind: String,
    document: String,
    expected: String,
}

fn vectors() -> Vec<Vector> {
    serde_json::from_str(include_str!("vectors.json")).unwrap()
}

/// #819: system text, shared prefixes and cache identities must remain byte-identical.
#[test]
fn python_prompt_identity_vectors() {
    let rows = vectors();
    assert!(rows.len() > 100);
    for row in rows {
        let actual = render(&row.kind, &row.document).unwrap();
        assert_eq!(actual, row.expected, "{row:?}");
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]
    /// #819: compose real-Python number/string cases under randomly ordered nested objects.
    #[test]
    fn python_serializer_corpus_composes(indices in prop::collection::vec(0usize..512, 0..24)) {
        let rows: Vec<_> = vectors().into_iter().filter(|v| v.kind == "serialize" && serde_json::from_str::<serde_json::Value>(&v.expected).is_ok()).collect();
        let selected: Vec<_> = indices.iter().map(|i| &rows[i % rows.len()]).collect();
        let document = format!("[{}]", selected.iter().map(|r| r.document.as_str()).collect::<Vec<_>>().join(","));
        let expected = format!("[{}]", selected.iter().map(|r| serde_json::from_str::<serde_json::Value>(&r.expected).unwrap()["text"].as_str().unwrap().to_owned()).collect::<Vec<_>>().join(", "));
        let actual = render("serialize", &document).unwrap();
        let actual: serde_json::Value = serde_json::from_str(&actual).unwrap();
        prop_assert_eq!(actual["text"].as_str().unwrap(), expected);
    }
}
