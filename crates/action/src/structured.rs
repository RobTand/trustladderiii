//! Structured redaction and bounded windows. Unredacted windows remain private until clean.
use tl3_redaction::{
    FieldRole, FieldValue, context_chars, field_is_secret, field_matches, redacted_marker,
    sensitive_file,
};
type Result<T> = std::result::Result<T, String>;
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
    Ok(utf8_json(value)?.len())
}
// Trace content uses Python's ensure_ascii=False, independently of the proved
// summary envelope. Retain object order, numeric atoms and default separators.
fn utf8_json(value: &SummaryValue) -> Result<String> {
    Ok(match value {
        SummaryValue::Null => "null".into(),
        SummaryValue::Bool(value) => value.to_string(),
        SummaryValue::Number(value) => value.as_str().into(),
        SummaryValue::String(value) => {
            serde_json::to_string(&scalar(value)?).map_err(|error| error.to_string())?
        }
        SummaryValue::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(utf8_json)
                .collect::<Result<Vec<_>>>()?
                .join(", ")
        ),
        SummaryValue::Object(fields) => format!(
            "{{{}}}",
            fields
                .iter()
                .map(|(key, value)| {
                    Ok(format!(
                        "{}: {}",
                        utf8_json(&SummaryValue::String(key.clone()))?,
                        utf8_json(value)?
                    ))
                })
                .collect::<Result<Vec<_>>>()?
                .join(", ")
        ),
    })
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
                        || part.source.len() > tl3_redaction::INPUT_BYTE_LIMIT
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
                    if field_matches(FieldRole::Path, &scalar(key)?)?
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
        tl3_redaction::window(
            source,
            start..end,
            &self.secrets.iter().map(String::as_str).collect::<Vec<_>>(),
        )
    }
    fn settle(&self, source: &str, cut: usize, end: usize) -> Result<usize> {
        tl3_redaction::settled_cut(
            source,
            cut,
            end,
            &self.secrets.iter().map(String::as_str).collect::<Vec<_>>(),
        )
    }
    fn cut(&self, source: &str, start: usize, end: usize, limit: usize) -> Result<usize> {
        self.settle(source, end.saturating_sub(limit).max(start), end)
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
                parts = vec![(
                    self.settle(value, cut + tail[..newline + 1].chars().count(), end)?,
                    end,
                )];
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
                    let low = start.saturating_sub(context_chars());
                    let high = stop.saturating_add(context_chars()).min(end);
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
    fn reclip(&self, value: &Clip, limit: usize) -> Result<Clip> {
        let total: usize = value.parts.iter().map(|part| part.end - part.start).sum();
        if total <= limit {
            return Ok(value.clone());
        }
        let tail = |part: &Window, limit: usize| -> Result<(usize, bool)> {
            let cut = part.end.saturating_sub(limit).max(part.start);
            let suffix = substring(&part.source, cut, part.end);
            if let Some(newline) = suffix.find('\n') {
                let newline = cut + suffix[..newline].chars().count();
                if part.end - newline > limit / 2 {
                    return Ok((self.settle(&part.source, newline + 1, part.end)?, true));
                }
            }
            Ok((self.settle(&part.source, cut, part.end)?, false))
        };
        let first = &value.parts[0];
        let mut result = value.clone();
        let head = limit / 4;
        if value.parts.len() == 1 {
            let (begin, whole_lines) = tail(first, limit)?;
            let mut newest = first.clone();
            newest.start = begin;
            if whole_lines {
                result.parts = vec![newest];
                result.lead = true;
            } else {
                let mut oldest = first.clone();
                oldest.end = first.start + head;
                result.parts = vec![oldest, newest];
                result.gap = begin.saturating_sub(first.start + head);
            }
        } else {
            let last = &value.parts[1];
            let mut oldest = first.clone();
            oldest.end = first.end.min(first.start + head);
            let mut newest = last.clone();
            newest.start = tail(last, limit - head)?.0;
            result.gap = result
                .gap
                .checked_add(first.end - oldest.end + newest.start - last.start)
                .ok_or("shaped redaction gap overflow")?;
            result.parts = vec![oldest, newest];
        }
        Ok(result)
    }
    fn clean_leaf_clip(&mut self, value: &Clip, limit: usize) -> Result<String> {
        let mut chars = STRING_CHARS.min(limit.saturating_sub(64));
        for _ in 0..4 {
            if chars < 32 {
                break;
            }
            let clipped = self.reclip(value, chars)?;
            let out = self.clean_clip(&clipped, PAYLOAD_BYTES)?;
            let used = measured(&SummaryValue::String(PythonText::from(out.as_str())))?;
            if used <= limit.saturating_sub(16) {
                return Ok(out);
            }
            chars = chars.saturating_mul(limit.saturating_sub(64)) / used;
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
                        let low = start.saturating_sub(context_chars());
                        let high = end
                            .saturating_add(context_chars())
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
                    let key_bytes = measured(&SummaryValue::String(PythonText::from(key.as_str())))?;
                    let available = remaining.saturating_sub(key_bytes + 8);
                    let share = available.min(ITEM_BYTES.max(available / (fields.len() - index)));
                    let item = self.reshape_value(item, share, depth + 1)?;
                    remaining = remaining.saturating_sub(item.size()?.saturating_add(key_bytes + 6));
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
                let mut path = None;
                for (key, value) in fields {
                    if field_matches(FieldRole::Path, &scalar(key)?)?
                        && let Some(candidate) = as_text(value)
                        && sensitive_file(&candidate)?
                    {
                        path = Some(candidate);
                        break;
                    }
                }
                let path = path.or_else(|| {
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
                    if field_matches(FieldRole::Skipped, &key)? {
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
                    let scalar_text = as_text(value);
                    let field_value = match value {
                        SummaryValue::Null => FieldValue::Null,
                        SummaryValue::Bool(_) => FieldValue::Bool,
                        SummaryValue::Number(_) => FieldValue::Number,
                        SummaryValue::String(_) => {
                            FieldValue::Text(scalar_text.as_deref().ok_or("invalid field scalar")?)
                        }
                        _ => FieldValue::Container,
                    };
                    let substitute = if let Some(path) = &path
                        && field_matches(FieldRole::Content, &key)?
                    {
                        Some(file_marker(path))
                    } else if field_is_secret(&key, field_value, &named)? {
                        Some(redacted_marker().into())
                    } else {
                        None
                    };
                    let key: String = key.chars().take(128).collect();
                    let key_bytes = measured(&SummaryValue::String(PythonText::from(key.as_str())))?;
                    let available = remaining.saturating_sub(key_bytes + 8);
                    let share = available.min(ITEM_BYTES.max(available / (fields.len() - index)));
                    let item = if let Some(text) = substitute {
                        Shaped::Text(self.clip(&text, STRING_CHARS)?)
                    } else {
                        self.shape_value(value, share, depth + 1)?
                    };
                    remaining = remaining.saturating_sub(key_bytes.saturating_add(item.size()?).saturating_add(6));
                    result.push((key, item));
                }
                result.reverse();
                Shaped::Object(result)
            }
        })
    }
    fn clean_value(&mut self, value: &Shaped, limit: usize, depth: usize) -> Result<SummaryValue> {
        // Source rule: Redactor.walk cleans an already fitting container whole,
        // then checks its actual redacted bytes before using truncation reserves.
        if depth <= 20
            && limit >= 128
            && matches!(value, Shaped::Array(_) | Shaped::Object(_))
            && value.size()? <= limit.saturating_sub(32)
        {
            let result = self.clean_value_inner(value, limit, depth, true)?;
            if measured(&result)? <= limit {
                return Ok(result);
            }
        }
        self.clean_value_inner(value, limit, depth, false)
    }
    fn clean_value_inner(
        &mut self,
        value: &Shaped,
        limit: usize,
        depth: usize,
        roomy: bool,
    ) -> Result<SummaryValue> {
        if depth > 20 || limit < 128 {
            self.truncated = true;
            return Ok(SummaryValue::String(PythonText::from(BUDGET_NOTE)));
        }
        Ok(match value {
            Shaped::Text(value) => SummaryValue::String(PythonText::from(
                self.clean_leaf_clip(value, limit)?.as_str(),
            )),
            Shaped::Json { value, raw } => {
                let cleaned = self.clean_value(value, limit / 2, depth + 1)?;
                let encoded = if let Some(raw) = raw
                    && parse(raw).ok().as_ref() == Some(&cleaned)
                    && raw.chars().count() <= (limit / 6).saturating_sub(64)
                {
                    let clip = self.clip(raw, raw.chars().count())?;
                    self.clean_leaf_clip(&clip, limit)?
                } else {
                    utf8_json(&cleaned)?
                };
                SummaryValue::String(PythonText::from(encoded.as_str()))
            }
            Shaped::Array(items) => {
                let mut remaining = limit.saturating_sub(96);
                let mut result = vec![];
                for (index, item) in items.iter().rev().enumerate() {
                    if remaining < 160 && !roomy {
                        self.truncated = true;
                        result.push(SummaryValue::String(PythonText::from(
                            "[truncated: earlier items omitted]",
                        )));
                        break;
                    }
                    let share = if roomy {
                        item.size()?.saturating_add(512)
                    } else {
                        remaining.min(ITEM_BYTES.max(remaining / (items.len() - index)))
                    };
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
                    if remaining < 160 && !roomy {
                        self.truncated = true;
                        result.push((
                            PythonText::from("tl3.truncated"),
                            SummaryValue::String(PythonText::from("earlier fields omitted")),
                        ));
                        break;
                    }
                    let key = self.text(key, 128)?;
                    let key_bytes = measured(&SummaryValue::String(PythonText::from(key.as_str())))?;
                    let available = remaining.saturating_sub(key_bytes + 8);
                    let share = if roomy {
                        value.size()?.saturating_add(512)
                    } else {
                        available.min(ITEM_BYTES.max(available / (fields.len() - index)))
                    };
                    let item = self.clean_value(value, share, depth + 1)?;
                    remaining = remaining.saturating_sub(
                        key_bytes.saturating_add(measured(&item)?).saturating_add(6),
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
        let text = utf8_json(&value)?;
        if text.len() > limit.min(PAYLOAD_BYTES) {
            return Err("redacted payload exceeds publication byte budget".into());
        }
        Ok(text)
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

/// Numeric usage fields pass through the same authoritative policy as text redaction.
/// Python booleans and negative/noninteger observations are never token counts.
pub fn usage_counts(value: &serde_json::Value) -> Result<serde_json::Value> {
    let mut counts = serde_json::Map::new();
    if let Some(fields) = value.as_object() {
        for (key, count) in fields {
            if field_matches(FieldRole::TokenCount, key)? && count.as_u64().is_some() {
                counts.insert(key.clone(), count.clone());
            }
        }
    }
    Ok(serde_json::Value::Object(counts))
}
