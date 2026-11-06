pub(crate) fn branch_satisfies(
    required: &str,
    branch: &str,
    issue: Option<u64>,
    node: Option<u64>,
    prefix: bool,
) -> bool {
    if !required.contains('<') {
        return if prefix {
            branch.starts_with(required)
        } else {
            branch == required
        };
    }
    let optional_word = required.strip_suffix("[-<word>]");
    let mut rest = optional_word.unwrap_or(required);
    let mut pattern = String::from("^");
    while let Some((literal, tail)) = rest.split_once('<') {
        pattern.push_str(&regex::escape(literal));
        let Some((slot, remaining)) = tail.split_once('>') else {
            return false;
        };
        match slot {
            "issue" => match issue {
                Some(issue) => pattern.push_str(&issue.to_string()),
                None => return false,
            },
            "node" => match node {
                Some(node) => pattern.push_str(&node.to_string()),
                None => return false,
            },
            "word" => pattern.push_str("[A-Za-z0-9][A-Za-z0-9_-]*"),
            _ => return false,
        }
        rest = remaining;
    }
    pattern.push_str(&regex::escape(rest));
    if optional_word.is_some() {
        pattern.push_str("(?:-[A-Za-z0-9][A-Za-z0-9_-]*)?");
    }
    if !prefix {
        pattern.push('$');
    }
    regex::Regex::new(&pattern).is_ok_and(|pattern| pattern.is_match(branch))
}
