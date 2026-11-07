//! Trace retention and allocation from primitive MLflow/protobuf observations.
use crate::structured::{PAYLOAD_BYTES, Redactor, Shaped};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
const TRACE_BYTES: usize = 1024 * 1024;
const FLOOR_BYTES: usize = 4096;
const FIRST_SPANS: usize = 1024;
const LAST_SPANS: usize = 3072;
const SHAPED_BYTES: usize = 4 * TRACE_BYTES;
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    root: bool,
    kind: String,
    proto_bytes: usize,
    sizes: Vec<usize>,
    shrunk: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Usage {
        value: serde_json::Value,
    },
    Label {
        kind: LabelKind,
        value: String,
    },
    Retain {
        entries: Vec<Entry>,
        shrinking: bool,
    },
    Allocate {
        entries: Vec<Entry>,
    },
    Live {
        enabled: bool,
        root: bool,
        spans: usize,
        used: usize,
        contents: Vec<Shaped>,
    },
    Packet {
        bytes: usize,
        entries: Vec<PacketEntry>,
    },
    Fits {
        bytes: usize,
        next: Option<usize>,
    },
    Clean {
        contents: Vec<Shaped>,
        limits: Vec<Option<usize>>,
    },
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LabelKind {
    Span,
    Attribute,
    Tag,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PacketEntry {
    root: bool,
    content_bytes: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Decision {
    Usage {
        value: serde_json::Value,
    },
    Label {
        text: String,
    },
    Retain {
        keep: Vec<usize>,
        shrink: Option<usize>,
        shrinking: bool,
        floor_bytes: usize,
    },
    Allocate {
        limits: Vec<Vec<usize>>,
        truncated: bool,
    },
    Live {
        values: Option<Vec<serde_json::Value>>,
        used: usize,
        spans: usize,
    },
    Packet {
        remove: Vec<usize>,
        root_only: bool,
    },
    Fits {
        fits: bool,
    },
    Clean {
        values: Vec<serde_json::Value>,
    },
}
fn exhausted() -> serde_json::Value {
    serde_json::json!({"truncated":true,"note":"trace content budget exhausted"})
}
pub fn decide(request: Request, redactor: &mut Redactor) -> Result<Decision, String> {
    Ok(match request {
        Request::Usage { value } => Decision::Usage {
            value: crate::structured::usage_counts(&value)?,
        },
        Request::Label { kind, value } => {
            let limit = match kind {
                LabelKind::Span | LabelKind::Attribute => 256,
                LabelKind::Tag => 512,
            };
            let safe = redactor.text(&value, crate::structured::STRING_CHARS)?;
            Decision::Label {
                text: safe.chars().take(limit).collect(),
            }
        }
        Request::Retain { entries, shrinking } => {
            let keep: Vec<_> = (0..entries.len())
                .filter(|index| {
                    *index < FIRST_SPANS || index.saturating_add(LAST_SPANS) >= entries.len()
                })
                .collect();
            let total = keep
                .iter()
                .try_fold(0usize, |sum, index| {
                    entries[*index]
                        .sizes
                        .iter()
                        .try_fold(sum, |sum, size| sum.checked_add(*size))
                })
                .ok_or("trace size overflow")?;
            let shrinking = (shrinking || total > SHAPED_BYTES) && total > SHAPED_BYTES / 2;
            let shrink = if shrinking {
                keep.iter()
                    .rev()
                    .skip(1)
                    .rev()
                    .find(|index| !entries[**index].shrunk)
                    .copied()
            } else {
                None
            };
            Decision::Retain {
                keep,
                shrink,
                shrinking,
                floor_bytes: FLOOR_BYTES,
            }
        }
        Request::Allocate { entries } => {
            if entries.iter().filter(|entry| entry.root).count() != 1 {
                return Err("trace allocation requires one root".into());
            }
            let mut budget = TRACE_BYTES as i128 - 32768;
            let mut limits: Vec<Vec<usize>> = entries
                .iter()
                .map(|entry| vec![0; entry.sizes.len()])
                .collect();
            for (index, entry) in entries.iter().enumerate() {
                budget -= entry.proto_bytes as i128 + 16;
                if entry.root {
                    for (item, size) in entry.sizes.iter().enumerate() {
                        let limit = PAYLOAD_BYTES.min(size.saturating_add(256));
                        limits[index][item] = limit;
                        budget -= limit as i128;
                    }
                }
            }
            let order: Vec<_> = [false, true]
                .into_iter()
                .flat_map(|chain| {
                    entries
                        .iter()
                        .enumerate()
                        .rev()
                        .filter(move |(_, entry)| !entry.root && (entry.kind == "CHAIN") == chain)
                        .flat_map(|(index, entry)| {
                            (0..entry.sizes.len()).map(move |item| (index, item))
                        })
                })
                .collect();
            let floor = FLOOR_BYTES.min((budget.max(0) / 2) as usize / order.len().max(1));
            let mut pool = budget - (floor * order.len()) as i128;
            let mut truncated = false;
            for (index, item) in order {
                let need = entries[index].sizes[item].saturating_add(256);
                let mut limit = need;
                if need <= floor {
                    pool += (floor - need) as i128;
                } else {
                    let extra = (need - floor).min(pool.max(0) as usize);
                    limit = floor + extra;
                    pool -= extra as i128;
                    truncated = true;
                    if limit < 512 {
                        limit = 0;
                    }
                }
                limits[index][item] = limit;
            }
            Decision::Allocate { limits, truncated }
        }
        Request::Live {
            enabled,
            root,
            spans,
            mut used,
            contents,
        } => {
            if !enabled || root || spans >= FIRST_SPANS {
                return Ok(Decision::Live {
                    values: None,
                    used,
                    spans,
                });
            }
            let mut values = vec![];
            for content in contents {
                let limit = PAYLOAD_BYTES.min(TRACE_BYTES.saturating_sub(used));
                let value = if limit >= 512 {
                    serde_json::from_str(&redactor.clean(&content, limit)?)
                        .map_err(|_| "invalid redacted trace JSON")?
                } else {
                    exhausted()
                };
                used = used
                    .checked_add(crate::structured::measure_json(&value.to_string())?)
                    .ok_or("trace size overflow")?;
                values.push(value);
            }
            Decision::Live {
                values: Some(values),
                used,
                spans: spans + 1,
            }
        }
        Request::Packet { bytes, entries } => {
            let mut remaining = bytes;
            let mut remove = vec![];
            for (index, entry) in entries.iter().enumerate() {
                if remaining <= TRACE_BYTES {
                    break;
                }
                if !entry.root {
                    remaining = remaining.saturating_sub(entry.content_bytes);
                    remove.push(index);
                }
            }
            Decision::Packet {
                remove,
                root_only: remaining > TRACE_BYTES,
            }
        }
        Request::Fits { bytes, next } => Decision::Fits {
            fits: bytes
                .checked_add(next.map_or(0, |next| next.saturating_add(16)))
                .is_some_and(|bytes| bytes <= TRACE_BYTES),
        },
        Request::Clean { contents, limits } => {
            if contents.len() != limits.len() {
                return Err("trace content limits differ from observations".into());
            }
            let mut values = vec![];
            for (content, limit) in contents.iter().zip(limits) {
                let limit = limit.unwrap_or(PAYLOAD_BYTES);
                values.push(if limit == 0 {
                    exhausted()
                } else {
                    serde_json::from_str(&redactor.clean(content, limit)?)
                        .map_err(|_| "invalid redacted trace JSON")?
                });
            }
            Decision::Clean { values }
        }
    })
}
