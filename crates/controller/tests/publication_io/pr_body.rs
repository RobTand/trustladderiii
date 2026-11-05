use crate::publication_failure::{LocalRefusal, Reason, RuleCategory, RuleFailure};
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
pub fn instructed(rule: &BodyLine, request: &str) -> Option<String> {
    let text = pr_rules(request).unwrap_or(request).to_lowercase();
    let label = format!("{}:", rule.label.to_lowercase());
    let mut named: Vec<&String> = Vec::new();
    for (at, _) in text.match_indices(&label) {
        let token: String = text[at + label.len()..]
            .trim_start_matches([' ', '\t'])
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
            .collect();
        if let Some(value) = rule
            .values
            .iter()
            .find(|value| !token.is_empty() && value.eq_ignore_ascii_case(&token))
            && !named.contains(&value)
        {
            named.push(value);
        }
    }
    match named[..] {
        [value] => Some(value.clone()),
        _ => None,
    }
}
fn pr_rules(text: &str) -> Option<&str> {
    let heading = |line: &str| line.trim_start().starts_with('#');
    let mut offset = 0;
    let mut start = None;
    for line in text.split_inclusive('\n') {
        if heading(line) {
            if let Some(start) = start {
                return Some(&text[start..offset]);
            }
            if line
                .trim()
                .trim_start_matches('#')
                .trim()
                .eq_ignore_ascii_case("PR rules")
            {
                start = Some(offset + line.len());
            }
        }
        offset += line.len();
    }
    start.map(|start| &text[start..])
}
pub fn value(rule: &BodyLine, request: Option<&str>, paths: &[String]) -> String {
    if let Some(value) = request.and_then(|request| instructed(rule, request)) {
        return value;
    }
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
pub fn lines(rules: &[BodyLine], request: Option<&str>, paths: &[String]) -> String {
    rules
        .iter()
        .map(|rule| format!("{}: {}\n", rule.label, value(rule, request, paths)))
        .collect()
}
fn declared<'a>(line: &'a str, rule: &BodyLine) -> Option<&'a str> {
    let (label, value) = line.trim_start().split_once(':')?;
    label
        .trim_end()
        .eq_ignore_ascii_case(&rule.label)
        .then(|| value.trim())
}
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
pub fn validate(body: &str, rules: &[BodyLine]) -> crate::Result<()> {
    let failures: Vec<_> = rules
        .iter()
        .filter_map(|rule| {
            let values: Vec<_> = body
                .lines()
                .filter_map(|line| declared(line, rule))
                .collect();
            let category = match values.as_slice() {
                [] => RuleCategory::Missing,
                [value] if allowed(rule, value) => return None,
                [_] => RuleCategory::InvalidValue,
                _ if values
                    .iter()
                    .all(|value| value.eq_ignore_ascii_case(values[0])) =>
                {
                    RuleCategory::Duplicate
                }
                _ => RuleCategory::Conflicting,
            };
            Some(failure(rule, &values, category))
        })
        .collect();
    if failures.is_empty() {
        Ok(())
    } else {
        Err(refused(failures))
    }
}
fn allowed(rule: &BodyLine, value: &str) -> bool {
    rule.values
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(value))
}
fn failure(rule: &BodyLine, values: &[&str], category: RuleCategory) -> RuleFailure {
    RuleFailure {
        label: rule.label.clone(),
        category,
        expected_count: 1,
        found_count: values.len(),
        distinct_count: values
            .iter()
            .map(|value| value.to_lowercase())
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        invalid_count: values.iter().filter(|value| !allowed(rule, value)).count(),
        allowed_value_count: rule.values.len(),
    }
}
fn refused(mut rules: Vec<RuleFailure>) -> crate::Error {
    let omitted_rules = rules.len().saturating_sub(16);
    rules.truncate(16);
    LocalRefusal::error(
        "pr_policy",
        Reason::BodyPolicy {
            rules,
            omitted_rules,
        },
    )
}
pub(crate) fn compose(
    body: &str,
    rules: &[BodyLine],
    request: Option<&str>,
    paths: &[String],
    required: &[&str],
) -> crate::Result<String> {
    let mut replacements = Vec::new();
    for rule in rules {
        let mut explicit: Vec<_> = required
            .iter()
            .copied()
            .filter(|line| declared(line, rule).is_some())
            .collect();
        explicit.sort_unstable();
        explicit.dedup();
        let values: Vec<_> = explicit
            .iter()
            .filter_map(|line| declared(line, rule))
            .collect();
        if explicit.len() > 1 {
            return Err(refused(vec![failure(
                rule,
                &values,
                RuleCategory::IncompatibleObligations,
            )]));
        }
        if values.iter().any(|value| !allowed(rule, value)) {
            return Err(refused(vec![failure(
                rule,
                &values,
                RuleCategory::InvalidValue,
            )]));
        }
        let declarations: Vec<_> = body
            .lines()
            .filter_map(|line| declared(line, rule))
            .collect();
        let compliant = declarations.len() == 1 && allowed(rule, declarations[0]);
        if compliant
            && explicit
                .first()
                .is_none_or(|required| body.lines().any(|line| line == *required))
        {
            continue;
        }
        let line = explicit.first().map_or_else(
            || format!("{}: {}", rule.label, value(rule, request, paths)),
            |line| (*line).to_owned(),
        );
        replacements.push((rule, line));
    }
    if replacements.is_empty() {
        return Ok(body.to_owned());
    }
    let kept: String = body
        .split_inclusive('\n')
        .filter(|line| {
            !replacements
                .iter()
                .any(|(rule, _)| declared(line, rule).is_some())
        })
        .collect();
    let added = replacements
        .iter()
        .map(|(_, line)| line.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let mut text = String::new();
    let mut inserted = false;
    for line in kept.split_inclusive('\n') {
        text.push_str(line);
        if !inserted && line.trim_start().starts_with("Refs #") {
            if !line.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(&added);
            text.push('\n');
            inserted = true;
        }
    }
    if !inserted {
        text = format!("{added}\n{text}");
    }
    Ok(text)
}
