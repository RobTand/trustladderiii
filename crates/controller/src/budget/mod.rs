//! One budget implementation for the controller and synchronous worker loop.
mod compaction;
pub mod decision;
use decision::Price;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

type Result<T> = std::result::Result<T, &'static str>;
fn number(value: Option<u64>) -> Result<u64> {
    value.ok_or("budget arithmetic exceeds its supported range")
}
fn sum(values: impl IntoIterator<Item = u64>) -> Result<u64> {
    number(values.into_iter().try_fold(0u64, u64::checked_add))
}
fn price(value: f64) -> Result<Price> {
    if !value.is_finite() || value < 0.0 {
        return Err("cache weights must be finite and nonnegative");
    }
    let bits = value.to_bits();
    let power = ((bits >> 52) & 0x7ff) as i16;
    Ok(Price {
        significand: (bits & ((1 << 52) - 1)) | if power == 0 { 0 } else { 1 << 52 },
        exponent: if power == 0 { -1074 } else { power - 1023 - 52 },
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Message {
    Ai { calls: Vec<String> },
    Tool { id: String },
    Other,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Forecast {
        brief_tokens: u64,
        file_tokens: BTreeMap<String, u64>,
        overhead_tokens: u64,
        fit_tokens: u64,
        context_tokens: u64,
        problems: u64,
    },
    Execution {
        files: u64,
        generated: u64,
        checks: u64,
        declared: u64,
        exploration: Option<u64>,
        generator: bool,
        tool_turns: u64,
        finalization_turns: u64,
    },
    Cutoff {
        input: Option<u64>,
        output: Option<u64>,
        reasoning: Option<u64>,
        message_characters: u64,
        prompt_characters: u64,
        context: u64,
    },
    CutoffSample {
        input: Option<u64>,
        output: Option<u64>,
        reasoning: Option<u64>,
        visible_characters: Option<u64>,
        fallback_characters: Option<u64>,
        prompt_characters: u64,
        context: u64,
    },
    Account {
        usage: Vec<Value>,
        bound: u64,
        weights: [f64; 3],
        spent: u64,
        turns: u64,
        no_progress: u64,
        r#final: bool,
        repairing: bool,
    },
    Charge {
        tokens: u64,
        read: u64,
        creation: u64,
        weights: [f64; 3],
    },
    Predict {
        context: u64,
        last_input: u64,
        cache_active: bool,
        compact_now: bool,
        weights: [f64; 3],
    },
    Reserve {
        estimate: u64,
        context: u64,
        history_bytes: u64,
        finish_characters: u64,
        cache_active: bool,
        weights: [f64; 3],
    },
    Context {
        estimate: u64,
        last_input: u64,
        last_estimate: Option<i64>,
    },
    Estimate {
        bytes: u64,
        messages: u64,
    },
    Compaction {
        before: u64,
        after: u64,
    },
    CompactNow {
        maker: bool,
        compacted: bool,
        context: u64,
        context_size: u64,
        has_ai: bool,
    },
    ContextFull {
        maker: bool,
        compacted: bool,
        compacted_below: bool,
        compact_now: bool,
        source_len: u64,
        offset: u64,
        context: u64,
        context_size: u64,
    },
    History {
        messages: Vec<Message>,
        start: usize,
    },
    Reply {
        done: bool,
        unfinished: bool,
    },
    Remaining {
        maker: bool,
        turns: u64,
        spent: u64,
        tool_turns: u64,
        input_tokens: u64,
        finalization_turns: u64,
        no_progress_window: u64,
        no_progress_turns: u64,
        last_progress_kind: Option<String>,
        implementation_files: bool,
        implementation_started: bool,
        phase: Option<String>,
        exhaustion: Option<String>,
        finalizing: bool,
        r#final: bool,
    },
    Finalizing {
        maker: bool,
        turns: u64,
        no_progress: u64,
        allowed: u64,
        window: u64,
        reserve: u64,
    },
    Exhaustion {
        maker: bool,
        turns: u64,
        allowed: u64,
        no_progress: u64,
        window: u64,
        context_full: bool,
        spent: u64,
        predicted: u64,
        reserve: u64,
        input_limit: u64,
    },
    Dropped {
        spent: u64,
        in_flight: u64,
        reserve: u64,
        allowed: u64,
    },
    Progress {
        maker: bool,
        window: u64,
        name: String,
        changed: bool,
        fresh: bool,
        evidence_new: Option<bool>,
    },
    ToolAllowed {
        maker: bool,
        finalizing: bool,
        r#final: bool,
        repairing: bool,
        name: String,
    },
    Restore {
        state: Value,
        maker: bool,
    },
    Retained {
        messages: Vec<compaction::Message>,
        limit: u64,
    },
}

fn threshold(context: u64, size: u64) -> bool {
    // Python evaluates size * 0.7 before comparing with the integer context.
    // 0.7 is decoded here; the decision core performs integer IEEE arithmetic.
    let seventy = Price {
        significand: 6_305_039_478_318_694,
        exponent: -53,
    };
    let threshold = Price::integer(size)
        .and_then(|s| s.times(seventy))
        .and_then(Price::ceil);
    threshold.is_none_or(|threshold| context >= threshold)
}
fn cold(weights: [f64; 3], cached: bool) -> Result<Price> {
    let values = weights.map(price);
    // Validate even the weight a particular branch does not use.
    let [uncached, creation, _] = values;
    let uncached = uncached?;
    let creation = creation?;
    values[2]?;
    Ok(if cached && creation.greater(uncached) {
        creation
    } else {
        uncached
    })
}

pub fn decide(request: Request) -> Result<Value> {
    use Request::*;
    Ok(match request {
        Forecast {
            brief_tokens,
            file_tokens,
            overhead_tokens,
            fit_tokens,
            context_tokens,
            problems,
        } => {
            let tokens = sum([
                brief_tokens,
                sum(file_tokens.values().copied())?,
                overhead_tokens,
                number(fit_tokens.checked_mul(problems))?,
            ])?;
            json!({"tokenizer":"o200k_base", "brief_tokens":brief_tokens,"file_tokens":file_tokens,
                "overhead_tokens":overhead_tokens,"tokens":tokens,"fit_tokens":fit_tokens,
                "context_tokens":context_tokens,"fits":decision::fits(tokens,fit_tokens)})
        }
        Execution {
            files,
            generated,
            checks,
            declared,
            exploration,
            generator,
            tool_turns,
            finalization_turns,
        } => {
            let handwritten = files
                .checked_sub(generated)
                .ok_or("generated outputs exceed declared files")?;
            let components = ExecutionComponents {
                exploration_operations: exploration.unwrap_or(0),
                file_operations: number(handwritten.checked_mul(9))?,
                check_operations: number(checks.checked_mul(4))?,
                scope_operations: 2,
                generation_operations: u64::from(generator),
                generated_inspection_operations: (generated != 0).then_some(generated),
            };
            let estimated = sum([
                components.exploration_operations,
                components.file_operations,
                components.check_operations,
                components.scope_operations,
                components.generation_operations,
                generated,
            ])?
            .max(declared);
            json!({"allowed_tool_turns":tool_turns,"finalization_turns":finalization_turns.min(tool_turns),
                "usable_tool_turns":tool_turns,"estimated_tool_turns":estimated,"components":components,
                "exploration_declared":exploration.is_some(),"fits":exploration.is_some() && estimated <= tool_turns})
        }
        Cutoff {
            input,
            output,
            reasoning,
            message_characters,
            prompt_characters,
            context,
        } => {
            match cutoff_sample(
                input,
                output,
                reasoning,
                Some(message_characters),
                Some(message_characters),
                prompt_characters,
                context,
            )? {
                CutoffObservation::Done { out_of_room } => json!(out_of_room),
                _ => unreachable!("both samples are supplied"),
            }
        }
        Request::CutoffSample {
            input,
            output,
            reasoning,
            visible_characters,
            fallback_characters,
            prompt_characters,
            context,
        } => serde_json::to_value(cutoff_sample(
            input,
            output,
            reasoning,
            visible_characters,
            fallback_characters,
            prompt_characters,
            context,
        )?)
        .map_err(|_| "invalid cutoff result")?,
        Account {
            usage,
            bound,
            weights,
            spent,
            turns,
            no_progress,
            r#final: final_,
            repairing,
        } => {
            let reported =
                !usage.is_empty() && usage.iter().all(|v| v["input_tokens"].as_u64().is_some());
            let weights = [price(weights[0])?, price(weights[1])?, price(weights[2])?];
            let last_input = if reported {
                sum(usage.iter().map(|v| v["input_tokens"].as_u64().unwrap()))?
            } else {
                bound
            };
            let cost = if reported {
                let costs = usage
                    .iter()
                    .map(|v| {
                        decision::charge(
                            v["input_tokens"].as_u64().unwrap(),
                            v["input_token_details"]["cache_read"].as_u64().unwrap_or(0),
                            v["input_token_details"]["cache_creation"]
                                .as_u64()
                                .unwrap_or(0),
                            weights,
                        )
                    })
                    .collect::<Option<Vec<_>>>();
                sum(costs.ok_or("budget charge overflow")?)?
            } else {
                number(decision::charge(bound, 0, 0, weights))?
            };
            let advance = u64::from(!final_ && !repairing);
            json!({"last_input":last_input,"spent":sum([spent,cost])?,
                "cache_active":reported && usage.iter().any(|v|truthy(&v["input_token_details"]["cache_read"])||truthy(&v["input_token_details"]["cache_creation"])),
                "turns":sum([turns,advance])?,"no_progress_turns":sum([no_progress,advance])?})
        }
        Charge {
            tokens,
            read,
            creation,
            weights,
        } => json!(number(decision::charge(
            tokens,
            read,
            creation,
            [price(weights[0])?, price(weights[1])?, price(weights[2])?]
        ))?),
        Predict {
            context,
            last_input,
            cache_active,
            compact_now,
            weights,
        } => {
            let cold = cold(weights, cache_active)?;
            let cached = if cache_active && !compact_now {
                context.min(last_input)
            } else {
                0
            };
            json!(number(
                Price::integer(cached)
                    .and_then(|n| n.times(price(weights[2]).ok()?))
                    .and_then(|read| read.plus(Price::integer(context - cached)?.times(cold)?))
                    .and_then(Price::ceil)
            )?)
        }
        Reserve {
            estimate,
            context,
            history_bytes,
            finish_characters,
            cache_active,
            weights,
        } => {
            let overhead = decision::estimate(sum([
                number(history_bytes.checked_mul(2))?,
                finish_characters,
                1024,
            ])?);
            let tokens = sum([estimate, overhead])?.max(context);
            json!(number(
                Price::integer(tokens)
                    .and_then(|n| n.times(cold(weights, cache_active).ok()?))
                    .and_then(Price::ceil)
            )?)
        }
        Context {
            estimate,
            last_input,
            last_estimate,
        } => json!(number(decision::context(
            estimate,
            last_input,
            last_estimate
        ))?),
        Estimate { bytes, messages } => json!(decision::estimate(sum([
            bytes,
            number(messages.checked_mul(32))?
        ])?)),
        Compaction { before, after } => json!(decision::compacted(before, after)),
        CompactNow {
            maker,
            compacted,
            context,
            context_size,
            has_ai,
        } => json!((maker || !compacted) && threshold(context, context_size) && has_ai),
        ContextFull {
            maker,
            compacted,
            compacted_below,
            compact_now,
            source_len,
            offset,
            context,
            context_size,
        } => json!(
            (if maker {
                compacted
            } else {
                compacted_below && !compact_now && source_len > offset
            }) && threshold(context, context_size)
        ),
        Reply { done, unfinished } => json!(decision::reject_done(done, unfinished)),
        History { messages, start } => {
            if start >= messages.len() {
                return Err("invalid visit checkpoint");
            }
            let mut pending = vec![];
            for index in (start + 1..messages.len()).rev() {
                if let Message::Ai { calls } = &messages[index] {
                    let answered: BTreeSet<_> = messages[index + 1..]
                        .iter()
                        .filter_map(|m| {
                            if let Message::Tool { id } = m {
                                Some(id)
                            } else {
                                None
                            }
                        })
                        .collect();
                    pending = calls
                        .iter()
                        .filter(|id| !answered.contains(id))
                        .cloned()
                        .collect();
                    break;
                }
            }
            let mut end = messages.len();
            let mut interrupted = false;
            while end > start + 1 {
                let mut first = end;
                while first > start + 1 && matches!(messages[first - 1], Message::Tool { .. }) {
                    first -= 1;
                }
                if let Message::Ai { calls } = &messages[first - 1] {
                    let mut results: Vec<_> = messages[first..end]
                        .iter()
                        .filter_map(|m| {
                            if let Message::Tool { id } = m {
                                Some(id)
                            } else {
                                None
                            }
                        })
                        .collect();
                    let mut expected: Vec<_> = calls.iter().collect();
                    results.sort();
                    expected.sort();
                    if first < end && expected == results {
                        break;
                    }
                    interrupted |= !calls.is_empty();
                }
                end = if first < end { first - 1 } else { end - 1 };
            }
            json!({"pending":pending,"end":end,"interrupted":interrupted})
        }
        Remaining {
            maker,
            turns,
            spent,
            tool_turns,
            input_tokens,
            finalization_turns,
            no_progress_window,
            no_progress_turns,
            last_progress_kind,
            implementation_files,
            implementation_started,
            phase,
            exhaustion,
            finalizing,
            r#final: _final,
        } => {
            let mut result = json!({"tool_turns":{"used":turns.min(tool_turns),"allowed":tool_turns,"remaining":decision::remaining(turns,tool_turns)},
                "input_tokens":{"used":spent,"allowed":input_tokens,"remaining":decision::remaining(spent,input_tokens)},
                "finalization_turns":finalization_turns.min(tool_turns),"phase":phase.unwrap_or_else(||if exhaustion.is_some(){"final"}else if finalizing{"finalizing"}else{"working"}.into())});
            if maker {
                let deadline = tool_turns.div_ceil(2);
                result["no_progress_window"] = json!(no_progress_window);
                result["no_progress_turns"] = json!(no_progress_turns);
                result["last_progress_kind"] = json!(last_progress_kind);
                result["implementation_started"] = json!(implementation_started);
                result["implementation_due_at_tool_turn"] = json!(deadline);
                if implementation_files && !implementation_started && turns >= deadline {
                    result["warning"] = json!(if finalizing || _final || exhaustion.is_some() {
                        "Implementation remains unfinished. The finalization reserve forbids edits; verify preserved work and report the unfinished implementation."
                    } else {
                        "Stop broad exploration now. Half the tool-turn budget is spent without a completed non-test edit. Use the remaining working turns to implement the declared plan, preserving test-first proof, metadata and the finalization reserve. If you cannot implement, report unfinished work precisely."
                    });
                }
            }
            result
        }
        Finalizing {
            maker,
            turns,
            no_progress,
            allowed,
            window,
            reserve,
        } => json!(decision::finalizing(
            maker,
            turns,
            no_progress,
            allowed,
            window,
            reserve
        )),
        Exhaustion {
            maker,
            turns,
            allowed,
            no_progress,
            window,
            context_full,
            spent,
            predicted,
            reserve,
            input_limit,
        } => json!(
            decision::exhaustion(decision::Run {
                maker,
                turns,
                allowed,
                stalled: maker && window != 0 && no_progress >= window,
                context_full,
                spent,
                predicted,
                reserve,
                input_limit
            })
            .map(|e| match e {
                decision::Exhaustion::Backstop => "backstop",
                decision::Exhaustion::ToolTurns => "tool_turns",
                decision::Exhaustion::NoProgress => "no_progress",
                decision::Exhaustion::ContextTokens => "context_tokens",
                decision::Exhaustion::InputTokens => "input_tokens",
            })
        ),
        Dropped {
            spent,
            in_flight,
            reserve,
            allowed,
        } => {
            let spent = sum([spent, in_flight])?;
            json!({"spent":spent,"fits":u128::from(spent)+u128::from(in_flight)+u128::from(reserve)<=u128::from(allowed)})
        }
        Progress {
            maker,
            window,
            name,
            changed,
            fresh,
            evidence_new,
        } => json!(
            maker
                && window != 0
                && evidence_new.unwrap_or(changed && matches!(name.as_str(), "write" | "edit"))
                && (fresh || name == "run_tests")
        ),
        ToolAllowed {
            maker,
            finalizing,
            r#final: final_,
            repairing,
            name,
        } => json!(if final_ || repairing {
            "unavailable"
        } else if maker
            && finalizing
            && !matches!(
                name.as_str(),
                "list" | "read" | "glob" | "grep" | "diff" | "run_tests"
            )
        {
            "reserve"
        } else {
            "allowed"
        }),
        Restore { mut state, maker } => match validate_restore(&state) {
            Err(message) => serde_json::to_value(RestoreVerdict::Invalid {
                message: message.into(),
            })
            .unwrap(),
            Ok(()) => {
                if maker && state["exhaustion"] == "tool_turns" {
                    state["exhaustion"] = json!("backstop");
                }
                serde_json::to_value(RestoreVerdict::Valid { state }).unwrap()
            }
        },
        Retained { messages, limit } => {
            serde_json::to_value(compaction::retain(&messages, limit)?).unwrap()
        }
    })
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(v) => *v,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

fn validate_restore(state: &Value) -> Result<()> {
    if !state.is_object() {
        return Err("invalid allowance state");
    }
    for name in ["spent", "turns", "last_input", "compaction_offset"] {
        if state[name].as_u64().is_none() {
            return Err("invalid allowance state");
        }
    }
    for name in ["cache_active", "compacted", "compacted_below"] {
        if !state[name].is_boolean() {
            return Err("invalid allowance state");
        }
    }
    if !state["last_estimate"].is_null() && state["last_estimate"].as_i64().is_none() {
        return Err("invalid allowance state");
    }
    let count = state
        .get("compaction_count")
        .cloned()
        .unwrap_or(json!(u64::from(state["compacted"] == true)));
    if count
        .as_u64()
        .is_none_or(|n| (n != 0) != (state["compacted"] == true))
    {
        return Err("invalid compaction checkpoint");
    }
    let no_progress = state.get("no_progress_turns").cloned().unwrap_or(json!(0));
    if no_progress
        .as_u64()
        .is_none_or(|n| n > state["turns"].as_u64().unwrap())
    {
        return Err("invalid maker progress checkpoint");
    }
    if (!state["last_progress_kind"].is_null() && !state["last_progress_kind"].is_string())
        || !matches!(
            state["last_progress_kind"].as_str(),
            None | Some("read" | "grep" | "glob" | "write" | "edit" | "run_tests")
        )
    {
        return Err("invalid maker progress checkpoint");
    }
    for name in ["seen_calls", "seen_content", "seen_tests"] {
        if let Some(values) = state.get(name)
            && values.as_array().is_none_or(|values| {
                values.iter().any(|v| {
                    v.as_str().is_none_or(|s| {
                        s.len() != 64
                            || !s
                                .bytes()
                                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    })
                })
            })
        {
            return Err("invalid maker progress checkpoint");
        }
    }
    Ok(())
}

/// The transport fails closed before any worker tools can be dispatched.
pub fn check() -> crate::Result<()> {
    use std::io::{Read, Write};
    if std::env::args().nth(2).as_deref() == Some("schema") {
        serde_json::to_writer_pretty(std::io::stdout().lock(), &wire_schema())?;
        return Ok(());
    }
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(32 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 32 * 1024 * 1024 {
        return Err(crate::Error::Invalid("budget request is too large"));
    }
    let request = crate::adapters::workers::json::parse(&bytes)
        .map_err(|_| crate::Error::Invalid("invalid check-budget JSON"))?;
    let request: Request = serde_json::from_value(request)
        .map_err(|_| crate::Error::Invalid("invalid check-budget facts"))?;
    let execution = matches!(&request, Request::Execution { .. });
    let result = decide(request).map_err(crate::Error::Invalid)?;
    // Value's map is sorted. Preserve the original component presentation order
    // when crossing the worker boundary, where planner diagnostics use .items().
    #[derive(Serialize)]
    struct Response<T> {
        result: T,
    }
    if execution {
        let result: ExecutionEvidence = serde_json::from_value(result)?;
        serde_json::to_writer(std::io::stdout().lock(), &Response { result })?;
    } else {
        serde_json::to_writer(std::io::stdout().lock(), &Response { result })?;
    }
    std::io::stdout().write_all(b"\n")?;
    Ok(())
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ForecastEvidence {
    pub tokenizer: String,
    pub brief_tokens: u64,
    pub file_tokens: BTreeMap<String, u64>,
    pub overhead_tokens: u64,
    pub tokens: u64,
    pub fit_tokens: u64,
    pub context_tokens: u64,
    pub fits: bool,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecutionEvidence {
    pub allowed_tool_turns: u64,
    pub finalization_turns: u64,
    pub usable_tool_turns: u64,
    pub estimated_tool_turns: u64,
    pub components: ExecutionComponents,
    pub exploration_declared: bool,
    pub fits: bool,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Counter {
    pub used: u64,
    pub allowed: u64,
    pub remaining: u64,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Remaining {
    pub tool_turns: Counter,
    pub input_tokens: Counter,
    pub finalization_turns: u64,
    pub phase: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_progress_window: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_progress_turns: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_progress_kind: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub implementation_started: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub implementation_due_at_tool_turn: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct History {
    pub pending: Vec<String>,
    pub end: usize,
    pub interrupted: bool,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Account {
    pub last_input: u64,
    pub spent: u64,
    pub cache_active: bool,
    pub turns: u64,
    pub no_progress_turns: u64,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Dropped {
    pub spent: u64,
    pub fits: bool,
}
/// #780: declaration order is model-facing through planner diagnostics.
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecutionComponents {
    pub exploration_operations: u64,
    pub file_operations: u64,
    pub check_operations: u64,
    pub scope_operations: u64,
    pub generation_operations: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generated_inspection_operations: Option<u64>,
}

/// #780 recovery: a validator verdict is distinct from transport failure.
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RestoreVerdict {
    Valid { state: Value },
    Invalid { message: String },
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolAction {
    Unavailable,
    Reserve,
    Allowed,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Dimension {
    Backstop,
    ToolTurns,
    NoProgress,
    ContextTokens,
    InputTokens,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CutoffObservation {
    NeedVisible,
    NeedFallback,
    Done { out_of_room: bool },
}

/// #780 extraction equivalence: request each observation at its original branch.
fn cutoff_sample(
    input: Option<u64>,
    output: Option<u64>,
    reasoning: Option<u64>,
    visible_characters: Option<u64>,
    fallback_characters: Option<u64>,
    prompt_characters: u64,
    context: u64,
) -> Result<CutoffObservation> {
    let tokens = match (input, output) {
        (Some(input), Some(output)) if input != 0 || output != 0 => {
            let visible = match reasoning {
                Some(reasoning) => output.saturating_sub(reasoning),
                None => match visible_characters {
                    Some(characters) => output.min(decision::estimate(characters)),
                    None => return Ok(CutoffObservation::NeedVisible),
                },
            };
            sum([input, visible])?
        }
        (None, None) | (Some(0), Some(0)) => 0,
        _ => return Err("invalid model usage metadata"),
    };
    let tokens = if tokens == 0 {
        match fallback_characters {
            Some(characters) => decision::estimate(sum([prompt_characters, characters])?),
            None => return Ok(CutoffObservation::NeedFallback),
        }
    } else {
        tokens
    };
    Ok(CutoffObservation::Done {
        out_of_room: decision::cutoff(tokens, context),
    })
}

/// Contracts are derived from Rust, consumed by the worker, and drift-checked.
pub fn wire_schema() -> Value {
    let mut responses = BTreeMap::new();
    macro_rules! schema {($ty:ty; $($name:literal),+) => {$(responses.insert($name,schemars::schema_for!($ty));)+};}
    schema!(bool; "cutoff","compaction","compact_now","context_full","reply","finalizing","progress");
    schema!(u64; "charge","predict","reserve","context","estimate");
    schema!(CutoffObservation; "cutoff_sample");
    schema!(ForecastEvidence; "forecast");
    schema!(ExecutionEvidence; "execution");
    schema!(Remaining; "remaining");
    schema!(History; "history");
    schema!(Account; "account");
    schema!(Dropped; "dropped");
    schema!(compaction::Retained; "retained");
    schema!(ToolAction; "tool_allowed");
    schema!(Option<Dimension>; "exhaustion");
    schema!(RestoreVerdict; "restore");
    json!({"$schema":"https://json-schema.org/draft/2020-12/schema",
           "request":schemars::schema_for!(Request),"responses":responses})
}
