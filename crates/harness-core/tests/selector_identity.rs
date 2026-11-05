use tl3_harness_core::identity::{Selection, selection_matches, selector_identity};
use tl3_harness_core::summary::PythonText;

#[test]
fn empty_selection_hashes_the_compact_python_array() {
    let observed = selector_identity(&[]);
    assert_eq!(observed.schema, "tl3.selectors.v1");
    assert_eq!(observed.count, 0);
    assert_eq!(
        observed.sha256,
        "4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945"
    );
}

#[test]
fn lone_surrogates_are_valid_python_identity_input() {
    let input = PythonText::try_from(vec![120, 0xd800, 121]).unwrap();
    let identity = selector_identity(std::slice::from_ref(&input));
    assert_eq!(identity.prefix, vec![input]);
    assert_eq!(identity.count, 1);
}

#[test]
fn order_duplicates_and_the_unshown_tail_are_part_of_identity() {
    let first = PythonText::from("test_x.py::test_first");
    let second = PythonText::from("test_x.py::test_second");
    assert_ne!(
        selector_identity(&[first.clone(), second.clone()]),
        selector_identity(&[second.clone(), first.clone()])
    );
    assert_ne!(
        selector_identity(std::slice::from_ref(&first)),
        selector_identity(&[first.clone(), first])
    );
    let mut selected = vec![second.clone(); 4];
    let identity = selector_identity(&selected);
    selected[3] = PythonText::from("test_x.py::test_changed_tail");
    assert_eq!(identity.prefix, selector_identity(&selected).prefix);
    assert!(!selection_matches(
        &Selection::Identity(identity),
        &selected
    ));
}

#[test]
fn matching_checks_every_identity_field() {
    let selected = vec![PythonText::from("test_x.py::test_first")];
    let identity = selector_identity(&selected);
    assert!(selection_matches(
        &Selection::Identity(identity.clone()),
        &selected
    ));
    assert!(selection_matches(
        &Selection::Full(selected.clone()),
        &selected
    ));
    let mut altered = identity;
    altered.schema.push_str(".untrusted");
    assert!(!selection_matches(&Selection::Identity(altered), &selected));
    assert!(!selection_matches(&Selection::Invalid, &selected));
}

fn text(value: &serde_json::Value) -> PythonText {
    if let Some(value) = value.as_str() {
        PythonText::from(value)
    } else {
        PythonText::try_from(
            value["python_codepoints"]
                .as_array()
                .unwrap()
                .iter()
                .map(|point| point.as_u64().unwrap() as u32)
                .collect::<Vec<_>>(),
        )
        .unwrap()
    }
}

#[test]
fn all_shared_python_identity_vectors() {
    use tl3_harness_core::identity::SelectorIdentity;
    let relative = "harness/test-vectors/core/selector-identity.json";
    let path = [
        std::path::PathBuf::from(relative),
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(relative),
    ]
    .into_iter()
    .find(|path| path.exists())
    .expect("shared identity vectors staged");
    let corpus: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let cases = corpus["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 14);
    for case in cases {
        let selected = case["input"]["selectors"]
            .as_array()
            .unwrap()
            .iter()
            .map(text)
            .collect::<Vec<_>>();
        let expected = &case["expected"]["identity"];
        let expected = SelectorIdentity {
            schema: expected["schema"].as_str().unwrap().to_owned(),
            count: expected["count"].as_u64().unwrap() as usize,
            sha256: expected["sha256"].as_str().unwrap().to_owned(),
            prefix: expected["prefix"]
                .as_array()
                .unwrap()
                .iter()
                .map(text)
                .collect(),
        };
        assert_eq!(selector_identity(&selected), expected, "{}", case["id"]);
        let value = &case["expected"]["candidate"];
        let candidate = if let Some(values) = value.as_array() {
            Selection::Full(values.iter().map(text).collect())
        } else if let Some(count) = value["count"].as_u64() {
            Selection::Identity(SelectorIdentity {
                schema: value["schema"].as_str().unwrap().to_owned(),
                count: count as usize,
                sha256: value["sha256"].as_str().unwrap().to_owned(),
                prefix: value["prefix"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(text)
                    .collect(),
            })
        } else {
            Selection::Invalid
        };
        assert_eq!(
            selection_matches(&candidate, &selected),
            case["expected"]["matches"].as_bool().unwrap(),
            "{}",
            case["id"]
        );
    }
}

proptest::proptest! {
    /// #704/#723: a prefix cannot hide changes to any one exact identity field.
    #[test]
    fn any_changed_identity_field_is_rejected(points in proptest::collection::vec(0u32..=0x10ffff, 0..400), field in 0u8..4) {
        let selected = vec![PythonText::try_from(points).unwrap()];
        let mut candidate = selector_identity(&selected);
        match field {
            0 => candidate.schema.push('x'),
            1 => candidate.count += 1,
            2 => candidate.sha256.push('0'),
            _ => candidate.prefix.push(PythonText::from("unexpected")),
        }
        proptest::prop_assert!(!selection_matches(&Selection::Identity(candidate), &selected));
    }
}
