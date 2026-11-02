//! A served repository's PR-body policy (#447): the lines its PR checks require.
use tl3_config::BodyLine;

fn is_test(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    path.split('/')
        .any(|part| part == "tests" || part == "test")
        || name.starts_with("test_")
        || name.ends_with("_test.py")
}

fn is_doc(path: &str) -> bool {
    path.starts_with("docs/") || path.ends_with(".md") || path.ends_with(".rst")
}

/// The value a request's own text instructs for a rule, such as PrismaBuild's
/// "exactly one `Change type: feature` line" under `## PR rules`.
pub fn instructed(_rule: &BodyLine, _request: &str) -> Option<String> {
    None
}

/// The value TL3 declares for a rule. The request's own instruction comes
/// first. Failing that, it follows the change's files: only tests is `test`,
/// only docs is `docs`, and a change with its own test is `fix`, since TL3's
/// demonstrating test is the regression test such policies ask for. A value
/// the repository doesn't allow falls back to its default.
pub fn value(rule: &BodyLine, _request: Option<&str>, paths: &[String]) -> String {
    let candidate = if paths.is_empty() {
        None
    } else if paths.iter().all(|path| is_test(path)) {
        Some("test")
    } else if paths.iter().all(|path| is_doc(path)) {
        Some("docs")
    } else if paths.iter().any(|path| is_test(path)) {
        Some("fix")
    } else {
        None
    };
    candidate
        .filter(|value| rule.values.iter().any(|allowed| allowed == value))
        .map_or_else(|| rule.default.clone(), str::to_owned)
}

/// One `label: value` line per rule.
pub fn lines(rules: &[BodyLine], request: Option<&str>, paths: &[String]) -> String {
    rules
        .iter()
        .map(|rule| format!("{}: {}\n", rule.label, value(rule, request, paths)))
        .collect()
}

/// The value a body line declares for this rule's label, if it is one.
fn declared<'a>(line: &'a str, rule: &BodyLine) -> Option<&'a str> {
    let (label, value) = line.trim_start().split_once(':')?;
    label
        .trim_end()
        .eq_ignore_ascii_case(&rule.label)
        .then(|| value.trim())
}

/// The rules a body doesn't satisfy with exactly one line holding an allowed
/// value, matched as the repository's own check does: case-insensitively.
pub fn gaps(body: &str, rules: &[BodyLine]) -> Vec<String> {
    rules
        .iter()
        .filter(|rule| {
            let values: Vec<_> = body
                .lines()
                .filter_map(|line| declared(line, rule))
                .collect();
            !(values.len() == 1
                && rule
                    .values
                    .iter()
                    .any(|allowed| allowed.eq_ignore_ascii_case(values[0])))
        })
        .map(|rule| rule.label.clone())
        .collect()
}

/// The body with every unsatisfied rule fixed, or `None` when it already
/// complies. Invalid declarations go, and the new lines follow `Refs #N.`.
pub fn healed(
    body: &str,
    rules: &[BodyLine],
    request: Option<&str>,
    paths: &[String],
) -> Option<String> {
    let gaps = gaps(body, rules);
    if gaps.is_empty() {
        return None;
    }
    let broken: Vec<_> = rules
        .iter()
        .filter(|rule| gaps.contains(&rule.label))
        .collect();
    let kept: Vec<&str> = body
        .split_inclusive('\n')
        .filter(|line| !broken.iter().any(|rule| declared(line, rule).is_some()))
        .collect();
    let added: String = broken
        .iter()
        .map(|rule| format!("{}: {}", rule.label, value(rule, request, paths)))
        .collect::<Vec<_>>()
        .join("\n");
    let mut text = String::new();
    let mut inserted = false;
    for line in kept {
        text.push_str(line);
        if !inserted && line.trim_start().starts_with("Refs #") {
            if !line.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(&added);
            if line.ends_with('\n') {
                text.push('\n');
            }
            inserted = true;
        }
    }
    if !inserted {
        text = format!("{added}\n{text}");
    }
    Some(text)
}

#[cfg(test)]
#[path = "pr_body/pb2626.rs"]
mod pb2626;

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
            assert_eq!(value(&rule, None, &paths(files)), expected, "{files:?}");
        }
        // A value the repository doesn't allow falls back to its default.
        let mut narrow = change_type();
        narrow.values = vec!["chore".into(), "feature".into()];
        assert_eq!(
            value(&narrow, None, &paths(&["tests/test_pool.py"])),
            "chore"
        );
    }

    #[test]
    fn the_issues_own_instruction_comes_before_the_files() {
        // #447 review: PrismaBuild #2626's `## PR rules` names `Change type: feature`.
        let rule = change_type();
        let feature = paths(&["src/prismabuild/gpu.py", "tests/test_gpu.py"]);
        let issue = super::pb2626::PB2626_ISSUE_BODY;
        assert_eq!(instructed(&rule, issue).as_deref(), Some("feature"));
        assert_eq!(value(&rule, Some(issue), &feature), "feature");
        assert_eq!(
            lines(&[rule.clone()], Some(issue), &feature),
            "Change type: feature\n"
        );
        // Without a `PR rules` section, the whole text counts; case is the rule's.
        assert_eq!(
            instructed(&rule, "Use `change TYPE: Docs` for this one.").as_deref(),
            Some("docs")
        );
        // Under a `PR rules` section, text outside it doesn't count.
        let sectioned = "Earlier work used Change type: fix.\n\n## PR rules\n\n- Change type: perf\n\n## Notes\n\nChange type: test\n";
        assert_eq!(instructed(&rule, sectioned).as_deref(), Some("perf"));
        // No value, a value the repository refuses, or two values: the files decide.
        for text in [
            "The PR body has exactly one `Change type:` line.",
            "Change type: <type>",
            "Change type: mystery",
            "## PR rules\n- Change type: fix or Change type: feature\n",
        ] {
            assert_eq!(instructed(&rule, text), None, "{text:?}");
            assert_eq!(value(&rule, Some(text), &feature), "fix", "{text:?}");
        }
    }

    #[test]
    fn lines_render_one_label_per_rule() {
        let rules = [change_type()];
        assert_eq!(
            lines(&rules, None, &paths(&["src/a.py", "tests/test_a.py"])),
            "Change type: fix\n"
        );
        assert_eq!(lines(&[], None, &paths(&["src/a.py"])), "");
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
        let healed = healed(body, &rules, None, &files).expect("the body lacks Change type");
        assert_eq!(
            healed,
            "Refs #2630.\nChange type: fix\n\n<!-- tl3:effect:6566666563743a39373a39 -->"
        );
        assert!(gaps(&healed, &rules).is_empty());
        // Idempotent: a healed body needs nothing more.
        assert_eq!(super::healed(&healed, &rules, None, &files), None);
        // An invalid declaration is replaced, not duplicated.
        let fixed =
            super::healed("Refs #1.\nChange type: mystery\n", &rules, None, &files).unwrap();
        assert!(gaps(&fixed, &rules).is_empty(), "{fixed}");
        assert!(!fixed.contains("mystery"), "{fixed}");
    }
}
