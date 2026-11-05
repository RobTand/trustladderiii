const SELECTED_CHECK: &str = "Existing tests that import the change";
pub(crate) fn selected_failures<'a>(
    previous: impl IntoIterator<Item = &'a serde_json::Value>,
    results: &[serde_json::Value],
) -> Vec<String> {
    let mut failures = std::collections::BTreeSet::new();
    for report in previous {
        failures.extend(
            report["selected_failures"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|test| test.as_str().map(str::to_owned)),
        );
    }
    for result in results.iter().filter(|r| r["name"] == SELECTED_CHECK) {
        failures.extend(
            result["tests"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|test| test["passed"] == false)
                .filter_map(|test| test["test"].as_str().map(str::to_owned)),
        );
    }
    failures.into_iter().collect()
}
