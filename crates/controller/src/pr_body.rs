//! A served repository's PR-body policy (#447): the lines its PR checks require.
use tl3_config::BodyLine;

/// The value TL3 declares for a rule, from the change's files.
pub fn value(rule: &BodyLine, _paths: &[String]) -> String {
    rule.default.clone()
}

/// One `label: value` line per rule.
pub fn lines(_rules: &[BodyLine], _paths: &[String]) -> String {
    String::new()
}

/// The rules a body doesn't satisfy with exactly one line holding an allowed value.
pub fn gaps(_body: &str, _rules: &[BodyLine]) -> Vec<String> {
    Vec::new()
}

/// The body with every unsatisfied rule fixed, or `None` when it already complies.
pub fn healed(_body: &str, _rules: &[BodyLine], _paths: &[String]) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change_type() -> BodyLine {
        BodyLine {
            label: "Change type".into(),
            values: [
                "bugfix", "build", "chore", "ci", "docs", "feature", "fix", "perf", "refactor",
                "revert", "test",
            ]
            .map(String::from)
            .to_vec(),
            default: "chore".into(),
        }
    }

    fn paths(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|path| (*path).to_owned()).collect()
    }

    #[test]
    fn the_value_follows_the_change_files() {
        let rule = change_type();
        for (files, expected) in [
            (&["tests/test_pool.py"][..], "test"),
            (
                &["tests/test_pool.py", "tests/conftest_helpers.py"][..],
                "test",
            ),
            (&["docs/merge_queue.md", "README.md"][..], "docs"),
            (
                &["src/prismabuild/pool.py", "tests/test_pool.py"][..],
                "fix",
            ),
            (&["src/prismabuild/pool.py"][..], "chore"),
            (&[][..], "chore"),
        ] {
            assert_eq!(value(&rule, &paths(files)), expected, "{files:?}");
        }
        // A value the repository doesn't allow falls back to its default.
        let mut narrow = change_type();
        narrow.values = vec!["chore".into(), "feature".into()];
        assert_eq!(value(&narrow, &paths(&["tests/test_pool.py"])), "chore");
    }

    #[test]
    fn lines_render_one_label_per_rule() {
        let rules = [change_type()];
        assert_eq!(
            lines(&rules, &paths(&["src/a.py", "tests/test_a.py"])),
            "Change type: fix\n"
        );
        assert_eq!(lines(&[], &paths(&["src/a.py"])), "");
    }

    #[test]
    fn gaps_follow_the_served_repositorys_own_rule() {
        // PrismaBuild's check_regression_test.py: exactly one line, case-insensitive.
        let rules = [change_type()];
        for (body, gap) in [
            ("Refs #2630.\n\n<!-- tl3:effect:x -->", true),
            ("Change type: fix\nRefs #2630.", false),
            ("Refs #2630.\n  change TYPE:  Fix  \n", false),
            ("Change type: fix\nChange type: test\n", true),
            ("Change type: mystery\n", true),
            ("Change type:\n", true),
        ] {
            assert_eq!(gaps(body, &rules).is_empty(), !gap, "{body:?}");
        }
        assert_eq!(gaps("Refs #1.", &rules), ["Change type"]);
    }

    #[test]
    fn healing_prismabuild_2632_adds_the_line_once() {
        let rules = [change_type()];
        let body = "Refs #2630.\n\n<!-- tl3:effect:6566666563743a39373a39 -->";
        let files = paths(&["src/prismabuild/pool.py", "tests/test_pool.py"]);
        let healed = healed(body, &rules, &files).expect("the body lacks Change type");
        assert_eq!(
            healed,
            "Refs #2630.\nChange type: fix\n\n<!-- tl3:effect:6566666563743a39373a39 -->"
        );
        assert!(gaps(&healed, &rules).is_empty());
        // Idempotent: a healed body needs nothing more.
        assert_eq!(super::healed(&healed, &rules, &files), None);
        // An invalid declaration is replaced, not duplicated.
        let fixed = super::healed("Refs #1.\nChange type: mystery\n", &rules, &files).unwrap();
        assert!(gaps(&fixed, &rules).is_empty(), "{fixed}");
        assert!(!fixed.contains("mystery"), "{fixed}");
    }
}
