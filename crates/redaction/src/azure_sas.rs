//! Confidential Azure SAS syntax, independent of signature entropy or authenticity.
use std::collections::BTreeMap;

const FIELD_BYTES: usize = 4096;

fn valid_percent_encoding(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() || !bytes[index + 1..index + 3].iter().all(u8::is_ascii_hexdigit) {
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
        if colon < scanned_until { continue; }
        let start = ["https", "http"].into_iter().find_map(|scheme| {
            let start = colon.checked_sub(scheme.len())?;
            text.get(start..colon)?.eq_ignore_ascii_case(scheme).then_some(start)
        });
        let Some(start) = start else { continue; };
        if text[..start].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_') { continue; }
        let end = text[start..].find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '\\' | ')' | ']' | '}')).map_or(text.len(), |offset| start + offset);
        scanned_until = end;
        let raw = &text[start..end];
        let Ok(parsed) = url::Url::parse(raw) else { continue; };
        if !parsed.username().is_empty() || parsed.password().is_some() { continue; }
        let Some(account) = parsed.host_str().and_then(|host| host.strip_suffix(".blob.core.windows.net")) else { continue; };
        if account.is_empty() || account.len() > 63 || !account.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()) { continue; }
        // Use the original query's byte coordinates. URL parsing only establishes authority.
        let query_end = raw.find('#').unwrap_or(raw.len());
        let Some(query_start) = raw[..query_end].find('?').map(|index| index + 1) else { continue; };
        let mut fields = BTreeMap::new();
        let mut offset = query_start;
        let mut valid = true;
        for pair in raw[query_start..query_end].split('&') {
            if let Some((key, value)) = pair.split_once('=') {
                if key.len() > FIELD_BYTES || value.len() > FIELD_BYTES || !valid_percent_encoding(pair) {
                    valid = false;
                    break;
                }
                let mut decoded = url::form_urlencoded::parse(pair.as_bytes());
                let Some((key, value)) = decoded.next() else { valid = false; break; };
                if matches!(key.as_ref(), "sv" | "se" | "ss" | "sr" | "sig") {
                    let span = (start + offset + pair.find('=').unwrap() + 1, start + offset + pair.len());
                    if value.trim().is_empty() || fields.insert(key.into_owned(), span).is_some() {
                        valid = false;
                        break;
                    }
                }
            }
            offset += pair.len() + 1;
        }
        if valid && fields.contains_key("sv") && fields.contains_key("se")
            && (fields.contains_key("ss") || fields.contains_key("sr"))
            && let Some(span) = fields.get("sig") {
                result.push(*span);
            }
    }
    result
}
