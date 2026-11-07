//! Structured redaction and bounded windows. Unredacted windows remain private until clean.
use super::{Kind, Result, full_rule, has, label_kind, plausible, policy, rule, spans};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use tl3_harness_core::summary::{PythonEnvelope, PythonText, SummarySerializer, SummaryValue};

pub const PAYLOAD_BYTES: usize = 65_536;
pub const STRING_CHARS: usize = 8192;
const ITEM_BYTES: usize = 16_384;
const LEAD_NOTE: &str = "[truncated: earlier text omitted]\n";
const BUDGET_NOTE: &str = "[truncated: content budget]";

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Window {
    source: String,
    start: usize,
    end: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Clip {
    parts: Vec<Window>,
    lead: bool,
    gap: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Shaped {
    Null,
    Bool(bool),
    Number(String),
    Text(Clip),
    Array(Vec<Shaped>),
    Object(Vec<(String, Shaped)>),
    Json {
        value: Box<Shaped>,
        raw: Option<String>,
    },
}

fn scalar(text: &PythonText) -> Result<String> {
    text.codepoints()
        .iter()
        .map(|point| {
            char::from_u32(*point).ok_or_else(|| "redaction requires Unicode scalars".into())
        })
        .collect()
}
fn as_text(value: &SummaryValue) -> Option<String> {
    if let SummaryValue::String(text) = value {
        scalar(text).ok()
    } else {
        None
    }
}
fn byte_at(text: &str, index: usize) -> usize {
    text.char_indices()
        .nth(index)
        .map_or(text.len(), |(index, _)| index)
}
fn substring(text: &str, start: usize, end: usize) -> &str {
    &text[byte_at(text, start)..byte_at(text, end)]
}
fn measured(value: &SummaryValue) -> Result<usize> {
    PythonEnvelope
        .serialize(value)
        .map(|bytes| bytes.len())
        .map_err(|error| format!("redaction serialization: {error:?}"))
}
fn parse(text: &str) -> Result<SummaryValue> {
    SummaryValue::from_python_json(text).map_err(|error| format!("redaction JSON: {error:?}"))
}
fn shown(clip: &Clip) -> String {
    let middle = format!(" [truncated: {} characters omitted] ", clip.gap);
    let mut text = if clip.lead {
        LEAD_NOTE.to_owned()
    } else {
        String::new()
    };
    text.push_str(
        &clip
            .parts
            .iter()
            .map(|part| substring(&part.source, part.start, part.end))
            .collect::<Vec<_>>()
            .join(&middle),
    );
    text
}
impl Shaped {
    pub fn validate(&self, depth: usize) -> Result<()> {
        if depth > 24 {
            return Err("shaped redaction depth exceeds bound".into());
        }
        match self {
            Self::Text(clip) => {
                if clip.parts.is_empty() || clip.parts.len() > 2 {
                    return Err("invalid shaped windows".into());
                }
                for part in &clip.parts {
                    if part.start > part.end
                        || part.end > part.source.chars().count()
                        || part.source.len() > super::LIMIT
                    {
                        return Err("invalid shaped window boundary".into());
                    }
                }
            }
            Self::Array(items) => {
                for item in items {
                    item.validate(depth + 1)?;
                }
            }
            Self::Object(fields) => {
                for (_, item) in fields {
                    item.validate(depth + 1)?;
                }
            }
            Self::Json { value, .. } => value.validate(depth + 1)?,
            Self::Number(value) => {
                parse(value)?;
            }
            _ => (),
        }
        Ok(())
    }
    fn displayed(&self) -> Result<SummaryValue> {
        Ok(match self {
            Self::Null => SummaryValue::Null,
            Self::Bool(value) => SummaryValue::Bool(*value),
            Self::Number(value) => parse(value)?,
            Self::Text(value) => SummaryValue::String(PythonText::from(shown(value).as_str())),
            Self::Array(items) => {
                SummaryValue::Array(items.iter().map(Self::displayed).collect::<Result<_>>()?)
            }
            Self::Object(fields) => SummaryValue::Object(
                fields
                    .iter()
                    .map(|(key, value)| Ok((PythonText::from(key.as_str()), value.displayed()?)))
                    .collect::<Result<_>>()?,
            ),
            Self::Json { value, .. } => value.displayed()?,
        })
    }
    pub fn size(&self) -> Result<usize> {
        self.validate(0)?;
        measured(&self.displayed()?)
    }
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
pub fn file_marker(path: &str) -> String {
    format!("[omitted: content of sensitive file {path}]")
}
fn sensitive_path(value: &SummaryValue) -> Result<Option<String>> {
    let mut pending = vec![value];
    for _ in 0..10_000 {
        let Some(value) = pending.pop() else {
            break;
        };
        match value {
            SummaryValue::Object(fields) => {
                for (key, value) in fields {
                    if has("PATH_KEYS", &scalar(key)?)?
                        && let Some(path) = as_text(value)
                        && sensitive_file(&path)?
                    {
                        return Ok(Some(path));
                    }
                    pending.push(value);
                }
            }
            SummaryValue::Array(items) => pending.extend(items),
            _ => (),
        }
    }
    Ok(None)
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Redactor {
    pub secrets: BTreeSet<String>,
    pub calls: BTreeMap<String, String>,
    pub truncated: bool,
}
impl Redactor {
    pub fn window(&self, source: &str, start: usize, end: usize) -> Result<String> {
        let length = source.chars().count();
        if start > end || end > length {
            return Err("invalid redaction window".into());
        }
        let low = start.saturating_sub(policy().context);
        let high = end.saturating_add(policy().context).min(length);
        let text = substring(source, low, high);
        if text.len() > super::LIMIT {
            return Err("redaction window exceeds one MiB".into());
        }
        let first = byte_at(text, start - low);
        let stop = byte_at(text, end - low);
        let mut found = spans(text)?;
        for secret in &self.secrets {
            if !secret.is_empty() {
                found.extend(
                    text.match_indices(secret)
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
    fn cut(&self, source: &str, start: usize, end: usize, limit: usize) -> Result<usize> {
        let mut cut = end.saturating_sub(limit).max(start);
        let text = substring(source, start, end);
        let relative = byte_at(text, cut - start);
        for (first, last) in spans(text)? {
            if first < relative && last > relative {
                cut = start + text[..last].chars().count();
            }
        }
        for secret in &self.secrets {
            if !secret.is_empty() {
                for (first, _) in text.match_indices(secret) {
                    if first < relative && first + secret.len() > relative {
                        cut = start + text[..first + secret.len()].chars().count();
                    }
                }
            }
        }
        Ok(cut.min(end))
    }
    fn clip(&self, value: &str, limit: usize) -> Result<Clip> {
        let end = value.chars().count();
        let parts;
        let mut lead = false;
        let mut gap = 0;
        if end <= limit {
            parts = vec![(0, end)];
        } else {
            let cut = end.saturating_sub(limit);
            let tail = substring(value, cut, end);
            if let Some(newline) = tail.find('\n')
                && tail[newline..].chars().count() > limit / 2
            {
                parts = vec![(cut + tail[..newline + 1].chars().count(), end)];
                lead = true;
            } else {
                let head = limit / 4;
                let begin = self.cut(value, head, end, limit.saturating_sub(head))?;
                parts = vec![(0, head), (begin, end)];
                gap = begin.saturating_sub(head);
            }
        }
        Ok(Clip {
            parts: parts
                .into_iter()
                .map(|(start, stop)| {
                    let low = start.saturating_sub(policy().context);
                    let high = stop.saturating_add(policy().context).min(end);
                    Window {
                        source: substring(value, low, high).to_owned(),
                        start: start - low,
                        end: stop - low,
                    }
                })
                .collect(),
            lead,
            gap,
        })
    }
    fn clean_clip(&mut self, value: &Clip, limit: usize) -> Result<String> {
        let mut out = if value.lead {
            LEAD_NOTE.to_owned()
        } else {
            String::new()
        };
        for (index, part) in value.parts.iter().enumerate() {
            if index != 0 {
                out.push_str(&format!(" [truncated: {} characters omitted] ", value.gap));
            }
            out.push_str(&self.window(&part.source, part.start, part.end)?);
        }
        self.truncated |= value.lead || value.parts.len() > 1;
        if measured(&SummaryValue::String(PythonText::from(out.as_str())))? <= limit {
            return Ok(out);
        }
        self.truncated = true;
        Ok(BUDGET_NOTE.into())
    }
    pub fn text(&mut self, value: &str, limit: usize) -> Result<String> {
        let clip = self.clip(value, limit.min(STRING_CHARS))?;
        self.clean_clip(&clip, PAYLOAD_BYTES)
    }
    fn note_calls(&mut self, value: &SummaryValue) -> Result<()> {
        let mut pending = vec![value];
        for _ in 0..200_000 {
            let Some(value) = pending.pop() else {
                break;
            };
            match value {
                SummaryValue::Object(fields) => {
                    let call = value.field("id").and_then(as_text);
                    let args = value
                        .field("function")
                        .and_then(|value| value.field("arguments"))
                        .or_else(|| value.field("args"))
                        .or_else(|| value.field("arguments"));
                    if let (Some(call), Some(args)) = (call, args) {
                        let decoded = as_text(args).and_then(|text| parse(&text).ok());
                        if let Some(path) = sensitive_path(decoded.as_ref().unwrap_or(args))? {
                            self.calls.insert(call, path);
                        }
                    }
                    pending.extend(fields.iter().map(|(_, value)| value));
                }
                SummaryValue::Array(items) => pending.extend(items),
                _ => (),
            }
        }
        Ok(())
    }
    pub fn shape(&mut self, input_json: &str, limit: usize) -> Result<Shaped> {
        let value = parse(input_json)?;
        self.note_calls(&value)?;
        self.shape_value(&value, limit.min(PAYLOAD_BYTES), 0)
    }
    pub fn reshape(&mut self, value: &Shaped, limit: usize) -> Result<Shaped> {
        value.validate(0)?;
        self.reshape_value(value, limit.min(PAYLOAD_BYTES), 0)
    }
    fn reshape_value(&mut self, value: &Shaped, limit: usize, depth: usize) -> Result<Shaped> {
        if depth > 20 || limit < 128 {
            self.truncated = true;
            return Ok(Shaped::Text(self.clip(BUDGET_NOTE, STRING_CHARS)?));
        }
        Ok(match value {
            Shaped::Text(clip) => {
                let total: usize = clip.parts.iter().map(|part| part.end - part.start).sum();
                let cap = STRING_CHARS.min(limit.saturating_sub(64));
                if total <= cap {
                    return Ok(value.clone());
                }
                let first = &clip.parts[0];
                let last = clip.parts.last().ok_or("absent shaped window")?;
                let head_end = (first.start + cap / 4).min(first.end);
                let tail_start = self.cut(&last.source, last.start, last.end, cap - cap / 4)?;
                let parts = [(first, first.start, head_end), (last, tail_start, last.end)]
                    .into_iter()
                    .map(|(old, start, end)| {
                        let low = start.saturating_sub(policy().context);
                        let high = end
                            .saturating_add(policy().context)
                            .min(old.source.chars().count());
                        Window {
                            source: substring(&old.source, low, high).to_owned(),
                            start: start - low,
                            end: end - low,
                        }
                    })
                    .collect();
                Shaped::Text(Clip {
                    parts,
                    lead: clip.lead,
                    gap: clip.gap.saturating_add(
                        total.saturating_sub(head_end - first.start + last.end - tail_start),
                    ),
                })
            }
            Shaped::Array(items) => {
                let mut remaining = limit.saturating_sub(96);
                let mut result = vec![];
                for (index, item) in items.iter().rev().enumerate() {
                    if remaining < 160 {
                        self.truncated = true;
                        break;
                    }
                    let share = remaining.min(ITEM_BYTES.max(remaining / (items.len() - index)));
                    let item = self.reshape_value(item, share, depth + 1)?;
                    remaining = remaining.saturating_sub(item.size()?.saturating_add(2));
                    result.push(item);
                }
                result.reverse();
                Shaped::Array(result)
            }
            Shaped::Object(fields) => {
                let mut remaining = limit.saturating_sub(96);
                let mut result = vec![];
                for (index, (key, item)) in fields.iter().rev().enumerate() {
                    if remaining < 160 {
                        self.truncated = true;
                        break;
                    }
                    let share = remaining
                        .saturating_sub(key.len() + 8)
                        .min(ITEM_BYTES.max(remaining / (fields.len() - index)));
                    let item = self.reshape_value(item, share, depth + 1)?;
                    remaining =
                        remaining.saturating_sub(item.size()?.saturating_add(key.len() + 8));
                    result.push((key.clone(), item));
                }
                result.reverse();
                Shaped::Object(result)
            }
            Shaped::Json { value, .. } => Shaped::Json {
                value: Box::new(self.reshape_value(value, limit / 2, depth + 1)?),
                raw: None,
            },
            other => other.clone(),
        })
    }
    fn shape_value(&mut self, value: &SummaryValue, limit: usize, depth: usize) -> Result<Shaped> {
        if limit < 128 || depth > 20 {
            self.truncated = true;
            return Ok(Shaped::Text(self.clip(BUDGET_NOTE, STRING_CHARS)?));
        }
        Ok(match value {
            SummaryValue::Null => Shaped::Null,
            SummaryValue::Bool(value) => Shaped::Bool(*value),
            SummaryValue::Number(_) => Shaped::Number(
                String::from_utf8(
                    PythonEnvelope
                        .serialize(value)
                        .map_err(|error| format!("{error:?}"))?,
                )
                .map_err(|error| error.to_string())?,
            ),
            SummaryValue::String(value) => {
                let text = scalar(value)?;
                if let Ok(decoded) = parse(&text) {
                    Shaped::Json {
                        value: Box::new(self.shape_value(&decoded, limit / 2, depth + 1)?),
                        raw: (text.chars().count() <= STRING_CHARS).then_some(text),
                    }
                } else {
                    Shaped::Text(self.clip(&text, STRING_CHARS.min(limit.saturating_sub(64)))?)
                }
            }
            SummaryValue::Array(items) => {
                let mut remaining = limit.saturating_sub(96);
                let mut result = vec![];
                for (index, value) in items.iter().rev().enumerate() {
                    if remaining < 160 {
                        self.truncated = true;
                        break;
                    }
                    let share = remaining.min(ITEM_BYTES.max(remaining / (items.len() - index)));
                    let item = self.shape_value(value, share, depth + 1)?;
                    remaining = remaining.saturating_sub(item.size()?.saturating_add(2));
                    result.push(item);
                }
                if result.len() < items.len() {
                    result.push(Shaped::Text(
                        self.clip("[truncated: earlier items omitted]", STRING_CHARS)?,
                    ));
                }
                result.reverse();
                Shaped::Array(result)
            }
            SummaryValue::Object(fields) => {
                let path = fields
                    .iter()
                    .find_map(|(key, value)| {
                        let key = scalar(key).ok()?;
                        let path = as_text(value)?;
                        (has("PATH_KEYS", &key).ok()? && sensitive_file(&path).ok()?)
                            .then_some(path)
                    })
                    .or_else(|| {
                        value
                            .field("tool_call_id")
                            .and_then(as_text)
                            .and_then(|id| self.calls.get(&id).cloned())
                    });
                let named = ["name", "key", "Name", "Key"]
                    .iter()
                    .find_map(|key| value.field(key).and_then(as_text))
                    .unwrap_or_default();
                let mut remaining = limit.saturating_sub(96);
                let mut result = vec![];
                for (index, (key, value)) in fields.iter().rev().enumerate() {
                    let key = scalar(key)?;
                    if has("SKIPPED_KEYS", &key)? {
                        continue;
                    }
                    if remaining < 160 {
                        self.truncated = true;
                        result.push((
                            "tl3.truncated".into(),
                            Shaped::Text(self.clip("earlier fields omitted", STRING_CHARS)?),
                        ));
                        break;
                    }
                    let kind = if key.chars().count() > 128 {
                        Some(Kind::Strong)
                    } else {
                        label_kind(&key)?
                    };
                    let substitute = if let Some(path) = &path
                        && has("CONTENT_KEYS", &key)?
                    {
                        Some(file_marker(path))
                    } else if matches!(kind, Some(Kind::Strong))
                        && !matches!(value, SummaryValue::Null | SummaryValue::Bool(_))
                        || matches!(kind, Some(Kind::Weak))
                            && as_text(value).is_some_and(|text| {
                                plausible(Some(Kind::Weak), &key, &text, "", "", false)
                                    .unwrap_or(true)
                            })
                        || matches!(key.as_str(), "value" | "Value")
                            && label_kind(&named)? == Some(Kind::Strong)
                            && matches!(value, SummaryValue::String(_) | SummaryValue::Number(_))
                    {
                        Some(policy().redacted.into())
                    } else {
                        None
                    };
                    let key: String = key.chars().take(128).collect();
                    let share = remaining
                        .saturating_sub(key.len() + 8)
                        .min(ITEM_BYTES.max(remaining / (fields.len() - index)));
                    let item = if let Some(text) = substitute {
                        Shaped::Text(self.clip(&text, STRING_CHARS)?)
                    } else {
                        self.shape_value(value, share, depth + 1)?
                    };
                    remaining = remaining
                        .saturating_sub(key.len().saturating_add(item.size()?).saturating_add(8));
                    result.push((key, item));
                }
                result.reverse();
                Shaped::Object(result)
            }
        })
    }
    fn clean_value(&mut self, value: &Shaped, limit: usize, depth: usize) -> Result<SummaryValue> {
        if depth > 20 || limit < 128 {
            self.truncated = true;
            return Ok(SummaryValue::String(PythonText::from(BUDGET_NOTE)));
        }
        Ok(match value {
            Shaped::Text(value) => {
                SummaryValue::String(PythonText::from(self.clean_clip(value, limit)?.as_str()))
            }
            Shaped::Json { value, raw } => {
                let cleaned = self.clean_value(value, limit / 2, depth + 1)?;
                let encoded = if let Some(raw) = raw
                    && parse(raw).ok().as_ref() == Some(&cleaned)
                {
                    self.text(raw, STRING_CHARS)?
                } else {
                    String::from_utf8(
                        PythonEnvelope
                            .serialize(&cleaned)
                            .map_err(|error| format!("{error:?}"))?,
                    )
                    .map_err(|error| error.to_string())?
                };
                SummaryValue::String(PythonText::from(encoded.as_str()))
            }
            Shaped::Array(items) => {
                let mut remaining = limit.saturating_sub(96);
                let mut result = vec![];
                for (index, item) in items.iter().rev().enumerate() {
                    if remaining < 160 {
                        self.truncated = true;
                        result.push(SummaryValue::String(PythonText::from(
                            "[truncated: earlier items omitted]",
                        )));
                        break;
                    }
                    let share = remaining.min(ITEM_BYTES.max(remaining / (items.len() - index)));
                    let item = self.clean_value(item, share, depth + 1)?;
                    remaining = remaining.saturating_sub(measured(&item)?.saturating_add(2));
                    result.push(item);
                }
                result.reverse();
                SummaryValue::Array(result)
            }
            Shaped::Object(fields) => {
                let mut remaining = limit.saturating_sub(96);
                let mut result = vec![];
                for (index, (key, value)) in fields.iter().rev().enumerate() {
                    if remaining < 160 {
                        self.truncated = true;
                        result.push((
                            PythonText::from("tl3.truncated"),
                            SummaryValue::String(PythonText::from("earlier fields omitted")),
                        ));
                        break;
                    }
                    let key = self.text(key, 128)?;
                    let share = remaining
                        .saturating_sub(key.len() + 8)
                        .min(ITEM_BYTES.max(remaining / (fields.len() - index)));
                    let item = self.clean_value(value, share, depth + 1)?;
                    remaining = remaining.saturating_sub(
                        key.len().saturating_add(measured(&item)?).saturating_add(8),
                    );
                    result.push((PythonText::from(key.as_str()), item));
                }
                result.reverse();
                SummaryValue::Object(result)
            }
            other => other.displayed()?,
        })
    }
    pub fn clean(&mut self, value: &Shaped, limit: usize) -> Result<String> {
        value.validate(0)?;
        let value = self.clean_value(value, limit.min(PAYLOAD_BYTES), 0)?;
        let bytes = PythonEnvelope
            .serialize(&value)
            .map_err(|error| format!("{error:?}"))?;
        if bytes.len() > limit.min(PAYLOAD_BYTES) {
            return Err("redacted payload exceeds publication byte budget".into());
        }
        String::from_utf8(bytes).map_err(|error| error.to_string())
    }
    pub fn payload(&mut self, input_json: &str, limit: usize) -> Result<String> {
        let shaped = self.shape(input_json, PAYLOAD_BYTES)?;
        self.clean(&shaped, limit)
    }
}

pub fn sensitive_path_json(text: &str) -> Result<Option<String>> {
    sensitive_path(&parse(text)?)
}
pub fn measure_json(text: &str) -> Result<usize> {
    measured(&parse(text)?)
}
