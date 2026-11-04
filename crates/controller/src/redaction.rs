//! Text matching against the authoritative Rust redaction policy.
use fancy_regex::{Captures, Regex};
mod compatibility;
mod source;
mod unicode;

use source::{Check, Policy};
use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
};
type Result<T> = std::result::Result<T, String>;
type Span = (usize, usize);
const LIMIT: usize = 1024 * 1024;
fn policy() -> &'static Policy {
    static POLICY: LazyLock<Policy> = LazyLock::new(source::policy);
    &POLICY
}

struct Characters {
    alnum: Vec<[u32; 2]>,
    alpha: Vec<[u32; 2]>,
    digit: Vec<[u32; 2]>,
    decimal: Vec<[u32; 2]>,
    upper: Vec<[u32; 2]>,
    lower: Vec<[u32; 2]>,
    space: Vec<[u32; 2]>,
}
fn characters() -> Result<&'static Characters> {
    static CHARACTERS: LazyLock<Result<Characters>> = LazyLock::new(|| {
        let ranges = |name: &str| -> Result<Vec<[u32; 2]>> {
            let mut ranges = Vec::new();
            for range in policy()
                .characters
                .get(name)
                .copied()
                .ok_or("missing Python character ranges")?
                .split(',')
            {
                let (start, end) = range.split_once('-').ok_or("invalid character range")?;
                let start = u32::from_str_radix(start, 16).map_err(|e| e.to_string())?;
                let end = u32::from_str_radix(end, 16).map_err(|e| e.to_string())?;
                if start > end
                    || end > 0x10ffff
                    || ranges
                        .last()
                        .is_some_and(|previous: &[u32; 2]| previous[1] >= start)
                {
                    return Err("invalid Python character ranges".into());
                }
                ranges.push([start, end]);
            }
            Ok(ranges)
        };
        Ok(Characters {
            alnum: ranges("alnum")?,
            alpha: ranges("alpha")?,
            digit: ranges("digit")?,
            decimal: ranges("decimal")?,
            upper: ranges("upper")?,
            lower: ranges("lower")?,
            space: ranges("space")?,
        })
    });
    CHARACTERS.as_ref().map_err(Clone::clone)
}
fn member(ranges: &[[u32; 2]], character: char) -> bool {
    let codepoint = character as u32;
    ranges
        .binary_search_by(|[start, end]| {
            if codepoint < *start {
                std::cmp::Ordering::Greater
            } else if codepoint > *end {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}
impl Characters {
    fn trim<'a>(&self, text: &'a str) -> &'a str {
        text.trim_matches(|c| member(&self.space, c))
    }
}

fn python_pattern(pattern: &str) -> Result<String> {
    fn class(ranges: &[[u32; 2]], word: bool) -> String {
        let mut body = if word { "_".to_owned() } else { String::new() };
        for [start, end] in ranges {
            body.push_str(&format!(r"\x{{{start:x}}}-\x{{{end:x}}}"));
        }
        body
    }
    static CLASSES: LazyLock<Result<[String; 3]>> = LazyLock::new(|| {
        let chars = characters()?;
        Ok([
            class(&chars.alnum, true),
            class(&chars.decimal, false),
            class(&chars.space, false),
        ])
    });
    compatibility::translate(pattern, CLASSES.as_ref().map_err(Clone::clone)?)
}

fn rx(pattern: &str) -> Result<Arc<Regex>> {
    static CACHE: LazyLock<Mutex<HashMap<String, Arc<Regex>>>> = LazyLock::new(Mutex::default);
    let mut cache = CACHE.lock().map_err(|_| "redaction cache poisoned")?;
    if let Some(regex) = cache.get(pattern) {
        return Ok(regex.clone());
    }
    let regex = Arc::new(Regex::new(&python_pattern(pattern)?).map_err(|e| e.to_string())?);
    cache.insert(pattern.into(), regex.clone());
    Ok(regex)
}
fn rule(name: &str) -> Result<Arc<Regex>> {
    rx(policy()
        .regex
        .get(name)
        .or_else(|| policy().helpers.get(name))
        .copied()
        .ok_or_else(|| format!("missing redaction rule {name}"))?)
}
fn has(set: &str, word: &str) -> Result<bool> {
    Ok(policy()
        .words
        .get(set)
        .ok_or("missing redaction words")?
        .contains(&word))
}
fn search(pattern: &str, text: &str) -> Result<bool> {
    rx(pattern)?.is_match(text).map_err(|e| e.to_string())
}
fn full(pattern: &str, text: &str) -> Result<bool> {
    search(&format!(r"\A(?:{pattern})\Z"), text)
}
fn full_rule(name: &str, text: &str) -> Result<bool> {
    full(
        policy()
            .regex
            .get(name)
            .or_else(|| policy().helpers.get(name))
            .copied()
            .ok_or("missing redaction rule")?,
        text,
    )
}
fn start_rule(name: &str, text: &str) -> Result<bool> {
    Ok(rule(name)?
        .find(text)
        .map_err(|e| e.to_string())?
        .is_some_and(|m| m.start() == 0))
}
fn matches<'a>(regex: &Arc<Regex>, text: &'a str) -> Result<Vec<Captures<'a>>> {
    regex
        .captures_iter(text)
        .map(|m| m.map_err(|e| e.to_string()))
        .collect()
}
fn get<'a>(cap: &Captures<'a>, name: &str) -> &'a str {
    cap.name(name).map_or("", |m| m.as_str())
}
fn span(cap: &Captures<'_>, name: &str) -> Option<Span> {
    cap.name(name).map(|m| (m.start(), m.end()))
}
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Strong,
    Weak,
}
fn label_kind(label: &str) -> Result<Option<Kind>> {
    if label.chars().count() > 128 {
        return Ok(None);
    }
    let words = rule("LABEL_WORDS")?
        .find_iter(label)
        .map(|m| {
            m.map(|m| m.as_str().to_lowercase())
                .map_err(|e| e.to_string())
        })
        .collect::<Result<Vec<_>>>()?;
    if words.is_empty() {
        return Ok(None);
    }
    for word in &words {
        if has("NOT_SECRET", word)? {
            return Ok(None);
        }
    }
    let mut previous = "";
    for word in &words {
        let parts = policy().words["SECRET_PARTS"];
        if has("SECRET_WORDS", word)?
            || word.ends_with("token")
            || parts.iter().any(|part| word.contains(part))
            || (word == "key" && has("KEY_QUALIFIERS", previous)?)
            || matches!(
                (previous, word.as_str()),
                ("session", "id") | ("key", "value")
            )
        {
            return Ok(Some(Kind::Strong));
        }
        previous = word;
    }
    for word in &words {
        if has("WEAK_WORDS", word)? {
            return Ok(Some(Kind::Weak));
        }
    }
    Ok(None)
}
fn unquote(value: &str) -> Result<(String, bool)> {
    let regex = rule("UNQUOTE_PREFIX")?;
    let stripped = regex.replace(value, "");
    let stripped = stripped.trim_start_matches('\\');
    if stripped.chars().count() >= 2 && stripped.starts_with(['"', '\'', '`']) {
        let end = stripped.char_indices().next_back().map_or(0, |(i, _)| i);
        return Ok((stripped[1..end].trim_end_matches('\\').into(), true));
    }
    Ok((value.into(), false))
}
fn placeholder(value: &str) -> Result<bool> {
    if !full_rule("PLACEHOLDER_TEXT", value)? {
        return Ok(false);
    }
    for word in rule("LABEL_WORDS")?.find_iter(value) {
        if has(
            "PLACEHOLDERS",
            &word.map_err(|e| e.to_string())?.as_str().to_lowercase(),
        )? {
            return Ok(true);
        }
    }
    Ok(false)
}
fn prefix(text: &str, count: usize) -> &str {
    &text[..text
        .char_indices()
        .nth(count)
        .map_or(text.len(), |(i, _)| i)]
}
fn after(text: &str, position: usize, count: usize) -> &str {
    prefix(&text[position..], count)
}
fn before(text: &str, position: usize, count: usize) -> usize {
    text[..position]
        .char_indices()
        .rev()
        .nth(count.saturating_sub(1))
        .map_or(0, |(i, _)| i)
}
fn random_like(token: &str, minimum: usize) -> Result<bool> {
    let chars = characters()?;
    if full_rule("SAFE_TOKEN", token)? || token.chars().any(|c| member(&chars.space, c)) {
        return Ok(false);
    }
    let parts = token.split(['-', '_']).collect::<Vec<_>>();
    if parts.len() > 1 {
        let mut ordinary = true;
        for part in &parts {
            ordinary &= full_rule("ORDINARY_PART", part)? || part.chars().count() <= 2;
        }
        if ordinary {
            for part in parts {
                if part.chars().count() >= minimum.max(24) && random_like(part, minimum)? {
                    return Ok(true);
                }
            }
            return Ok(false);
        }
    }
    let classes = token
        .chars()
        .filter(|c| member(&chars.alnum, *c))
        .map(|c| {
            if member(&chars.digit, c) {
                2
            } else {
                u8::from(member(&chars.upper, c))
            }
        })
        .collect::<Vec<_>>();
    if classes.len() < minimum || classes.iter().all(|c| *c == classes[0]) {
        return Ok(false);
    }
    let switches = classes.windows(2).filter(|pair| pair[0] != pair[1]).count();
    let words = rule("WORDISH")?
        .find_iter(token)
        .map(|m| {
            m.map(|m| m.as_str().chars().count())
                .map_err(|e| e.to_string())
        })
        .sum::<Result<usize>>()?;
    let letters = token.chars().filter(|c| member(&chars.alpha, *c)).count();
    let vowels = token.chars().filter(|c| "aeiouAEIOU".contains(*c)).count();
    Ok((5 * switches >= 2 * classes.len()
        && (10 * words < 7 * classes.len() || 4 * vowels < letters))
        || (2 * words < classes.len() && 5 * switches >= classes.len()))
}
fn plausible(
    kind: Option<Kind>,
    label: &str,
    value: &str,
    tail: &str,
    assign: &str,
    code: bool,
) -> Result<bool> {
    let chars = characters()?;
    let (inner, quoted) = unquote(value)?;
    if kind.is_none()
        || chars.trim(&inner).is_empty()
        || start_rule("REFERENCE", &inner)?
        || full_rule("PATHLIKE", &inner)?
        || rule("TEMPLATE")?
            .is_match(&inner)
            .map_err(|e| e.to_string())?
        || full_rule("VERSION", &inner)?
        || inner.to_lowercase() == label.to_lowercase()
        || full_rule("SCHEME_NAME", chars.trim(&inner))?
        || placeholder(prefix(&inner, 64))?
    {
        return Ok(false);
    }
    if quoted
        && (tail
            .trim_start_matches(|c| member(&chars.space, c))
            .starts_with(['*', '+'])
            || start_rule("CONCATENATION", &inner)?)
    {
        return Ok(false);
    }
    if !quoted {
        if has("KEYWORDS", &inner)?
            || has("TYPE_NAMES", &inner)?
            || inner.contains(['{', '(', '['])
            || inner.starts_with(['&', '-'])
            || tail.starts_with(['(', '['])
            || full_rule("DOTTED", &inner)?
            || full_rule("NUMBER", &inner)?
        {
            return Ok(false);
        }
        if assign == ":" && full_rule("TYPE_IDENTIFIER", &inner)? && start_rule("TYPE_TAIL", tail)?
        {
            return Ok(false);
        }
        if full_rule("IDENTIFIER", &inner)? && label_kind(&inner)?.is_some() {
            return Ok(false);
        }
        if full_rule("CODE_NAME", &inner)? && code {
            return Ok(false);
        }
        if full_rule("WORD", &inner)? && start_rule("WORD_TAIL", tail)? {
            return Ok(false);
        }
        if full_rule("SHORT_NAME", &inner)? && start_rule("LOWER_TAIL", tail)? {
            return Ok(false);
        }
    }
    if kind == Some(Kind::Weak) {
        let sample = prefix(&inner, 64);
        return Ok(random_like(sample, 12)?
            || (full_rule("WEAK_TOKEN", sample)?
                && sample.chars().any(|c| member(&chars.upper, c))
                && sample.chars().any(|c| member(&chars.lower, c))
                && sample.chars().any(|c| member(&chars.digit, c))));
    }
    Ok(true)
}
fn readable(value: &str) -> Result<bool> {
    let inner = unquote(value)?.0;
    Ok(full_rule("READABLE_TEXT", &inner)? && !random_like(&inner, 8)?)
}
fn token_shaped(value: &str) -> Result<bool> {
    let chars = characters()?;
    Ok(!full_rule("WORD", value)?
        && (value.chars().any(|c| member(&chars.digit, c))
            || value.contains(['.', '_', '~', '+', '/', '=', '-'])
            || value.chars().skip(1).any(|c| member(&chars.upper, c))))
}

fn secretish(value: &str, quoted: bool) -> Result<bool> {
    let chars = characters()?;
    let core = if quoted {
        value
    } else {
        value.trim_end_matches(['.', ',', ':', ';', ')'])
    }
    .trim_matches(['*', '_']);
    if core.chars().count() < 8
        || core.chars().any(|c| member(&chars.space, c))
        || start_rule("REFERENCE", core)?
        || rule("TEMPLATE")?
            .is_match(core)
            .map_err(|e| e.to_string())?
        || full_rule("SAFE_TOKEN", core)?
        || full_rule("PATHLIKE", core)?
        || core.contains("://")
        || full_rule("DOTTED", core)?
        || full_rule("NUMBER_TEXT", core)?
        || placeholder(core)?
    {
        return Ok(false);
    }
    if random_like(core, 12)? {
        return Ok(true);
    }
    if core.contains('/') {
        return Ok(false);
    }
    let upper = core.chars().any(|c| member(&chars.upper, c));
    let lower = core.chars().any(|c| member(&chars.lower, c));
    let digit = core.chars().any(|c| member(&chars.digit, c));
    let special = core.chars().filter(|c| "!#%^*~@$&+?".contains(*c)).count();
    Ok((upper && lower && digit)
        || (special >= 2 && (upper || digit))
        || (quoted && digit && (upper || lower) && special >= 1))
}
fn checked(check: Check, cap: &Captures<'_>, text: &str) -> Result<Option<Span>> {
    let chars = characters()?;
    let label = get(cap, "l");
    let value = get(cap, "v");
    let keep = match check {
        Check::Labelled => {
            if label_kind(label)? == Some(Kind::Strong)
                && matches!(label.to_lowercase().as_str(), "signature" | "sig")
                && readable(value)?
            {
                return Ok(None);
            }
            let assign = cap.name("a").ok_or("missing assignment")?;
            let v = cap.name("v").ok_or("missing value")?;
            let whole = cap.get(0).ok_or("missing match")?;
            let gap = &text[assign.end()..v.start()];
            if gap.contains('\n') {
                let line = text[..whole.start()].rfind('\n').map_or(0, |i| i + 1);
                let indent = text[line..whole.start()]
                    .chars()
                    .take_while(|c| matches!(c, ' ' | '\t'))
                    .count();
                let value_line = text[..v.start()].rfind('\n').map_or(0, |i| i + 1);
                if text[value_line..v.start()].chars().count() <= indent {
                    return Ok(None);
                }
            }
            let low = before(text, whole.start(), 80);
            let line = text[low..whole.start()]
                .rfind('\n')
                .map_or(low, |i| low + i + 1);
            if full_rule("WORD", value)?
                && matches(&rule("PROSE_WORDS")?, &text[line..whole.start()])?.len() >= 2
            {
                return Ok(None);
            }
            let code = get(cap, "q").ends_with(['"', '\'', '`'])
                || (assign.as_str() == "="
                    && (text[..assign.start()]
                        .chars()
                        .next_back()
                        .is_some_and(|c| member(&chars.space, c))
                        || gap.chars().next().is_some_and(|c| member(&chars.space, c))));
            plausible(
                label_kind(label)?,
                label,
                value,
                after(text, v.end(), 8),
                assign.as_str(),
                code,
            )?
        }
        Check::Flag => plausible(label_kind(label)?, label, value, "", "=", false)?,
        Check::Table => {
            let value = value.trim_matches('`');
            label_kind(label.trim_matches(['`', '*', ' ']))?.is_some()
                && !start_rule("REFERENCE", value)?
                && value.chars().count() >= 8
                && token_shaped(value)?
                && !value.chars().any(|c| member(&chars.space, c))
        }
        Check::Block => label_kind(label)? == Some(Kind::Strong),
        Check::NameValue => plausible(label_kind(label)?, label, value, "", "", false)?,
        Check::Header => {
            let group = if cap.name("v").is_some() { "v" } else { "c" };
            let value = get(cap, group);
            let inner = unquote(value)?.0;
            if start_rule("REFERENCE", &inner)?
                || rule("TEMPLATE")?
                    .is_match(&inner)
                    .map_err(|e| e.to_string())?
                || placeholder(&inner)?
                || start_rule("HEADER_REFERENCE", value)?
            {
                return Ok(None);
            }
            if let Some(scheme) = rule("SCHEME")?.captures(value).map_err(|e| e.to_string())?
                && scheme.get(0).is_some_and(|m| m.start() == 0)
            {
                let token = scheme.get(1).ok_or("missing scheme token")?;
                if token.as_str().is_empty()
                    || start_rule("REFERENCE", token.as_str())?
                    || rule("TEMPLATE")?
                        .is_match(token.as_str())
                        .map_err(|e| e.to_string())?
                    || placeholder(token.as_str())?
                {
                    return Ok(None);
                }
                let start = cap.name(group).ok_or("missing header")?.start();
                return Ok(Some((start, start + token.end())));
            }
            return Ok(span(cap, group));
        }
        Check::Bearer => token_shaped(value)? && !placeholder(value)?,
        Check::Url => {
            !start_rule("REFERENCE", value)?
                && !rule("TEMPLATE")?
                    .is_match(value)
                    .map_err(|e| e.to_string())?
                && !placeholder(value)?
        }
        Check::User => {
            !value.chars().all(|c| member(&chars.digit, c))
                && !rule("TEMPLATE")?
                    .is_match(value)
                    .map_err(|e| e.to_string())?
        }
        Check::Aws => label_kind(label)?.is_some(),
        Check::SshKeygen => !unquote(value)?.0.is_empty(),
        Check::Xml => {
            chars.trim(value).chars().count() >= 4
                && label_kind(label)? == Some(Kind::Strong)
                && plausible(Some(Kind::Strong), label, value, "", "=", false)?
        }
        Check::JsonPair => {
            unquote(value)?.0.chars().count() >= 4
                && plausible(label_kind(label)?, label, value, "", ":", false)?
        }
        Check::Pair => secretish(value, false)?,
    };
    Ok(if keep { span(cap, "v") } else { None })
}
fn spans(text: &str) -> Result<Vec<Span>> {
    let chars = characters()?;
    let mut found = Vec::new();
    let ends = matches(&rule("PRIVATE_END")?, text)?
        .iter()
        .filter_map(|c| c.get(0).map(|m| m.end()))
        .collect::<Vec<_>>();
    let mut reach = 0;
    for begin in matches(&rule("PRIVATE_HEADER")?, text)? {
        let begin = begin.get(0).ok_or("missing private header")?;
        if begin.start() < reach {
            continue;
        }
        reach = ends
            .iter()
            .find(|end| **end > begin.end())
            .copied()
            .unwrap_or(begin.end());
        if reach == begin.end()
            && let Some(lines) = rule("KEY_LINES")?
                .find(&text[begin.end()..])
                .map_err(|e| e.to_string())?
            && lines.start() == 0
        {
            reach += lines.end();
        }
        found.push((begin.start(), reach));
    }
    for pattern in policy().patterns {
        for cap in matches(&rx(pattern)?, text)? {
            let value = cap
                .name("v")
                .or_else(|| cap.get(0))
                .ok_or("missing redaction match")?;
            found.push((value.start(), value.end()));
        }
    }
    for &(pattern, check) in policy().checked {
        for cap in matches(&rx(pattern)?, text)? {
            if let Some(span) = checked(check, &cap, text)? {
                found.push(span);
            }
        }
    }
    for word in matches(&rule("NEAR_WORD")?, text)? {
        let word = word.get(0).ok_or("missing nearby word")?;
        let rest = &text[word.end()..];
        let high = prefix(rest, policy().near_chars + 256).len();
        let end = rest[..high].find('\n').unwrap_or(high);
        let limit = prefix(&rest[..end], policy().near_chars).len();
        for token in matches(&rule("NEAR_TOKEN")?, &rest[..end])? {
            if token.get(0).ok_or("missing nearby token")?.start() >= limit {
                break;
            }
            let group = if token.name("qv").is_some() {
                "qv"
            } else {
                "v"
            };
            if secretish(get(&token, group), group == "qv")?
                && let Some((start, stop)) = span(&token, group)
            {
                found.push((word.end() + start, word.end() + stop));
            }
        }
    }
    if text.contains("Secret") {
        for block in matches(&rule("SECRET_DATA")?, text)? {
            let body = block.name("b").ok_or("missing secret body")?;
            for value in matches(&rule("SECTION_VALUE")?, body.as_str())? {
                if let Some((start, end)) = span(&value, "v") {
                    found.push((body.start() + start, body.start() + end));
                }
            }
        }
    }
    let sections = matches(&rule("SECTION")?, text)?;
    for (index, section) in sections.iter().enumerate() {
        if label_kind(chars.trim(get(section, "l")))? == Some(Kind::Strong) {
            let start = section.get(0).ok_or("missing section")?.end();
            let end = sections
                .get(index + 1)
                .and_then(|c| c.get(0))
                .map_or(text.len(), |m| m.start());
            for value in matches(&rule("SECTION_VALUE")?, &text[start..end])? {
                if let Some((a, b)) = span(&value, "v") {
                    found.push((start + a, start + b));
                }
            }
        }
    }
    let mut columns: Option<(usize, Vec<usize>)> = None;
    let mut position = 0;
    if text.contains(',') {
        for line in text.split('\n') {
            let cells = line.split(',').collect::<Vec<_>>();
            if let Some((width, indices)) = &columns
                && cells.len() == *width
            {
                let mut offset = position;
                for (index, cell) in cells.iter().enumerate() {
                    if indices.contains(&index)
                        && !chars.trim(cell).is_empty()
                        && label_kind(chars.trim(cell))? != Some(Kind::Strong)
                    {
                        found.push((offset, offset + cell.len()));
                    }
                    offset += cell.len() + 1;
                }
            } else {
                let mut header = (2..=64).contains(&cells.len());
                for cell in &cells {
                    header &= full_rule("CSV_HEADER", cell)?;
                }
                columns = if header {
                    let mut indices = Vec::new();
                    for (index, cell) in cells.iter().enumerate() {
                        if label_kind(chars.trim(cell))? == Some(Kind::Strong) {
                            indices.push(index);
                        }
                    }
                    if indices.is_empty() {
                        None
                    } else {
                        Some((cells.len(), indices))
                    }
                } else {
                    None
                };
            }
            position += line.len() + 1;
        }
    }
    for cap in matches(&rule("CANDIDATE")?, text)? {
        let matched = cap.get(0).ok_or("missing candidate")?;
        let token = matched.as_str();
        let mut position = matched.start();
        if (text[matched.end()..].starts_with("...") || text[matched.end()..].starts_with('…'))
            && full_rule("HEX_PREFIX", token)?
        {
            continue;
        }
        let segments = token.split('/').collect::<Vec<_>>();
        let mut path = false;
        for segment in &segments {
            path |= full_rule("PATH_SEGMENT", segment)?;
        }
        if segments.len() > 1 && path {
            for segment in segments {
                if segment.chars().count() >= 24 && random_like(segment, 20)? {
                    found.push((position, position + segment.len()));
                }
                position += segment.len() + 1;
            }
            continue;
        }
        if random_like(token, 20)? {
            found.push((matched.start(), matched.end()));
            continue;
        }
        if full_rule("SAFE_TOKEN", token)? {
            continue;
        }
        for piece in token.split(['/', '_', '.', '-']) {
            if piece.chars().count() >= 24 && random_like(piece, 20)? {
                found.push((position, position + piece.len()));
            }
            position += piece.len() + 1;
        }
    }
    found.sort_unstable();
    let mut merged: Vec<Span> = Vec::new();
    for (start, end) in found {
        if end <= start {
            continue;
        }
        if let Some(last) = merged.last_mut()
            && start <= last.1
        {
            last.1 = last.1.max(end);
            continue;
        }
        merged.push((start, end));
    }
    Ok(merged)
}
pub fn text(value: &str) -> crate::Result<String> {
    let render = || -> Result<String> {
        if value.len() > LIMIT {
            return Err("redaction input exceeds one MiB".into());
        }
        let mut out = String::new();
        let mut cursor = 0;
        for (start, end) in spans(value)? {
            out.push_str(
                value
                    .get(cursor..start)
                    .ok_or("invalid redaction boundary")?,
            );
            out.push_str(policy().redacted);
            cursor = end;
        }
        out.push_str(value.get(cursor..).ok_or("invalid redaction boundary")?);
        Ok(out)
    };
    render().map_err(|error| crate::Error::RedactionInfra(std::io::Error::other(error)))
}

/// Serialize the authoritative Rust redaction policy for Python consumers.
pub fn generated_policy() -> String {
    static VALID: LazyLock<Result<()>> = LazyLock::new(|| {
        for pattern in policy()
            .regex
            .values()
            .chain(policy().helpers.values())
            .copied()
            .chain(policy().patterns.iter().copied())
            .chain(policy().checked.iter().map(|(pattern, _)| *pattern))
        {
            rx(pattern)?;
        }
        Ok(())
    });
    VALID
        .as_ref()
        .expect("cannot generate an unsupported redaction policy");
    serde_json::to_string_pretty(policy()).expect("static redaction policy is serializable") + "\n"
}

/// Bind the packaged Python consumer to this build's exact policy and structure.
pub fn generated_policy_identity() -> String {
    use sha2::{Digest, Sha256};
    let policy = policy();
    let identity = serde_json::json!({
        "sha256": format!("{:x}", Sha256::digest(generated_policy().as_bytes())),
        "fields": serde_json::to_value(policy).expect("static policy").as_object().expect("policy object").keys().cloned().collect::<Vec<_>>(),
        "values": {
            "schema_version": policy.schema_version, "unicode_version": policy.unicode_version,
            "context": policy.context, "near_chars": policy.near_chars, "redacted": policy.redacted,
        },
        "maps": {
            "regex": policy.regex.keys().collect::<Vec<_>>(),
            "helpers": policy.helpers.keys().collect::<Vec<_>>(),
            "words": policy.words.keys().collect::<Vec<_>>(),
            "characters": policy.characters.keys().collect::<Vec<_>>(),
        },
        "patterns": policy.patterns.len(),
        "checked": policy.checked.iter().map(|(_, check)| check).collect::<Vec<_>>(),
    });
    format!(
        "# Generated by tl3 redaction-policy --identity-output. Do not edit.\nIDENTITY = {}\n",
        serde_json::to_string_pretty(&identity).expect("static identity")
    )
}

#[cfg(test)]
fn worker_python() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../workers/.venv/bin/python")
}
#[cfg(test)]
fn python_command() -> std::process::Command {
    let mut command = std::process::Command::new(worker_python());
    command.env_clear();
    for key in [
        "PATH",
        "HOME",
        "TMPDIR",
        "TEST_TMPDIR",
        "TEST_SRCDIR",
        "TEST_WORKSPACE",
        "RUNFILES_DIR",
        "RUNFILES_MANIFEST_FILE",
        "JAVA_RUNFILES",
        "PYTHON_RUNFILES",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command
}
#[cfg(test)]
mod parity_tests {
    #[test]
    fn character_predicates_and_regex_classes_match_python_for_every_scalar() {
        use sha2::{Digest, Sha256};
        let output = super::python_command()
            .args([
                "-I",
                "-c",
                r#"
import hashlib,json,re,sys
text = ''.join(chr(n) for n in range(sys.maxunicode + 1) if not 0xd800 <= n <= 0xdfff)
predicates = [str.isalnum,str.isalpha,str.isdigit,str.isdecimal,str.isupper,str.islower,str.isspace]
flags = bytes(sum(1 << i for i,predicate in enumerate(predicates) if predicate(c)) for c in text)
patterns = [r'\w',r'\W',r'\d',r'\D',r'\s',r'\S',r'(?i)[\w-]',r'(?i)[^\w-]']
out = {'flags':hashlib.sha256(flags).hexdigest()}
for pattern in patterns:
    digest = hashlib.sha256()
    for match in re.finditer(pattern,text):
        digest.update(match[0].encode())
    out[pattern] = digest.hexdigest()
print(json.dumps(out))
"#,
            ])
            .output()
            .unwrap();
        assert!(output.status.success());
        let expected: std::collections::BTreeMap<String, String> =
            serde_json::from_slice(&output.stdout).unwrap();
        let text = (0..=0x10ffff)
            .filter_map(char::from_u32)
            .collect::<String>();
        let chars = super::characters().unwrap();
        let mut flags = Sha256::new();
        for c in text.chars() {
            let properties = [
                &chars.alnum,
                &chars.alpha,
                &chars.digit,
                &chars.decimal,
                &chars.upper,
                &chars.lower,
                &chars.space,
            ];
            let bits = properties
                .into_iter()
                .enumerate()
                .fold(0u8, |bits, (i, ranges)| {
                    bits | (u8::from(super::member(ranges, c)) << i)
                });
            flags.update([bits]);
        }
        assert_eq!(format!("{:x}", flags.finalize()), expected["flags"]);
        for (pattern, expected) in expected.into_iter().filter(|(p, _)| p != "flags") {
            let mut digest = Sha256::new();
            for matched in super::rx(&pattern).unwrap().find_iter(&text) {
                digest.update(matched.unwrap().as_str().as_bytes());
            }
            assert_eq!(format!("{:x}", digest.finalize()), expected, "{pattern}");
        }
        for (pattern, input, expected) in [
            (r"\b", "", false),
            (r"\B", "", true),
            (r"\bpassword", "\u{301}password", true),
            (r"\bpassword", "\u{200c}password", true),
        ] {
            assert_eq!(super::search(pattern, input).unwrap(), expected);
        }
    }

    #[test]
    fn oversized_input_fails_closed() {
        assert!(matches!(
            super::text(&"x".repeat(super::LIMIT + 1)),
            Err(crate::Error::RedactionInfra(_))
        ));
    }

    #[test]
    fn generated_policy_matches_the_authoritative_source() {
        assert_eq!(
            super::generated_policy(),
            include_str!("redaction/policy.json")
        );
        let worker = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../workers/src/tl3_workers/policy.json");
        assert_eq!(
            super::generated_policy(),
            std::fs::read_to_string(worker).unwrap()
        );
    }
}
