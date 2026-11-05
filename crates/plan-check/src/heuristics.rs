use crate::unicode::{is_space, is_word, lowercase};

const EDIT_VERBS: &[&str] = &[
    "extend",
    "extends",
    "extending",
    "patch",
    "patches",
    "patching",
    "update",
    "updates",
    "updating",
    "modify",
    "modifies",
    "modifying",
    "rewrite",
    "rewrites",
    "rewriting",
    "change",
    "changes",
    "changing",
    "edit",
    "edits",
    "editing",
    "fix",
    "fixes",
    "fixing",
    "replace",
    "replaces",
    "replacing",
];
const ADD_VERBS: &[&str] = &["add", "adds", "adding"];
const HOLDS: &[&str] = &[
    "not",
    "never",
    "don't",
    "don’t",
    "dont",
    "nothing",
    "without",
    "unchanged",
    "keep",
    "keeps",
    "keeping",
    "leave",
    "leaves",
    "leaving",
    "preserve",
    "preserves",
    "preserving",
];
const CLAUSES: &[&str] = &[
    "so", "that", "until", "while", "and", "because", "when", "then", "but", "or", "if",
];

fn words(text: &str) -> Vec<String> {
    let text = lowercase(text);
    let mut result = Vec::new();
    let mut start = None;
    for (index, c) in text.char_indices() {
        if let Some(begin) = start {
            if !(is_word(c) || matches!(c, '\'' | '’' | '-')) {
                result.push(text[begin..index].to_owned());
                start = None;
            }
        } else if c.is_ascii_lowercase() {
            start = Some(index);
        }
    }
    if let Some(begin) = start {
        result.push(text[begin..].to_owned());
    }
    result
}

fn sentence_mentions(sentence: &str, test: &str, result: &mut Vec<(Vec<String>, Vec<String>)>) {
    let mut next = 0;
    // Include the final position for Python's zero-width empty-name matches.
    for start in sentence
        .char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(sentence.len()))
    {
        if start < next || !sentence[start..].starts_with(test) {
            continue;
        }
        let end = start + test.len();
        if sentence[..start]
            .chars()
            .next_back()
            .is_some_and(|c| is_word(c) || c == '/')
            || sentence[end..].chars().next().is_some_and(is_word)
            || sentence[end..].starts_with(".py")
        {
            continue;
        }
        result.push((words(&sentence[..start]), words(&sentence[end..])));
        next = end;
    }
}

pub fn mentions(text: &str, test: &str) -> Vec<(Vec<String>, Vec<String>)> {
    let mut result = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((index, c)) = chars.next() {
        if c == '\n' {
            sentence_mentions(&text[start..index], test, &mut result);
            start = index + 1;
        } else if matches!(c, '.' | ';' | '!' | '?')
            && chars.peek().is_none_or(|&(_, next)| is_space(next))
        {
            sentence_mentions(&text[start..index], test, &mut result);
            start = chars.next().map_or(text.len(), |(i, c)| i + c.len_utf8());
        }
    }
    sentence_mentions(&text[start..], test, &mut result);
    result
}

pub fn asks_to_edit(before: &[String], after: &[String]) -> bool {
    for index in (before.len().saturating_sub(7)..before.len()).rev() {
        let word = before[index].as_str();
        if HOLDS.contains(&word) || CLAUSES.contains(&word) {
            return false;
        }
        if EDIT_VERBS.contains(&word)
            || ADD_VERBS.contains(&word)
                && before
                    .last()
                    .is_some_and(|word| matches!(word.as_str(), "to" | "into"))
        {
            return !before[index.saturating_sub(3)..index]
                .iter()
                .any(|w| HOLDS.contains(&w.as_str()))
                && !after.iter().take(3).any(|w| w == "unchanged");
        }
    }
    false
}
