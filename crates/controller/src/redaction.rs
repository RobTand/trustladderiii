//! Rust execution of #302's text policy, generated from its Python definitions.
use fancy_regex::{Captures, Regex};
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
};
type Result<T> = std::result::Result<T, String>;
type Span = (usize, usize);
const LIMIT: usize = 1024 * 1024;
fn policy() -> Result<&'static Value> {
    static POLICY: LazyLock<Result<Value>> = LazyLock::new(|| {
        serde_json::from_str(include_str!("redaction/policy.json")).map_err(|e| e.to_string())
    });
    POLICY.as_ref().map_err(Clone::clone)
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
            for range in policy()?["characters"][name]
                .as_str()
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
    fn shorthand(code: char, classes: &[String; 3], invert: bool) -> String {
        let index = match code {
            'w' | 'W' => 0,
            'd' | 'D' => 1,
            _ => 2,
        };
        let negate = if code.is_ascii_uppercase() ^ invert {
            "^"
        } else {
            ""
        };
        format!("(?-i:[{negate}{}])", classes[index])
    }
    fn bracket(
        source: &mut std::iter::Peekable<std::str::Chars<'_>>,
        classes: &[String; 3],
    ) -> Result<String> {
        let negate = source.next_if_eq(&'^').is_some();
        let mut literal = String::new();
        let mut codes = Vec::new();
        loop {
            match source.next().ok_or("unfinished regex class")? {
                ']' => break,
                '\\' => match source.next().ok_or("unfinished class escape")? {
                    code @ ('w' | 'W' | 'd' | 'D' | 's' | 'S') => codes.push(code),
                    'b' => literal.push_str(r"\x08"),
                    code => {
                        literal.push('\\');
                        literal.push(code);
                    }
                },
                '[' => literal.push_str(r"\["),
                c => literal.push(c),
            }
        }
        if codes.is_empty() {
            return Ok(format!("[{}{literal}]", if negate { "^" } else { "" }));
        }
        if literal.is_empty() && codes.len() == 1 {
            return Ok(shorthand(codes[0], classes, negate));
        }
        let mut parts = codes
            .into_iter()
            .map(|c| shorthand(c, classes, false))
            .collect::<Vec<_>>();
        if !literal.is_empty() {
            parts.push(format!("[{literal}]"));
        }
        let union = parts.join("|");
        Ok(if negate {
            format!("(?!(?:{union}))(?s:.)")
        } else {
            format!("(?:{union})")
        })
    }
    let classes = CLASSES.as_ref().map_err(Clone::clone)?;
    let word = shorthand('w', classes, false);
    let mut out = String::new();
    let mut source = pattern.chars().peekable();
    while let Some(c) = source.next() {
        if c == '[' {
            out.push_str(&bracket(&mut source, classes)?);
            continue;
        }
        if c != '\\' {
            out.push(c);
            continue;
        }
        let escape = source.next().ok_or("unfinished regex escape")?;
        match escape {
            'w' | 'W' | 'd' | 'D' | 's' | 'S' => out.push_str(&shorthand(escape, classes, false)),
            'b' => out.push_str(&format!(r"(?:(?<!{word})(?={word})|(?<={word})(?!{word}))")),
            'B' => out.push_str(&format!(r"(?:(?<!{word})(?!{word})|(?<={word})(?={word}))")),
            other => {
                out.push('\\');
                out.push(other);
            }
        }
    }
    Ok(out)
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
    rx(policy()?["regex"][name]
        .as_str()
        .ok_or_else(|| format!("missing redaction rule {name}"))?)
}
fn has(set: &str, word: &str) -> Result<bool> {
    Ok(policy()?["words"][set]
        .as_array()
        .ok_or("missing redaction words")?
        .iter()
        .any(|v| v.as_str() == Some(word)))
}
fn search(pattern: &str, text: &str) -> Result<bool> {
    rx(pattern)?.is_match(text).map_err(|e| e.to_string())
}
fn full(pattern: &str, text: &str) -> Result<bool> {
    search(&format!(r"\A(?:{pattern})\z"), text)
}
fn full_rule(name: &str, text: &str) -> Result<bool> {
    full(
        policy()?["regex"][name]
            .as_str()
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
        let parts = policy()?["words"]["SECRET_PARTS"]
            .as_array()
            .ok_or("missing secret parts")?;
        if has("SECRET_WORDS", word)?
            || word.ends_with("token")
            || parts
                .iter()
                .filter_map(Value::as_str)
                .any(|part| word.contains(part))
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
    let regex = rx(r#"^[rbfuRBFU]{1,2}(?=["'])"#)?;
    let stripped = regex.replace(value, "");
    let stripped = stripped.trim_start_matches('\\');
    if stripped.chars().count() >= 2 && stripped.starts_with(['"', '\'', '`']) {
        let end = stripped.char_indices().next_back().map_or(0, |(i, _)| i);
        return Ok((stripped[1..end].trim_end_matches('\\').into(), true));
    }
    Ok((value.into(), false))
}
fn placeholder(value: &str) -> Result<bool> {
    if !full(r"[A-Za-z]+(?:[-_ .:][A-Za-z]+)*", value)? {
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
            ordinary &= full("[a-z0-9]*", part)? || part.chars().count() <= 2;
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
            || search(r"\A(?:\s*\+|.*\+\s*$)", &inner)?)
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
        if assign == ":" && full(r"[A-Z]\w*", &inner)? && search(r"\A[ \t]*[<\[,)|=]", tail)? {
            return Ok(false);
        }
        if full_rule("IDENTIFIER", &inner)? && label_kind(&inner)?.is_some() {
            return Ok(false);
        }
        if full_rule("CODE_NAME", &inner)? && code {
            return Ok(false);
        }
        if full_rule("WORD", &inner)? && search(r"\A[ \t]+[^\s#]", tail)? {
            return Ok(false);
        }
        if full(r"[A-Za-z]{2,12}\d{0,3}", &inner)? && search(r"\A[ \t]+[a-z]{2,}", tail)? {
            return Ok(false);
        }
    }
    if kind == Some(Kind::Weak) {
        let sample = prefix(&inner, 64);
        return Ok(random_like(sample, 12)?
            || (full(r"[A-Za-z0-9]{12,}", sample)?
                && sample.chars().any(|c| member(&chars.upper, c))
                && sample.chars().any(|c| member(&chars.lower, c))
                && sample.chars().any(|c| member(&chars.digit, c))));
    }
    Ok(true)
}
fn readable(value: &str) -> Result<bool> {
    let inner = unquote(value)?.0;
    Ok(full(r"[A-Za-z][A-Za-z:/._ -]*", &inner)? && !random_like(&inner, 8)?)
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
        || full(r"[\d.:,/+-]+", core)?
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
fn checked(check: &str, cap: &Captures<'_>, text: &str) -> Result<Option<Span>> {
    let chars = characters()?;
    let label = get(cap, "l");
    let value = get(cap, "v");
    let keep = match check {
        "check_labelled" => {
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
                && matches(&rx(r"[A-Za-z]{2,}[ \t]")?, &text[line..whole.start()])?.len() >= 2
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
        "check_flag" => plausible(label_kind(label)?, label, value, "", "=", false)?,
        "check_table" => {
            let value = value.trim_matches('`');
            label_kind(label.trim_matches(['`', '*', ' ']))?.is_some()
                && !start_rule("REFERENCE", value)?
                && value.chars().count() >= 8
                && token_shaped(value)?
                && !value.chars().any(|c| member(&chars.space, c))
        }
        "check_block" => label_kind(label)? == Some(Kind::Strong),
        "check_name_value" => plausible(label_kind(label)?, label, value, "", "", false)?,
        "check_header" => {
            let group = if cap.name("v").is_some() { "v" } else { "c" };
            let value = get(cap, group);
            let inner = unquote(value)?.0;
            if start_rule("REFERENCE", &inner)?
                || rule("TEMPLATE")?
                    .is_match(&inner)
                    .map_err(|e| e.to_string())?
                || placeholder(&inner)?
                || search(r"\A[A-Za-z_][\w.]*+[ \t]*+(?:[,)}]|$)", value)?
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
        "check_bearer" => token_shaped(value)? && !placeholder(value)?,
        "check_url" => {
            !start_rule("REFERENCE", value)?
                && !rule("TEMPLATE")?
                    .is_match(value)
                    .map_err(|e| e.to_string())?
                && !placeholder(value)?
        }
        "check_user" => {
            !value.chars().all(|c| member(&chars.digit, c))
                && !rule("TEMPLATE")?
                    .is_match(value)
                    .map_err(|e| e.to_string())?
        }
        "check_aws" => label_kind(label)?.is_some(),
        "check_ssh_keygen" => !unquote(value)?.0.is_empty(),
        "check_xml" => {
            chars.trim(value).chars().count() >= 4
                && label_kind(label)? == Some(Kind::Strong)
                && plausible(Some(Kind::Strong), label, value, "", "=", false)?
        }
        "check_json_pair" => {
            unquote(value)?.0.chars().count() >= 4
                && plausible(label_kind(label)?, label, value, "", ":", false)?
        }
        "check_pair" => secretish(value, false)?,
        _ => return Err(format!("unknown redaction check {check}")),
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
    for pattern in policy()?["patterns"].as_array().ok_or("missing patterns")? {
        for cap in matches(&rx(pattern.as_str().ok_or("invalid pattern")?)?, text)? {
            let value = cap
                .name("v")
                .or_else(|| cap.get(0))
                .ok_or("missing redaction match")?;
            found.push((value.start(), value.end()));
        }
    }
    for pair in policy()?["checked"]
        .as_array()
        .ok_or("missing checked patterns")?
    {
        for cap in matches(&rx(pair[0].as_str().ok_or("invalid pattern")?)?, text)? {
            if let Some(span) = checked(pair[1].as_str().ok_or("invalid check")?, &cap, text)? {
                found.push(span);
            }
        }
    }
    for word in matches(&rule("NEAR_WORD")?, text)? {
        let word = word.get(0).ok_or("missing nearby word")?;
        let rest = &text[word.end()..];
        let high = prefix(rest, 48 + 256).len();
        let end = rest[..high].find('\n').unwrap_or(high);
        let limit = prefix(&rest[..end], 48).len();
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
                    header &= full(r"[\w .-]{1,40}", cell)?;
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
            && full("[0-9a-f]+", token)?
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
pub(crate) fn text(value: &str) -> crate::Result<String> {
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
            out.push_str("[REDACTED]");
            cursor = end;
        }
        out.push_str(value.get(cursor..).ok_or("invalid redaction boundary")?);
        Ok(out)
    };
    render().map_err(|error| crate::Error::RedactionInfra(std::io::Error::other(error)))
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
        let status = super::python_command()
            .arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tools/redaction_policy.py"))
            .arg("--check")
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[test]
    fn matches_python_on_the_shared_matrix() {
        use std::io::Write;
        let mut matrix: Vec<String> =
            serde_json::from_str(include_str!("redaction/matrix.json")).unwrap();
        assert_eq!(matrix.len(), 196);
        matrix.extend(
            serde_json::from_str::<Vec<String>>(include_str!("redaction/corpus.json")).unwrap(),
        );
        let mut child = super::python_command()
            .args(["-I", "-c", "import json,sys; from tl3_workers.tracing import Redactor; r=Redactor(None); print(json.dumps([r.text(s,limit=len(s)) for s in json.load(sys.stdin)]))"])
            .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped())
            .spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&serde_json::to_vec(&matrix).unwrap())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        let expected: Vec<String> = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(expected.len(), matrix.len());
        let actual = matrix
            .iter()
            .map(|text| super::text(text).unwrap_or_else(|error| panic!("{text:?}: {error:?}")))
            .collect::<Vec<_>>();
        let mismatches = matrix
            .iter()
            .zip(actual)
            .zip(expected)
            .filter(|((_, actual), expected)| actual != expected)
            .map(|((input, actual), expected)| (input, actual, expected))
            .collect::<Vec<_>>();
        assert!(
            mismatches.is_empty(),
            "Python redaction parity mismatches: {mismatches:?}"
        );
    }
}
