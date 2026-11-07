//! Text matching against the authoritative Rust redaction policy.
use fancy_regex::{Captures, Regex};
mod azure_sas {
//! Confidential Azure SAS syntax, independent of signature entropy or authenticity.
use std::collections::BTreeMap;

const FIELD_BYTES: usize = 4096;

fn valid_percent_encoding(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len()
                || !bytes[index + 1..index + 3]
                    .iter()
                    .all(u8::is_ascii_hexdigit)
            {
                return false;
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    true
}

pub(super) fn spans(text: &str) -> Vec<(usize, usize)> {
    let mut result = Vec::new();
    // The caller's one-MiB source bound also bounds candidate scanning and parsing.
    let mut scanned_until = 0;
    for (colon, _) in text.match_indices("://") {
        if colon < scanned_until {
            continue;
        }
        let start = ["https", "http"].into_iter().find_map(|scheme| {
            let start = colon.checked_sub(scheme.len())?;
            text.get(start..colon)?
                .eq_ignore_ascii_case(scheme)
                .then_some(start)
        });
        let Some(start) = start else {
            continue;
        };
        if text[..start]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
        {
            continue;
        }
        let end = text[start..]
            .find(|c: char| {
                c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '\\' | ')' | ']' | '}')
            })
            .map_or(text.len(), |offset| start + offset);
        scanned_until = end;
        let raw = &text[start..end];
        let Ok(parsed) = url::Url::parse(raw) else {
            continue;
        };
        if !parsed.username().is_empty() || parsed.password().is_some() {
            continue;
        }
        let Some(account) = parsed
            .host_str()
            .and_then(|host| host.strip_suffix(".blob.core.windows.net"))
        else {
            continue;
        };
        if account.is_empty()
            || account.len() > 63
            || !account
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        {
            continue;
        }
        // Use the original query's byte coordinates. URL parsing only establishes authority.
        let query_end = raw.find('#').unwrap_or(raw.len());
        let Some(query_start) = raw[..query_end].find('?').map(|index| index + 1) else {
            continue;
        };
        let mut fields = BTreeMap::new();
        let mut offset = query_start;
        let mut valid = true;
        for pair in raw[query_start..query_end].split('&') {
            if let Some((key, value)) = pair.split_once('=') {
                if key.len() > FIELD_BYTES
                    || value.len() > FIELD_BYTES
                    || !valid_percent_encoding(pair)
                {
                    valid = false;
                    break;
                }
                let mut decoded = url::form_urlencoded::parse(pair.as_bytes());
                let Some((key, value)) = decoded.next() else {
                    valid = false;
                    break;
                };
                if matches!(key.as_ref(), "sv" | "se" | "ss" | "sr" | "sig") {
                    let span = (
                        start + offset + pair.find('=').unwrap() + 1,
                        start + offset + pair.len(),
                    );
                    if value.trim().is_empty() || fields.insert(key.into_owned(), span).is_some() {
                        valid = false;
                        break;
                    }
                }
            }
            offset += pair.len() + 1;
        }
        if valid
            && fields.contains_key("sv")
            && fields.contains_key("se")
            && (fields.contains_key("ss") || fields.contains_key("sr"))
            && let Some(span) = fields.get("sig")
        {
            result.push(*span);
        }
    }
    result
}

}
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
    let mut found = azure_sas::spans(text);
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
pub fn text(value: &str) -> Result<String> {
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
    render()
}

/// Serialize the authoritative Rust redaction policy for Python consumers.
pub fn generated_policy() -> String {
    static VALID: LazyLock<Result<()>> = LazyLock::new(|| {
        let policy = policy();
        let expressions = [("regex", &policy.regex), ("helpers", &policy.helpers)]
            .into_iter()
            .flat_map(|(section, entries)| {
                entries
                    .iter()
                    .map(move |(name, pattern)| (format!("{section}:{name}"), *pattern))
            })
            .chain(
                policy
                    .patterns
                    .iter()
                    .enumerate()
                    .map(|(index, pattern)| (format!("patterns:{index}"), *pattern)),
            )
            .chain(
                policy
                    .checked
                    .iter()
                    .enumerate()
                    .map(|(index, (pattern, _))| (format!("checked:{index}"), *pattern)),
            );
        for (name, pattern) in expressions {
            rx(pattern).map_err(|error| format!("{name}: {error}"))?;
        }
        Ok(())
    });
    VALID
        .as_ref()
        .expect("cannot generate an unsupported redaction policy");
    serde_json::to_string_pretty(policy()).expect("static redaction policy is serializable") + "\n"
}

/// The standalone pytest edge uses the same policy without worker package dependencies.
pub fn generated_harness_policy() -> String {
    format!(
        "\"\"\"Generated by tl3 redaction-policy --harness-output; do not edit.\"\"\"\nimport json\n\nPOLICY = json.loads({})\n",
        serde_json::to_string(&generated_policy()).expect("policy string")
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
        assert!(super::text(&"x".repeat(super::LIMIT + 1)).is_err());
    }

    #[test]
    fn generated_policy_matches_the_authoritative_source() {
        assert_eq!(super::generated_policy(), include_str!("policy.json"));
    }
}


/// Maximum matcher input or extracted context window bytes, not raw-source length.
pub const INPUT_BYTE_LIMIT: usize = LIMIT;
/// Immutable context retained on each side of a character-index window.
pub fn context_chars() -> usize { policy().context }
pub fn redacted_marker() -> &'static str { policy().redacted }
#[derive(Clone, Copy)]
pub enum FieldRole { Path, Content, Skipped, TokenCount }
pub fn field_matches(role: FieldRole, key: &str) -> Result<bool> {
    has(match role {
        FieldRole::Path => "PATH_KEYS",
        FieldRole::Content => "CONTENT_KEYS",
        FieldRole::Skipped => "SKIPPED_KEYS",
        FieldRole::TokenCount => "TOKEN_FIELDS",
    }, key)
}
#[derive(Clone, Copy)]
pub enum FieldValue<'a> { Null, Bool, Number, Text(&'a str), Container }
pub fn field_is_secret(key: &str, value: FieldValue<'_>, named: &str) -> Result<bool> {
    let kind = if key.chars().count() > 128 { Some(Kind::Strong) } else { label_kind(key)? };
    let weak_secret = if kind == Some(Kind::Weak) {
        if let FieldValue::Text(text) = value { plausible(Some(Kind::Weak), key, text, "", "", false)? } else { false }
    } else { false };
    Ok(matches!(kind, Some(Kind::Strong)) && !matches!(value, FieldValue::Null | FieldValue::Bool)
        || weak_secret
        || matches!(key, "value" | "Value") && label_kind(named)? == Some(Kind::Strong)
            && matches!(value, FieldValue::Text(_) | FieldValue::Number))
}

pub fn sensitive_file(path: &str) -> Result<bool> {
    if path.chars().count() > 4096 {
        return Ok(false);
    }
    let path = path.replace('\\', "/");
    let path = path.trim().trim_end_matches('/');
    let name = path.rsplit('/').next().unwrap_or("").trim();
    if path.ends_with(".docker/config.json") || path.ends_with(".kube/config") {
        return Ok(true);
    }
    if full_rule("SENSITIVE_NAME", name)? {
        return Ok(name.starts_with(".env")
            || !rule("SOURCE_FILE")?
                .is_match(name)
                .map_err(|error| error.to_string())?);
    }
    Ok(rule("SENSITIVE_BROAD")?
        .is_match(path)
        .map_err(|error| error.to_string())?
        && !rule("SOURCE_FILE")?
            .is_match(name)
            .map_err(|error| error.to_string())?)
}

/// Redact a character-index range with bounded surrounding source context.
pub fn window(source: &str, range: std::ops::Range<usize>, known_secrets: &[&str]) -> Result<String> {
        let (start, end) = (range.start, range.end);
        let length = source.chars().count();
        if start > end || end > length {
            return Err("invalid redaction window".into());
        }
        let low = start.saturating_sub(policy().context);
        let high = end.saturating_add(policy().context).min(length);
        let text = substring(source, low, high);
        if text.len() > LIMIT {
            return Err("redaction window exceeds one MiB".into());
        }
        let first = byte_at(text, start - low);
        let stop = byte_at(text, end - low);
        let mut found = spans(text)?;
        for secret in known_secrets {
            if !secret.is_empty() {
                found.extend(
                    text.match_indices(*secret)
                        .map(|(index, _)| (index, index + secret.len())),
                );
            }
        }
        found.sort_unstable();
        let mut cursor = first;
        let mut out = String::new();
        for (start, end) in found {
            if end <= cursor || start >= stop {
                continue;
            }
            if start > cursor {
                out.push_str(&text[cursor..start]);
            }
            out.push_str(policy().redacted);
            cursor = cursor.max(end);
        }
        if cursor < stop {
            out.push_str(&text[cursor..stop]);
        }
        Ok(out)
    }

/// Return a safe character-index cut; reject cut > end or an end beyond the source.
    // Source rule: the removed Python settle/tail_window safety boundary. Use the
    // authoritative TOKEN_CHAR/NOT_TOKEN/PRIVATE_HEADER grammar, never rescan a huge
    // omitted prefix through every secret matcher just to choose a window.
pub fn settled_cut(source: &str, cut: usize, end: usize, known_secrets: &[&str]) -> Result<usize> {
        if cut > end || end > source.chars().count() {
            return Err("invalid redaction cut".into());
        }
        let stop = byte_at(source, end);
        let mut position = byte_at(source, cut);
        if let Some(begin) = source[..position].rfind("-----BEGIN ") {
            let header = rule("PRIVATE_HEADER")?
                .find(&source[begin..stop])
                .map_err(|error| error.to_string())?;
            if header.is_some_and(|matched| matched.start() == 0)
                && !source[begin..position].contains("-----END ")
            {
                position = source[position..stop]
                    .find("-----END ")
                    .and_then(|close| {
                        let close = position + close + 9;
                        source[close..stop]
                            .find("-----")
                            .map(|tail| close + tail + 5)
                    })
                    .unwrap_or(stop);
            }
        }
        for secret in known_secrets {
            if secret.is_empty() {
                continue;
            }
            let mut low = position.saturating_sub(secret.len());
            while !source.is_char_boundary(low) {
                low += 1;
            }
            let mut high = position.saturating_add(secret.len()).min(stop);
            while !source.is_char_boundary(high) {
                high -= 1;
            }
            for (first, _) in source[low..high].match_indices(*secret) {
                let first = low + first;
                if first < position && first + secret.len() > position {
                    position = first + secret.len();
                }
            }
        }
        if position > 0 && position < stop {
            let before = source[..position]
                .chars()
                .next_back()
                .ok_or("missing cut context")?;
            let after = source[position..stop]
                .chars()
                .next()
                .ok_or("missing cut context")?;
            if full_rule("TOKEN_CHAR", &before.to_string())?
                && full_rule("TOKEN_CHAR", &after.to_string())?
            {
                position = rule("NOT_TOKEN")?
                    .find(&source[position..stop])
                    .map_err(|error| error.to_string())?
                    .map_or(stop, |matched| position + matched.start());
            }
        }
        Ok(source[..position].chars().count())
    }

fn byte_at(text: &str, index: usize) -> usize { text.char_indices().nth(index).map_or(text.len(), |(index, _)| index) }
fn substring(text: &str, start: usize, end: usize) -> &str { &text[byte_at(text, start)..byte_at(text, end)] }
