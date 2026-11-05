//! #698/#718: retain complete call/result groups and bound their evidence.
//! LangChain supplies immutable serialization facts; every choice happens here.
use super::{Result, number, sum};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Human,
    Ai,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Message {
    pub kind: Kind,
    pub size: u64,
    pub overhead: u64,
    pub arguments_overhead: u64,
    pub text: String,
    pub arguments: Vec<String>,
}
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Clip {
    pub content: String,
    pub arguments: Option<Vec<String>>,
}
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Instruction {
    pub index: usize,
    pub clip: Option<Clip>,
}
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Retained {
    pub prefix_end: usize,
    pub notice: bool,
    pub messages: Vec<Instruction>,
}
fn cost(c: char) -> u64 {
    match c {
        '"' | '\\' | '\u{8}' | '\u{c}' | '\n' | '\r' | '\t' => 2,
        '\0'..='\u{1f}' => 6,
        _ => c.len_utf8() as u64,
    }
}
fn excerpt(text: &str, allowance: u64) -> (&str, u64) {
    let mut used = 0;
    for (index, c) in text.char_indices() {
        let next = cost(c);
        if next > allowance.saturating_sub(used) {
            return (&text[..index], used);
        }
        used += next;
    }
    (text, used)
}

/// #780 extraction: group/argument clipping is decided once over the transcript.
pub fn retain(messages: &[Message], limit: u64) -> Result<Retained> {
    let prefix_end = messages
        .iter()
        .position(|m| m.kind == Kind::Human)
        .ok_or("compaction requires session inputs")?
        + 1;
    let mut groups: Vec<Vec<usize>> = vec![];
    for (index, message) in messages.iter().enumerate().skip(prefix_end) {
        match message.kind {
            Kind::Human => (),
            Kind::Ai => groups.push(vec![index]),
            Kind::Other => {
                if let Some(group) = groups.last_mut() {
                    group.push(index);
                }
            }
        }
    }
    let total: usize = groups.iter().map(Vec::len).sum();
    let mut kept = BTreeMap::new();
    let mut used = 0u64;
    let mut clipped = false;
    for group in groups.iter().rev() {
        let available = i128::from(limit) - i128::from(used);
        let overhead = sum(group.iter().map(|&i| messages[i].overhead))?;
        if i128::from(overhead) > available && !kept.is_empty() {
            break;
        }
        let mut size = sum(group.iter().map(|&i| messages[i].size))?;
        if i128::from(size) > available {
            clipped = true;
            let arguments = i128::from(overhead) > available;
            let overhead = if arguments {
                sum(group.iter().map(|&i| messages[i].arguments_overhead))?
            } else {
                overhead
            };
            let fields = sum([
                group.len() as u64,
                if arguments {
                    sum(group.iter().map(|&i| messages[i].arguments.len() as u64))?
                } else {
                    0
                },
            ])?;
            let each = ((available - i128::from(overhead)).max(0) as u64) / fields;
            size = overhead;
            for &index in group {
                let message = &messages[index];
                let (content, bytes) = excerpt(&message.text, each);
                size = number(size.checked_add(bytes))?;
                let args = if arguments && message.kind == Kind::Ai {
                    let mut args = vec![];
                    for argument in &message.arguments {
                        let (text, bytes) = excerpt(argument, each);
                        size = number(size.checked_add(bytes))?;
                        args.push(text.to_owned());
                    }
                    Some(args)
                } else {
                    None
                };
                kept.insert(
                    index,
                    Some(Clip {
                        content: content.to_owned(),
                        arguments: args,
                    }),
                );
            }
        } else {
            for &index in group {
                kept.insert(index, None);
            }
        }
        used = number(used.checked_add(size))?;
    }
    let notice = clipped || kept.len() < total;
    let messages = messages
        .iter()
        .enumerate()
        .skip(prefix_end)
        .filter_map(|(index, message)| {
            if let Some(clip) = kept.remove(&index) {
                Some(Instruction { index, clip })
            } else if message.kind == Kind::Human {
                Some(Instruction { index, clip: None })
            } else {
                None
            }
        })
        .collect();
    Ok(Retained {
        prefix_end,
        notice,
        messages,
    })
}
