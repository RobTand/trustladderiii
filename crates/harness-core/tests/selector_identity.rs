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
