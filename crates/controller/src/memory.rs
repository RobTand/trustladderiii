//! Controller-owned memory delivery. It never enters the kernel's effect machine.
use crate::{Error, Result};
use regex::Regex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{BufRead, BufReader, Read},
    path::Path,
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, AtomicI64, Ordering},
        mpsc::{self, Sender},
    },
    time::Duration,
};
use tl3_store::Store;

const SECTION_BYTES: usize = 6000;
const RECORD_BYTES: usize = 32768;

fn clip(text: &str, limit: usize) -> &str {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

pub(crate) fn sensitive_key(key: &str) -> bool {
    let key = key.replace('\0', "").to_ascii_lowercase().replace('-', "_");
    matches!(
        key.as_str(),
        "key"
            | "key_file"
            | "api_key"
            | "apikey"
            | "password"
            | "passwd"
            | "secret"
            | "token"
            | "access_token"
            | "refresh_token"
            | "authorization"
            | "cookie"
            | "private_key"
            | "credentials"
            | "client_secret"
    ) || key.ends_with("password")
        || key.ends_with("_key")
        || key.ends_with("_secret")
        || key.ends_with("_token")
}

pub(crate) fn redact_text(text: &str) -> String {
    // Like #302's Redactor, accept quotes and whitespace escaped by nested JSON.
    static SECRET: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
        r#"(?is)(?:bearer\s+\S+|-----BEGIN[^\r\n]*PRIVATE KEY-----.*?(?:-----END[^\r\n]*PRIVATE KEY-----|$)|\b(?:sk-|gh[pousr]_|github_pat_|hf_|glpat-|xox[a-z]-)[a-zA-Z0-9_-]+|AKIA[A-Z0-9]{16}|\b[a-fA-F0-9]{32,}\b|eyJ[a-zA-Z0-9_-]+\.[a-zA-Z0-9_-]+\.[a-zA-Z0-9_-]+|(?:https?|postgres(?:ql)?)://[^/\s:@]+:[^/\s@]+@|(?:[a-z0-9_]*_api_key|[a-z0-9_]*_secret[a-z0-9_]*|[a-z0-9_]*_token|api[_-]?key|password|passwd|secret|authorization|cookie)\\*["']?(?:\s|\\+[nrt])*[=:](?:\s|\\+[nrt])*(?:(?:bearer|basic)\s+)?(?:\\+"[^"\r\n]*"|"[^"]*"|\\*'[^']*'|\S+)|\btoken\\*["']?(?:\s|\\+[nrt])*=(?:\s|\\+[nrt])*(?:(?:bearer|basic)\s+)?(?:\\+"[^"\r\n]*"|"[^"]*"|\\*'[^']*'|\S+)|\btoken\\*["']?(?:\s|\\+[nrt])*:(?:\s|\\+[nrt])*(?:\\*["'][^"']+["']|[a-zA-Z0-9_-]*[0-9_-][a-zA-Z0-9_-]*))"#
    ).expect("memory secret pattern")
    });
    let text = text.replace('\0', "");
    SECRET.replace_all(&text, "[redacted]").into_owned()
}

fn safe_text(text: &str) -> String {
    let text = redact_text(text);
    if text.len() > 2048 {
        format!("{} [truncated]", clip(&text, 2048))
    } else {
        text
    }
}

fn sanitize(value: &Value, depth: usize, budget: &mut usize) -> Value {
    if *budget == 0 || depth == 0 {
        return json!("[truncated]");
    }
    *budget -= 1;
    match value {
        Value::String(text) => json!(safe_text(text)),
        Value::Array(items) => {
            let mut values: Vec<_> = items
                .iter()
                .take(32)
                .map(|v| sanitize(v, depth - 1, budget))
                .collect();
            if items.len() > 32 {
                values.push(json!("[truncated]"));
            }
            Value::Array(values)
        }
        Value::Object(items) => {
            let mut values = serde_json::Map::new();
            for (key, value) in items.iter().take(32) {
                if sensitive_key(key) {
                    continue;
                }
                values.insert(safe_text(key), sanitize(value, depth - 1, budget));
            }
            if items.len() > 32 {
                values.insert("truncated".into(), json!(true));
            }
            Value::Object(values)
        }
        value => value.clone(),
    }
}

fn section(value: &Value) -> Value {
    bound_section(sanitize(value, 8, &mut 256))
}

fn bound_section(value: Value) -> Value {
    let encoded = value.to_string();
    if encoded.len() <= SECTION_BYTES {
        value
    } else {
        json!({"summary": clip(&encoded, SECTION_BYTES / 2), "truncated": true})
    }
}

fn activity_section(activity: &Value) -> Value {
    let mut value = sanitize(activity, 8, &mut 256);
    if let (Some(source), Some(tests)) = (
        activity["tests"].as_array(),
        value.get_mut("tests").and_then(Value::as_array_mut),
    ) {
        for (source, test) in source.iter().zip(tests) {
            if let (Some(action), Some(test)) =
                (source["action_key"].as_str(), test.as_object_mut())
            {
                // Structured PrismaBuild action IDs are public identifiers, unlike input keys.
                let action = if action.len() == 64 && action.bytes().all(|b| b.is_ascii_hexdigit())
                {
                    action.to_owned()
                } else {
                    safe_text(action)
                };
                test.insert("action_key".into(), json!(action));
            }
        }
    }
    bound_section(value)
}

fn read(path: &Path, limit: u64) -> Result<Option<Value>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(Error::Invalid("oversized memory evidence"));
    }
    Ok(Some(serde_json::from_slice(&bytes)?))
}

/// Queue before worker evidence is removed. Replay uses the same remote identity,
/// even if an earlier outbox row was already sent and deleted.
pub fn queue_attempt(store: &mut Store, key: &str, attempt: &Path) -> Result<()> {
    let Some(request) = read(&attempt.join("request.json"), 16 * 1024 * 1024)? else {
        return Ok(());
    };
    let Some(context) = request.get("memory") else {
        return Ok(());
    };
    let summary = read(&attempt.join("memory.json"), 32768)
        .ok()
        .flatten()
        .unwrap_or(Value::Null);
    let exit = read(&attempt.join("exit.json"), 1024)?.unwrap_or(Value::Null);
    let mut reply = summary["reply"].clone();
    if reply.is_null()
        && let Ok(file) = File::open(attempt.join("stdout.jsonl"))
    {
        // Older workers have no sidecar. Keep only their final structured reply.
        let mut reader = BufReader::new(file.take(16 * 1024 * 1024));
        loop {
            let mut line = Vec::new();
            if reader
                .by_ref()
                .take(1024 * 1024)
                .read_until(b'\n', &mut line)?
                == 0
            {
                break;
            }
            if let Ok(value) = serde_json::from_slice::<Value>(&line)
                && value["event"] == "reply"
            {
                reply = value["reply"].clone();
            }
        }
    }
    let succeeded = exit["code"] == 0 && summary["outcome"] != "failed" && !reply.is_null();
    let reason = if succeeded {
        Value::Null
    } else if summary["outcome"] == "failed" {
        summary["reason"].clone()
    } else {
        json!(if exit.is_null() {
            "worker ended without an exit record"
        } else {
            "worker exit or reply failed"
        })
    };
    let repo = safe_text(clip(context["repo"].as_str().unwrap_or("unknown"), 256));
    let request_id = safe_text(clip(context["request"].as_str().unwrap_or("unknown"), 128));
    let role = safe_text(clip(request["role"].as_str().unwrap_or("unknown"), 128));
    let identity = json!([
        repo,
        request_id,
        context["epoch"],
        key,
        attempt.file_name().and_then(|n| n.to_str()),
        request["operation"]
    ]);
    let id = uuid(&identity);
    if succeeded && matches!(role.as_str(), "ingest" | "planner") {
        let worktree = attempt
            .parent()
            .ok_or(Error::Invalid("worker attempt has no run directory"))?
            .join("worktree");
        let run = Run {
            role: &role,
            repo: &repo,
            request: &request_id,
            id: &id,
        };
        review_memories(store, &run, &request["inputs"], &reply, &worktree)?;
    }
    // Infrastructure inputs, recalled memories and repository contents belong in
    // the trace, not memory.
    let mut inputs = request["inputs"].clone();
    if let Some(inputs) = inputs.as_object_mut() {
        for field in ["tests", "hindsight", "forecast", "memories", "memory_note"] {
            inputs.remove(field);
        }
        if let Some(repo) = inputs.get_mut("repo").and_then(Value::as_object_mut) {
            repo.remove("materials");
        }
    }
    let mut record = json!({
        "role": role, "model": section(&request["model"]["name"]), "repo": repo,
        "issue": context["issue"], "request": request_id, "run": id,
        "inputs": section(&inputs), "activity": activity_section(&summary["activity"]),
        "reply": section(&reply), "outcome": if succeeded { "succeeded" } else { "failed" },
        "reason": section(&reason),
    });
    if request["role"] == "maker"
        && let Some(head) = reply["head"].as_str()
        && head.len() == 40
        && head.bytes().all(|b| b.is_ascii_hexdigit())
    {
        record["head_sha"] = json!(head);
    }
    let mut item = json!({"content": record.to_string(), "document_id": format!("tl3-run-{id}"),
        "context": "TL3 Deep Agents run summary", "tags": [format!("repo:{repo}"), format!("request:{request_id}"), format!("role:{role}"), "kind:run", "source:run"],
        "metadata": {"source": "tl3", "run": id}});
    if stored_json_bytes(&item) > RECORD_BYTES {
        for field in ["inputs", "activity", "reply", "reason", "model"] {
            record[field] =
                json!({"summary": clip(&record[field].to_string(), 512), "truncated": true});
        }
        item["content"] = json!(record.to_string());
    }
    // Postgres is authoritative for JSONB's numeric and escaping representation.
    if store.memory_item_bytes(&item)? > RECORD_BYTES as i32 {
        return Err(Error::Invalid("oversized memory record"));
    }
    store.queue_memory(&id, &item)?;
    Ok(())
}

/// Hindsight owns the durable async operation after acknowledgement. Retrying
/// after any ambiguous response reuses its UUID and cannot enqueue new work.
pub fn flush(store: &mut Store, settings: &tl3_config::Hindsight) -> Result<usize> {
    store.expire_parked_memories()?;
    // A wrong memory misleads every run until Hindsight retires it, so curations go first.
    let mut count = curate(store, settings)?;
    let mut pending = store.pending_memories(0)?;
    if pending.is_empty() {
        if count > 0 {
            store.memory_delivery_ack()?;
        }
        return Ok(count);
    }
    let key = settings.retain_key()?;
    let client = client(Duration::from_secs(10))?;
    let url = bank_url(settings, &[])?;
    let mut after = 0;
    loop {
        for (sequence, id, item) in pending {
            // Q7: a preceding curation cannot authorize this operation's retry.
            if memory_needs_rob(&store.load()?, &format!("memory:retain:{id}"))? {
                return Err(Error::Invalid(
                    "memory delivery waits for Rob to clear its incident",
                ));
            }
            after = sequence;
            let result: Result<()> = crate::executor::runtime()?.block_on(async {
                let mut request = client
                    .post(url.clone())
                    .json(&json!({"async": true, "operation_id": id, "items": [item]}));
                if let Some(key) = &key {
                    request = request.bearer_auth(key);
                }
                let response = request
                    .send()
                    .await
                    .map_err(|e| Error::Adapter(Box::new(e)))?;
                if !response.status().is_success() {
                    return Err(Error::HindsightStatus(response.status().as_u16()));
                }
                let body: Value = response
                    .json()
                    .await
                    .map_err(|e| Error::Adapter(Box::new(e)))?;
                if body["success"] != true
                    || body["async"] != true
                    || body["operation_id"] != id
                    || body["bank_id"] != settings.bank
                    || body["items_count"] != 1
                {
                    return Err(Error::Invalid("invalid Hindsight retain acknowledgement"));
                }
                Ok(())
            });
            match result {
                Ok(()) => {
                    store.forget_memory(&id)?;
                    count += 1;
                }
                Err(Error::HindsightStatus(status @ (401 | 403))) => {
                    store.reject_memory_key(status)?;
                    return Err(Error::HindsightStatus(status));
                }
                Err(error @ Error::HindsightStatus(408 | 429)) => return Err(error),
                Err(Error::HindsightStatus(status @ 400..=499)) => {
                    store.park_memory(&id, status)?;
                }
                Err(error) => return Err(error),
            }
        }
        pending = store.pending_memories(after)?;
        if pending.is_empty() {
            if count == 0 || store.replay_capacity_memories()? == 0 {
                break;
            }
            after = 0;
            pending = store.pending_memories(after)?;
        }
    }
    if count > 0 {
        store.memory_delivery_ack()?;
    }
    Ok(count)
}

/// At most this many memories go into one run's inputs.
pub const MAX_MEMORIES: usize = 8;
/// Characters kept from one memory.
pub const MEMORY_CHARS: usize = 1500;
/// Characters kept across one run's memories.
pub const MEMORIES_CHARS: usize = 8000;
const RECALL_NOTE: &str =
    "Hindsight recall was unavailable, so no memories were supplied. Work from the other inputs.";
const RECALL_PAUSED: &str = "Hindsight recall is paused after a failed recall, so no memories were supplied. Work from the other inputs.";
const RECALL_RESPONSE_BYTES: usize = 256 * 1024;
// Hindsight 0.10.2 rejects queries over 500 tokens. A 400-byte UTF-8 prefix
// leaves margin even for text that tokenizes into one token per byte.
const RECALL_QUERY_BYTES: usize = 400;
const CURATED_KINDS: [&str; 3] = ["kind:lesson", "kind:decision", "kind:convention"];
/// Flags and conventions read per run; more are ignored.
const PER_RUN: usize = 3;
const EVIDENCE_LINES: u64 = 40;
const EVIDENCE_FILE_BYTES: u64 = 1024 * 1024;

/// What one worker run recalls before it starts.
pub enum Recall<'a> {
    /// Ingest and planning: lessons, decisions and conventions first, then anything relevant.
    Context { repo: &'a str, query: &'a str },
    /// Review: the repo's conventions only.
    Conventions { repo: &'a str, query: &'a str },
}

fn clip_chars(text: &str, limit: usize) -> &str {
    match text.char_indices().nth(limit) {
        Some((end, _)) => &text[..end],
        None => text,
    }
}

fn uuid(identity: &Value) -> String {
    let mut hash = Sha256::digest(identity.to_string().as_bytes());
    hash[6] = (hash[6] & 0x0f) | 0x50;
    hash[8] = (hash[8] & 0x3f) | 0x80;
    let h: String = hash[..16].iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}

fn client(timeout: Duration) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(Duration::from_secs(2))
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| Error::Adapter(Box::new(e)))
}

fn bank_url(settings: &tl3_config::Hindsight, tail: &[&str]) -> Result<reqwest::Url> {
    let mut url = settings.endpoint.clone();
    url.path_segments_mut()
        .map_err(|_| Error::Invalid("invalid Hindsight endpoint"))?
        .clear()
        .extend(["v1", "default", "banks", &settings.bank, "memories"])
        .extend(tail);
    Ok(url)
}

fn recall_once(settings: &tl3_config::Hindsight, body: &Value) -> Result<Vec<Value>> {
    let key = settings.retain_key()?;
    // Live recalls take about 0.6 s per call (#439 trial); a hang costs one launch this much.
    let client = client(Duration::from_secs(5))?;
    let url = bank_url(settings, &["recall"])?;
    crate::executor::runtime()?.block_on(async {
        let mut request = client.post(url).json(body);
        if let Some(key) = &key {
            request = request.bearer_auth(key);
        }
        let mut response = request
            .send()
            .await
            .map_err(|e| Error::Adapter(Box::new(e)))?;
        if !response.status().is_success() {
            return Err(Error::HindsightStatus(response.status().as_u16()));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| Error::Adapter(Box::new(e)))?
        {
            bytes.extend_from_slice(&chunk);
            if bytes.len() > RECALL_RESPONSE_BYTES {
                return Err(Error::Invalid("oversized Hindsight recall"));
            }
        }
        let value: Value = serde_json::from_slice(&bytes)?;
        value["results"]
            .as_array()
            .cloned()
            .ok_or(Error::Invalid("invalid Hindsight recall"))
    })
}

fn memory_kind(tags: &Value) -> &'static str {
    let tags: Vec<&str> = tags
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    for kind in ["lesson", "decision", "convention", "run"] {
        if tags
            .iter()
            .any(|tag| tag.strip_prefix("kind:") == Some(kind))
        {
            return kind;
        }
    }
    // Run records retained before #422 carry a role tag but no kind.
    if tags.iter().any(|tag| tag.starts_with("role:")) {
        "run"
    } else {
        "other"
    }
}

/// The memory's single `repo:` tag. A run can retire only memories of its own repo.
fn memory_repo(tags: &Value) -> Option<String> {
    let mut repos = tags
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|tag| tag.as_str()?.strip_prefix("repo:"));
    match (repos.next(), repos.next()) {
        (Some(repo), None) if !repo.is_empty() && repo.len() <= 256 => Some(safe_text(repo)),
        _ => None,
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 256
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_:.".contains(&b))
}

fn memory_id(value: &Value) -> Option<&str> {
    value.as_str().filter(|id| valid_id(id))
}

/// Recall bounded, redacted memories as worker inputs. Hindsight failures degrade
/// to no memories and a note, never an incident or a wait; only store errors are returned.
pub fn recall(
    store: &mut Store,
    settings: &tl3_config::Hindsight,
    breaker: &Breaker,
    recall: Recall<'_>,
) -> Result<Value> {
    if breaker.is_open() {
        return Ok(json!({"memories": [], "memory_note": RECALL_PAUSED}));
    }
    let (repo, query) = match recall {
        Recall::Context { repo, query } | Recall::Conventions { repo, query } => (repo, query),
    };
    let base = json!({
        "query": clip(&safe_text(&format!("Repository {repo}\n{query}")), RECALL_QUERY_BYTES),
        "types": ["world", "experience"], "budget": "low", "max_tokens": 2048,
    });
    let mut calls = Vec::new();
    match recall {
        Recall::Context { .. } => {
            let mut lessons = base.clone();
            lessons["tags"] = json!(CURATED_KINDS);
            lessons["tags_match"] = json!("any_strict");
            calls.extend([lessons, base]);
        }
        Recall::Conventions { .. } => {
            let mut conventions = base;
            conventions["tag_groups"] = json!([
                {"tags": ["kind:convention"], "match": "any_strict"},
                {"tags": [format!("repo:{repo}")], "match": "any_strict"},
            ]);
            calls.push(conventions);
        }
    }
    let mut found = Vec::new();
    let mut unavailable = false;
    for body in &calls {
        match recall_once(settings, body) {
            Ok(results) => found.extend(results),
            Err(error) => {
                // One failure is enough: a second call would only double the wait.
                eprintln!("Hindsight recall degraded to no memories: {error}");
                // A refused request says nothing about Hindsight's health.
                if !matches!(error, Error::HindsightStatus(400)) {
                    breaker.open();
                }
                unavailable = true;
                break;
            }
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    let found: Vec<(&str, &Value)> = found
        .iter()
        .filter_map(|item| Some((memory_id(&item["id"])?, item)))
        .filter(|(id, item)| item["text"].is_string() && seen.insert(*id))
        .collect();
    let ids: Vec<String> = found.iter().map(|(id, _)| (*id).to_owned()).collect();
    let withheld = store.withheld_memories(&ids)?;
    let mut memories = Vec::new();
    let mut total = 0;
    for (id, item) in found {
        if withheld.contains(id) {
            continue;
        }
        let content =
            clip_chars(&safe_text(item["text"].as_str().unwrap()), MEMORY_CHARS).to_owned();
        total += content.chars().count();
        if memories.len() == MAX_MEMORIES || total > MEMORIES_CHARS {
            break;
        }
        let mut memory = json!({"content": content, "id": id, "kind": memory_kind(&item["tags"])});
        if let Some(repo) = memory_repo(&item["tags"]) {
            memory["repo"] = json!(repo);
        }
        memories.push(memory);
    }
    let mut value = json!({"memories": memories});
    if unavailable {
        value["memory_note"] = json!(RECALL_NOTE);
    }
    Ok(value)
}

/// Recall stays off after an outage, so a hung Hindsight costs one launch
/// rather than every launch. It closes again on evidence, never on a clock: the
/// next delivery Hindsight acknowledges.
#[derive(Clone, Default)]
pub struct Breaker(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl Breaker {
    fn is_open(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn open(&self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    fn close(&self) {
        self.0.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Flush, then let recall try again if Hindsight acknowledged anything.
pub fn deliver(
    store: &mut Store,
    settings: &tl3_config::Hindsight,
    breaker: &Breaker,
) -> Result<usize> {
    let count = flush(store, settings)?;
    if count > 0 {
        breaker.close();
    }
    Ok(count)
}

/// Posts one line about a curation on the TL3 operations issue.
pub trait Announce: Send + Sync {
    /// `key` is stable per curation, so a retried post can't duplicate.
    fn post(&self, key: &str, body: &str) -> Result<()>;
}

/// The configured operations issue, through the GitHub App.
pub struct OperationsIssue {
    pub client: tl3_github::GitHub,
    pub repo: String,
    pub issue: u64,
}

impl Announce for OperationsIssue {
    fn post(&self, key: &str, body: &str) -> Result<()> {
        crate::executor::runtime()?
            .block_on(self.client.post_comment(&self.repo, self.issue, key, body))?;
        Ok(())
    }
}

/// Untrusted text inside a one-line code span: no backticks, line breaks or mentions.
fn inline(text: &str, limit: usize) -> String {
    let text: String = clip_chars(text, limit)
        .chars()
        .map(|c| match c {
            '`' => '\'',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    format!("`{}`", text.replace('@', "(at)"))
}

/// Post every hide and retirement not yet announced, oldest first.
pub fn announce(store: &mut Store, announcer: &dyn Announce) -> Result<usize> {
    let mut count = 0;
    loop {
        let pending = store.unannounced_memory_curations()?;
        if pending.is_empty() {
            return Ok(count);
        }
        for (sequence, operation, row) in pending {
            let done = if row["state"] == "hidden" {
                "hidden from TL3's recall (one signal; a second request's flag or `tl3 memory confirm` retires it)"
            } else {
                "retired in Hindsight"
            };
            let evidence = &row["evidence"];
            let mut body = format!(
                "Memory {} {done}. Claim: {}.",
                inline(row["memory_id"].as_str().unwrap_or(""), 256),
                inline(row["claim"].as_str().unwrap_or("not quoted"), 300)
            );
            if evidence["path"].is_string() {
                body += &format!(" Evidence: {}.", inline(&cite(evidence), 400));
            }
            body += &format!(
                " Source: {}, run {}, request {}. Undo: `tl3 memory revert`.",
                inline(row["source"].as_str().unwrap_or(""), 64),
                inline(row["run"].as_str().unwrap_or("none"), 64),
                inline(row["request"].as_str().unwrap_or("none"), 128)
            );
            announcer.post(&format!("memory-curation:{operation}"), &body)?;
            store.announce_memory_curation(sequence)?;
            count += 1;
        }
    }
}

/// A decision memory: the question, Rob's choice and his words for that answer.
/// The classifier's guidance or new text is the answer's own words; otherwise
/// the comment's first paragraph is.
pub fn decision(asked: &str, reply: &Value, comment: &str) -> String {
    let words = ["guidance", "new_text"]
        .iter()
        .find_map(|field| reply[field].as_str())
        .unwrap_or_else(|| comment.trim().split("\n\n").next().unwrap_or(""));
    format!(
        "Question: {}\nRob chose: {}\nHis words: {}",
        clip_chars(asked.trim(), 500),
        reply["answer"].as_str().unwrap_or("unknown"),
        clip_chars(words.trim(), 300)
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RobCuration {
    /// Retire a memory in Hindsight on Rob's word.
    Invalidate,
    /// Second signal for a memory a run hid: retire it in Hindsight.
    Confirm,
    /// Make a hidden or retired memory valid again.
    Revert,
}

/// Rob's own decision about one memory. Each call is its own decision.
pub fn rob_curation(
    store: &mut Store,
    memory_id: &str,
    action: RobCuration,
    reason: &str,
) -> Result<()> {
    if !valid_id(memory_id) {
        return Err(Error::Invalid("invalid memory id"));
    }
    let reason = safe_text(clip_chars(reason.trim(), 1000));
    if reason.is_empty() {
        return Err(Error::Invalid("a curation needs a reason"));
    }
    let signals = store.memory_signals(memory_id)?;
    let state = match action {
        RobCuration::Invalidate => "invalidated",
        RobCuration::Revert => "valid",
        RobCuration::Confirm if signals.last().is_some_and(|(state, ..)| state == "hidden") => {
            "invalidated"
        }
        RobCuration::Confirm => {
            return Err(Error::Invalid(
                "this memory is not hidden; nothing to confirm",
            ));
        }
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let operation = uuid(&json!(["rob", memory_id, state, reason, now.to_string()]));
    // A confirmation carries the hiding flag's claim and evidence onto the trail.
    let hidden = if action == RobCuration::Confirm {
        store
            .memory_curations()?
            .into_iter()
            .rev()
            .find(|row| row["memory_id"] == memory_id && row["state"] == "hidden")
    } else {
        None
    };
    store.record_memory_curation(
        &operation,
        &tl3_store::MemoryCuration {
            memory_id: memory_id.into(),
            state,
            verdict: "accepted",
            source: "rob".into(),
            request: None,
            run: None,
            claim: hidden
                .as_ref()
                .and_then(|row| row["claim"].as_str().map(str::to_owned)),
            reason: format!("Rob: {reason}"),
            evidence: hidden.map_or_else(|| json!({}), |row| row["evidence"].clone()),
        },
    )?;
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LessonKind {
    Lesson,
    Decision,
    Convention,
}

/// A durable lesson, decision or convention for later recall.
pub struct Lesson<'a> {
    pub kind: LessonKind,
    pub source: &'a str,
    pub repo: &'a str,
    pub topic: Option<&'a str>,
    pub text: &'a str,
    /// Stable identity, so a replay queues the same remote operation.
    pub identity: &'a str,
}

/// Queue a lesson on the retain outbox, tagged so recall ranks it before run records.
pub fn queue_lesson(store: &mut Store, lesson: &Lesson<'_>) -> Result<()> {
    let kind = match lesson.kind {
        LessonKind::Lesson => "lesson",
        LessonKind::Decision => "decision",
        LessonKind::Convention => "convention",
    };
    let repo = safe_text(clip(lesson.repo, 256));
    let source = safe_text(clip(lesson.source, 64));
    let id = uuid(&json!(["lesson", kind, source, repo, lesson.identity]));
    let mut tags = vec![
        format!("repo:{repo}"),
        format!("kind:{kind}"),
        format!("source:{source}"),
    ];
    if let Some(topic) = lesson.topic {
        tags.push(format!("topic:{}", safe_text(clip(topic, 64))));
    }
    let content = clip(&redact_text(lesson.text), 8192).to_owned();
    if content.trim().is_empty() {
        return Err(Error::Invalid("an empty lesson"));
    }
    let item = json!({"content": content, "document_id": format!("tl3-{kind}-{id}"),
        "context": format!("TL3 {kind}"), "tags": tags,
        "metadata": {"source": "tl3", "kind": kind}});
    store.queue_memory(&id, &item)?;
    Ok(())
}

fn normalized(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Check a quote against the base checkout the run read. Code verifies the
/// evidence; the model's word alone never retires or teaches anything.
fn verify(worktree: &Path, evidence: &Value) -> std::result::Result<Value, &'static str> {
    let (Some(path), Some(start), Some(end), Some(quote)) = (
        evidence["path"].as_str(),
        evidence["start_line"].as_u64(),
        evidence["end_line"].as_u64(),
        evidence["quote"].as_str(),
    ) else {
        return Err("the evidence is incomplete");
    };
    if start == 0 || end < start || end - start >= EVIDENCE_LINES {
        return Err("the evidence lines are invalid");
    }
    let relative = Path::new(path);
    let mut current = worktree.to_owned();
    for component in relative.components() {
        match component {
            std::path::Component::Normal(name) if name != ".git" => current.push(name),
            _ => return Err("the evidence path is not a repository file"),
        }
        match std::fs::symlink_metadata(&current) {
            Ok(meta) if !meta.file_type().is_symlink() => {}
            _ => return Err("the evidence path is not a repository file"),
        }
    }
    match std::fs::metadata(&current) {
        Ok(meta) if meta.is_file() && meta.len() <= EVIDENCE_FILE_BYTES => {}
        _ => return Err("the evidence path is not a repository file"),
    }
    let text = std::fs::read_to_string(&current).map_err(|_| "the evidence file is not text")?;
    let lines: Vec<&str> = text
        .lines()
        .skip(start as usize - 1)
        .take((end - start + 1) as usize)
        .collect();
    if lines.len() as u64 != end - start + 1 {
        return Err("the evidence lines are past the end of the file");
    }
    let wanted = normalized(quote);
    if wanted.chars().count() < 8 || wanted.len() > 2000 {
        return Err("the quote is too short or too long");
    }
    if !normalized(&lines.join("\n")).contains(&wanted) {
        return Err("the quote is not at the cited lines");
    }
    Ok(json!({"path": path, "start_line": start, "end_line": end,
        "quote": clip_chars(&safe_text(quote), 500)}))
}

/// File extensions that make a token in a memory name a file.
const FILE_EXTENSIONS: &[&str] = &[
    "py", "rs", "md", "toml", "json", "yaml", "yml", "ts", "tsx", "js", "jsx", "sh", "txt", "cfg",
    "ini", "lock", "sql", "go", "java", "c", "h", "cc", "cpp", "hpp", "cs", "rb", "html", "css",
    "bzl", "bazel", "proto", "xml", "ipynb",
];

/// Code-shaped tokens: words, dotted and `::` names, and paths.
fn tokens(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !(c.is_alphanumeric() || matches!(c, '_' | '.' | ':' | '/' | '-')))
        .map(|token| token.trim_matches(|c| matches!(c, '.' | ':' | '/' | '-')))
        .filter(|token| !token.is_empty())
}

/// A name only code uses: at least 5 characters, a letter, and an underscore, a
/// digit, internal mixed case, `::`, `.` or `/`. Plain words like `database` or
/// `error` appear in too many lines to tie evidence to a claim.
fn identifier_like(token: &str) -> bool {
    token.chars().count() >= 5
        && token.chars().any(char::is_alphabetic)
        && (token.contains('_')
            || token.chars().any(|c| c.is_ascii_digit())
            || token.contains("::")
            || token.contains('.')
            || token.contains('/')
            || (token.chars().skip(1).any(char::is_uppercase)
                && token.chars().any(char::is_lowercase)))
}

/// Identifier-like tokens, with the identifier-like parts of dotted, `::` and
/// path names, so `module._helper` in a claim matches `_helper` in the code.
fn identifiers(text: &str) -> std::collections::BTreeSet<&str> {
    let mut found = std::collections::BTreeSet::new();
    for token in tokens(text).filter(|token| identifier_like(token)) {
        found.insert(token);
        found.extend(
            token
                .split(['.', ':', '/'])
                .filter(|part| identifier_like(part)),
        );
    }
    found
}

/// Files a memory names, by path or by a known extension.
fn named_files(text: &str) -> Vec<&str> {
    tokens(text)
        .filter(|token| !token.contains("://"))
        .filter(|token| {
            token.contains('/')
                || token.rsplit_once('.').is_some_and(|(stem, extension)| {
                    !stem.is_empty() && FILE_EXTENSIONS.contains(&extension)
                })
        })
        .collect()
}

/// A flag shows a memory wrong only when it quotes the memory's own claim, and
/// the repo evidence is about that claim: it shares an identifier with the claim,
/// and comes from the file the memory names, if it names one.
fn contradicts(
    memory: &Value,
    claim: &str,
    worktree: &Path,
    evidence: &Value,
) -> std::result::Result<Value, &'static str> {
    let wanted = normalized(claim);
    if wanted.chars().count() < 8 {
        return Err("the flag does not quote the memory's claim");
    }
    let content = memory["content"].as_str().unwrap_or("");
    if !normalized(content).contains(&wanted) {
        return Err("the claim is not in the memory");
    }
    let evidence = verify(worktree, evidence)?;
    if identifiers(&wanted).is_disjoint(&identifiers(evidence["quote"].as_str().unwrap_or(""))) {
        return Err("the evidence shares no identifier with the claim");
    }
    let files = named_files(content);
    let path = evidence["path"].as_str().unwrap_or("");
    if !files.is_empty()
        && !files
            .iter()
            .any(|file| path == *file || path.ends_with(&format!("/{file}")))
    {
        return Err("the evidence is not from the file the memory names");
    }
    Ok(evidence)
}

fn cite(evidence: &Value) -> String {
    format!(
        "{}:{}-{} \"{}\"",
        evidence["path"].as_str().unwrap_or(""),
        evidence["start_line"],
        evidence["end_line"],
        evidence["quote"].as_str().unwrap_or("")
    )
}

struct Run<'a> {
    role: &'a str,
    repo: &'a str,
    request: &'a str,
    id: &'a str,
}

/// Record a run's memory flags and queue its verified conventions. Only ingest and
/// planner runs qualify: they read the pinned base, so a quote is evidence about the
/// repo rather than about TL3's own change.
fn review_memories(
    store: &mut Store,
    run: &Run<'_>,
    inputs: &Value,
    reply: &Value,
    worktree: &Path,
) -> Result<()> {
    let supplied: std::collections::BTreeMap<&str, &Value> = inputs["memories"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| Some((m["id"].as_str()?, m)))
        .collect();
    let cited: std::collections::BTreeSet<&str> = reply["memory_ids"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    for flag in reply["memory_flags"]
        .as_array()
        .into_iter()
        .flatten()
        .take(PER_RUN)
    {
        let Some(memory_id) = memory_id(&flag["id"]) else {
            continue;
        };
        let reason = safe_text(clip_chars(
            flag["reason"].as_str().unwrap_or("").trim(),
            1000,
        ));
        let claim = flag["claim"].as_str().unwrap_or("").trim();
        let checked = match supplied.get(memory_id) {
            None => Err("the run was not given this memory"),
            Some(_) if !cited.contains(memory_id) => Err("the run did not cite this memory"),
            Some(memory) if memory["kind"] == "decision" => Err("only Rob retires a decision"),
            Some(memory) if memory["repo"].as_str() != Some(run.repo) => {
                Err("the memory is not tagged with this run's repo")
            }
            Some(_) if reason.is_empty() => Err("the flag gives no reason"),
            Some(memory) => contradicts(memory, claim, worktree, &flag["evidence"]),
        };
        // A replayed retirement reuses the operation id, so its first decision stands.
        let operation = uuid(&json!([run.id, "flag", memory_id]));
        let (verdict, state, recorded, evidence) = match checked {
            Ok(evidence) => {
                // One flag hides; another request's flag, or Rob, is the second signal.
                let independent =
                    store
                        .memory_signals(memory_id)?
                        .iter()
                        .any(|(state, source, request)| {
                            state != "valid"
                                && (source == "rob" || request.as_deref() != Some(run.request))
                        });
                (
                    "accepted",
                    if independent { "invalidated" } else { "hidden" },
                    format!(
                        "TL3 {} run {} ({} request {}): {reason} Claim: \"{}\". Evidence: {}",
                        run.role,
                        run.id,
                        run.repo,
                        run.request,
                        clip_chars(claim, 500),
                        cite(&evidence)
                    ),
                    evidence,
                )
            }
            Err(why) => (
                "rejected",
                "hidden",
                format!("Not hidden: {why}. The flag said: {reason}"),
                section(&flag["evidence"]),
            ),
        };
        store.record_memory_curation(
            &operation,
            &tl3_store::MemoryCuration {
                memory_id: memory_id.into(),
                state,
                verdict,
                source: run.role.into(),
                request: Some(run.request.into()),
                run: Some(run.id.into()),
                claim: (!claim.is_empty()).then(|| safe_text(clip_chars(claim, 1000))),
                reason: clip_chars(&recorded, 4000).into(),
                evidence,
            },
        )?;
    }
    if run.role == "planner" {
        for (index, convention) in reply["conventions"]
            .as_array()
            .into_iter()
            .flatten()
            .take(PER_RUN)
            .enumerate()
        {
            let text = convention["text"].as_str().unwrap_or("").trim();
            if text.is_empty() {
                continue;
            }
            let Ok(evidence) = verify(worktree, &convention["evidence"]) else {
                continue;
            };
            queue_lesson(
                store,
                &Lesson {
                    kind: LessonKind::Convention,
                    source: "planner",
                    repo: run.repo,
                    topic: None,
                    text: &format!("{}\nEvidence: {}", clip_chars(text, 1000), cite(&evidence)),
                    identity: &format!("{}:convention:{index}", run.id),
                },
            )?;
        }
    }
    Ok(())
}

/// Send accepted curations in decision order. A PATCH sets the same state again on
/// replay, so retrying after an ambiguous response is safe.
fn curate(store: &mut Store, settings: &tl3_config::Hindsight) -> Result<usize> {
    let mut pending = store.pending_memory_curations()?;
    if pending.is_empty() {
        return Ok(0);
    }
    let key = settings.retain_key()?;
    let client = client(Duration::from_secs(10))?;
    let mut count = 0;
    while !pending.is_empty() {
        for (sequence, memory_id, state, reason) in pending {
            if memory_needs_rob(&store.load()?, &format!("memory:curation:{sequence}"))? {
                return Err(Error::Invalid(
                    "memory curation waits for Rob to clear its incident",
                ));
            }
            let url = bank_url(settings, &[&memory_id])?;
            let status = crate::executor::runtime()?.block_on(async {
                let mut request = client
                    .patch(url)
                    .json(&json!({"state": state, "reason": reason}));
                if let Some(key) = &key {
                    request = request.bearer_auth(key);
                }
                request
                    .send()
                    .await
                    .map(|response| response.status().as_u16())
                    .map_err(|e| Error::Adapter(Box::new(e)))
            })?;
            match status {
                200..=299 => {
                    store.settle_memory_curation(sequence, None)?;
                    count += 1;
                }
                401 | 403 => {
                    store.reject_memory_key(status)?;
                    return Err(Error::HindsightStatus(status));
                }
                408 | 429 => return Err(Error::HindsightStatus(status)),
                400..=499 => store.settle_memory_curation(sequence, Some(status))?,
                _ => return Err(Error::HindsightStatus(status)),
            }
        }
        pending = store.pending_memory_curations()?;
    }
    Ok(count)
}

/// The memory feed's announcer, set once production knows the operations issue.
pub(crate) type Announcer = std::sync::Arc<std::sync::Mutex<Option<Box<dyn Announce>>>>;

#[derive(Clone)]
pub(crate) struct Feed(Arc<FeedWake>);

enum FeedEvent {
    Wake,
    Barrier(Sender<()>),
    Stop,
}

struct FeedWake {
    sender: Sender<FeedEvent>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
    life: Arc<FeedLife>,
    generation: AtomicI64,
}

#[derive(Default)]
struct FeedLife {
    queued: AtomicBool,
    retry: AtomicBool,
    held: AtomicBool,
    failed: AtomicBool,
    exited: AtomicBool,
    supervisor: Mutex<Option<crate::sync::mpsc::Sender<crate::cli::Input>>>,
}

/// Exit evidence is independent of the feed handle's cancellation ownership.
#[derive(Clone)]
pub(crate) struct FeedExit(Arc<FeedLife>);

impl FeedExit {
    pub(crate) fn supervise(&self, sender: &crate::sync::mpsc::Sender<crate::cli::Input>) {
        let mut supervisor = self.0.supervisor.lock().unwrap();
        if supervisor.is_none() {
            *supervisor = Some(sender.clone());
            // Registration racing exit must also deliver the wake.
            if self.0.exited.load(Ordering::SeqCst) {
                let _ = sender.send(crate::cli::Input::Wake);
            }
        }
    }
}

impl FeedLife {
    fn exited(&self) {
        let supervisor = self.supervisor.lock().unwrap();
        self.exited.store(true, Ordering::SeqCst);
        if let Some(sender) = supervisor.as_ref() {
            let _ = sender.send(crate::cli::Input::Wake);
        }
    }
}

impl FeedWake {
    fn join(&self) {
        if let Some(thread) = self.thread.lock().unwrap().take()
            && thread.join().is_err()
        {
            eprintln!("memory feed thread panicked");
        }
    }
}

impl Drop for FeedWake {
    fn drop(&mut self) {
        // #773: cancellation is local, even when Postgres refuses new sessions.
        let _ = self.sender.send(FeedEvent::Stop);
        self.join();
    }
}

fn run_feed(
    store: &mut Store,
    settings: &tl3_config::Hindsight,
    breaker: &Breaker,
    announcer: &Announcer,
    receiver: &mpsc::Receiver<FeedEvent>,
    life: &FeedLife,
) -> Result<()> {
    for event in receiver {
        match event {
            FeedEvent::Stop => break,
            FeedEvent::Barrier(reply) => {
                // A processed-event barrier also detects a lost writer session.
                store.memory_retry_generation()?;
                let _ = reply.send(());
                continue;
            }
            FeedEvent::Wake => life.queued.store(false, Ordering::SeqCst),
        }
        let retry = life.retry.swap(false, Ordering::SeqCst);
        if life.held.load(Ordering::SeqCst) && !retry {
            continue;
        }
        // Q7: a later incident holds this lineage until Rob clears it.
        if memory_needs_rob(&store.load()?, &memory_lineage(store)?)? {
            life.held.store(true, Ordering::SeqCst);
            store.set_memory_feed_state(tl3_store::MemoryFeedState::Held)?;
            continue;
        }
        store.set_memory_feed_state(tl3_store::MemoryFeedState::Delivering)?;
        let delivery = deliver(store, settings, breaker);
        if let Some(announcer) = announcer.lock().unwrap().as_deref()
            && let Err(error) = announce(store, announcer)
        {
            eprintln!("memory curation notes retained for retry: {error}");
        }
        let state = match delivery {
            Ok(_) => tl3_store::MemoryFeedState::Ready,
            Err(error @ Error::Store(_)) => return Err(error),
            Err(error) => {
                eprintln!("Hindsight memory retained for retry: {error}");
                tl3_store::MemoryFeedState::Held
            }
        };
        life.held
            .store(state == tl3_store::MemoryFeedState::Held, Ordering::SeqCst);
        // The queue's acknowledged deletes precede healthy evidence and ingress wake.
        store.set_memory_feed_state(state)?;
    }
    Ok(())
}

impl Feed {
    pub(crate) fn start(
        settings: tl3_config::Store,
        hindsight: tl3_config::Hindsight,
        breaker: Breaker,
        announcer: Announcer,
    ) -> Result<Self> {
        let mut store = Store::open(&settings)?;
        store.claim_memory_feed()?;
        let generation = store.memory_retry_generation()?;
        let (sender, receiver) = mpsc::channel();
        let life = Arc::new(FeedLife::default());
        let thread_life = life.clone();
        let thread = std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_feed(
                    &mut store,
                    &hindsight,
                    &breaker,
                    &announcer,
                    &receiver,
                    &thread_life,
                )
            }));
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    thread_life.failed.store(true, Ordering::SeqCst);
                    eprintln!("memory feed failed; no consumer; restart tl3d: {error}");
                }
                Err(_) => {
                    thread_life.failed.store(true, Ordering::SeqCst);
                    eprintln!("memory feed panicked; no consumer; restart tl3d");
                }
            }
            // Release the consumer lock before disconnecting pending barriers.
            drop(store);
            thread_life.exited();
            drop(receiver);
        });
        let feed = Self(Arc::new(FeedWake {
            sender,
            thread: Mutex::new(Some(thread)),
            life,
            generation: AtomicI64::new(generation),
        }));
        feed.wake()?;
        Ok(feed)
    }

    pub(crate) fn exits(&self) -> FeedExit {
        FeedExit(self.0.life.clone())
    }

    pub(crate) fn refresh(&self, store: &mut Store) -> Result<()> {
        if store.memory_feed_state()? == tl3_store::MemoryFeedState::Failed {
            self.0.life.failed.store(true, Ordering::SeqCst);
            let _ = self.0.sender.send(FeedEvent::Stop);
            return Err(Error::Invalid(
                "memory feed failed: no consumer; restart tl3d",
            ));
        }
        let generation = store.memory_retry_generation()?;
        if self.0.generation.swap(generation, Ordering::SeqCst) != generation {
            self.retry()?;
        } else {
            self.wake()?;
        }
        Ok(())
    }

    pub(crate) fn retry(&self) -> Result<()> {
        self.0.life.retry.store(true, Ordering::SeqCst);
        self.wake()
    }

    pub(crate) fn wake(&self) -> Result<()> {
        if self.0.life.failed.load(Ordering::SeqCst) {
            return Err(Error::Invalid(
                "memory feed failed: no consumer; restart tl3d",
            ));
        }
        if self.0.life.held.load(Ordering::SeqCst) && !self.0.life.retry.load(Ordering::SeqCst) {
            return Ok(());
        }
        if !self.0.life.queued.swap(true, Ordering::SeqCst) {
            self.0
                .sender
                .send(FeedEvent::Wake)
                .map_err(|_| Error::Invalid("memory feed: no consumer; restart tl3d"))?;
        }
        Ok(())
    }

    pub(crate) fn barrier(&self) -> Result<()> {
        let (sender, receiver) = mpsc::channel();
        self.0
            .sender
            .send(FeedEvent::Barrier(sender))
            .map_err(|_| Error::Invalid("memory feed: no consumer; restart tl3d"))?;
        if receiver.recv().is_err() {
            self.0.join();
            return Err(Error::Invalid(
                "memory feed failed: no consumer; restart tl3d",
            ));
        }
        Ok(())
    }
}

/// #773: Hindsight uses the controller's single earliest-deadline look, as
/// #759's unavailable-model probe does. A deadline requests evidence only;
/// the acknowledged delivery is what clears an incident.
const PROBE_MS: u64 = 120_000;
const MEMORY_PROBES: &str = "upstream_probes";
const MEMORY_PROBE: &str = "hindsight";

#[derive(serde::Serialize, serde::Deserialize)]
struct MemoryProbe {
    due_ms: u64,
}

pub(crate) fn next_probe_ms(snapshot: &tl3_store::Snapshot) -> Result<Option<u64>> {
    snapshot
        .rows
        .iter()
        .find(|row| row.table == MEMORY_PROBES && row.id == MEMORY_PROBE)
        .map(|row| crate::decode_stored::<MemoryProbe>(row.value.clone()).map(|probe| probe.due_ms))
        .transpose()
}

fn memory_lineage(store: &mut Store) -> Result<String> {
    if let Some((sequence, ..)) = store.pending_memory_curations()?.first() {
        return Ok(format!("memory:curation:{sequence}"));
    }
    Ok(store.pending_memories(0)?.first().map_or_else(
        || "memory:empty".into(),
        |(_, operation, _)| format!("memory:retain:{operation}"),
    ))
}

fn memory_needs_rob(snapshot: &tl3_store::Snapshot, lineage: &str) -> Result<bool> {
    for row in snapshot.rows.iter().filter(|row| row.table == "incidents") {
        let incident: crate::looks::Incident = crate::decode_stored(row.value.clone())?;
        if incident.open
            && incident.needs_rob
            && incident
                .input_failure
                .as_ref()
                .is_some_and(|failure| failure.memory_lineage(lineage))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

impl crate::Controller {
    pub(crate) fn reconcile_memory(&mut self, snapshot: &tl3_store::Snapshot) -> Result<bool> {
        use crate::looks::{Cause, Incident, IncidentSignature};
        use tl3_store::{Commit, MemoryFeedState, RowChange};
        let state = self.store.memory_feed_state()?;
        let incidents = snapshot
            .rows
            .iter()
            .filter(|row| row.table == "incidents")
            .map(|row| crate::decode_stored::<Incident>(row.value.clone()))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut open = incidents.iter().filter(|incident| {
            incident.open
                && matches!(
                    incident.signature.class.as_str(),
                    "hindsight_unavailable" | "memory_feed_failed"
                )
        });
        if state == MemoryFeedState::Ready {
            if let Some(incident) = open.find(|incident| !incident.needs_rob) {
                self.probe_passed(None, Some(&incident.id))?;
                return Ok(true);
            }
            if next_probe_ms(snapshot)?.is_some() {
                let mut commit = Commit::new(
                    format!("memory-probe-retire:{}", snapshot.revision.get()),
                    json!({"memory_recovered":true}),
                    snapshot,
                );
                commit.rows.push(RowChange::Delete {
                    table: MEMORY_PROBES.into(),
                    id: MEMORY_PROBE.into(),
                });
                self.store.commit(&commit)?;
                return Ok(true);
            }
            return Ok(false);
        }
        if !matches!(state, MemoryFeedState::Held | MemoryFeedState::Failed) {
            return Ok(false);
        }
        let version = self.store.memory_feed_version()?;
        let lineage = if state == MemoryFeedState::Held {
            memory_lineage(&mut self.store)?
        } else {
            "memory:feed".into()
        };
        let count = incidents
            .iter()
            .filter(|incident| {
                incident
                    .input_failure
                    .as_ref()
                    .is_some_and(|failure| failure.memory_lineage(&lineage))
            })
            .count() as u64
            + 1;
        let needs_rob =
            crate::looks::incident_needs_rob(crate::looks::IncidentCause::ReadFailure {
                incidents: count,
            });
        let (class, cause, mut summary) = if state == MemoryFeedState::Held {
            let pending = self.store.pending_memory_count()?;
            (
                "hindsight_unavailable",
                Cause::Upstream,
                format!(
                    "Hindsight unavailable: {pending} memories held. Restore Hindsight or its credentials. Other requests can continue."
                ),
            )
        } else {
            ("memory_feed_failed", Cause::Tl3, "Memory feed failed: no consumer. Restart tl3d to reattach the durable queue. Other requests can continue.".into())
        };
        if let Some(previous) = open.find(|incident| {
            incident.signature.class == class
                && incident
                    .input_failure
                    .as_ref()
                    .is_some_and(|failure| failure.memory_lineage(&lineage))
        }) {
            // A reused incident keeps its original escalation decision.
            if previous.needs_rob {
                summary.push_str(&format!(
                    " Only Rob can clear this later incident: `clear incident {}`.",
                    previous.id
                ));
            } else if state == MemoryFeedState::Held {
                summary.push_str(" The controller probes in two minutes and drains automatically after acknowledgement; `tl3 memory retry --config FILE` requests an earlier attempt.");
            }
            let probe_armed = next_probe_ms(snapshot)?.is_some();
            if previous.summary == summary
                && (probe_armed == (state == MemoryFeedState::Held))
                && previous.input_failure.as_ref().is_some_and(|failure| {
                    failure.memory_lineage(&lineage) && failure.memory_version == Some(version)
                })
            {
                return Ok(false);
            }
            let mut incident = previous.clone();
            incident.summary = summary;
            incident.input_failure = Some(crate::input_errors::InputFailure::memory(
                "Hindsight memory delivery",
                &lineage,
                version,
            ));
            // Preserve posting evidence when updating health or rearming a probe.
            if let Some(failure) = previous.input_failure.as_ref() {
                incident.input_failure = Some(failure.with_memory_lineage(&lineage, version));
            }
            let mut commit = Commit::new(
                format!("memory-health:{}", snapshot.revision.get()),
                json!({"memory_health":true}),
                snapshot,
            );
            commit.rows.push(crate::put(
                "incidents",
                incident.id.clone(),
                json!(incident),
            ));
            if state == MemoryFeedState::Held && !probe_armed {
                commit.rows.push(crate::put(
                    MEMORY_PROBES,
                    MEMORY_PROBE.into(),
                    json!(MemoryProbe {
                        due_ms: self.now_ms().saturating_add(PROBE_MS)
                    }),
                ));
            }
            if state == MemoryFeedState::Failed && next_probe_ms(snapshot)?.is_some() {
                commit.rows.push(RowChange::Delete {
                    table: MEMORY_PROBES.into(),
                    id: MEMORY_PROBE.into(),
                });
            }
            self.store.commit(&commit)?;
            return Ok(true);
        }
        // Rob's clearance is not another delivery failure. Wait for a new
        // attempt outcome before creating another incident for this lineage.
        if incidents.iter().any(|incident| {
            !incident.open
                && incident.input_failure.as_ref().is_some_and(|failure| {
                    failure.memory_lineage(&lineage) && failure.memory_version == Some(version)
                })
        }) {
            return Ok(false);
        }
        let id = crate::looks::incident_id(snapshot, "memory");
        if needs_rob {
            summary.push_str(&format!(
                " Only Rob can clear this later incident: `clear incident {id}`."
            ));
        } else if state == MemoryFeedState::Held {
            summary.push_str(" The controller probes in two minutes and drains automatically after acknowledgement; `tl3 memory retry --config FILE` requests an earlier attempt.");
        }
        let incident = Incident {
            id: id.clone(),
            node: None,
            input_failure: Some(crate::input_errors::InputFailure::memory(
                "Hindsight memory delivery",
                &lineage,
                version,
            )),
            open: true,
            needs_rob,
            blocking: false,
            summary,
            signature: IncidentSignature {
                upstream: crate::executor::Upstream::Workers,
                class: class.into(),
            },
            effect: None,
            post_key: Some(format!("incident:{id}")),
            cause: Some(cause),
        };
        let mut commit = Commit::new(
            format!("memory-incident:{}", snapshot.revision.get()),
            json!({"memory_incident":id}),
            snapshot,
        );
        commit
            .rows
            .push(crate::put("incidents", id, json!(incident)));
        if state == MemoryFeedState::Held && next_probe_ms(snapshot)?.is_none() {
            commit.rows.push(crate::put(
                MEMORY_PROBES,
                MEMORY_PROBE.into(),
                json!(MemoryProbe {
                    due_ms: self.now_ms().saturating_add(PROBE_MS)
                }),
            ));
        }
        if state == MemoryFeedState::Failed && next_probe_ms(snapshot)?.is_some() {
            commit.rows.push(RowChange::Delete {
                table: MEMORY_PROBES.into(),
                id: MEMORY_PROBE.into(),
            });
        }
        self.store.commit(&commit)?;
        Ok(true)
    }

    pub(crate) fn look_memory_probe(&mut self, force: bool, key: Option<&str>) -> Result<()> {
        if key.is_some() {
            return Ok(());
        }
        let snapshot = self.store.load()?;
        let Some(due) = next_probe_ms(&snapshot)? else {
            return Ok(());
        };
        let now = self.now_ms();
        if !force && due > now {
            return Ok(());
        }
        // This schedules one real operation under its original UUID. A probe
        // deadline never manufactures healthy evidence or clears the hold.
        if !memory_needs_rob(&snapshot, &memory_lineage(&mut self.store)?)?
            && let Err(error) = self
                .adapters
                .get(crate::executor::Upstream::Workers)?
                .probe_memory()
        {
            eprintln!("Hindsight memory probe failed: {error}");
        }
        let mut commit = tl3_store::Commit::new(
            format!("memory-probe:{}", snapshot.revision.get()),
            json!({"memory_probe":true}),
            &snapshot,
        );
        commit.rows.push(crate::put(
            MEMORY_PROBES,
            MEMORY_PROBE.into(),
            json!(MemoryProbe {
                due_ms: now.saturating_add(PROBE_MS)
            }),
        ));
        self.store.commit(&commit)?;
        Ok(())
    }
}

fn stored_json_bytes(value: &Value) -> usize {
    match value {
        Value::Array(items) => {
            2 + items.iter().map(stored_json_bytes).sum::<usize>()
                + items.len().saturating_sub(1) * 2
        }
        Value::Object(items) => {
            2 + items
                .iter()
                .map(|(key, value)| {
                    serde_json::to_string(key).expect("JSON key").len()
                        + 2
                        + stored_json_bytes(value)
                })
                .sum::<usize>()
                + items.len().saturating_sub(1) * 2
        }
        value => value.to_string().len(),
    }
}

#[cfg(test)]
mod review_tests {
    use super::*;

    #[test]
    fn size_includes_postgres_jsonb_spaces() {
        assert_eq!(
            stored_json_bytes(&json!({"a": [1, 2]})),
            r#"{"a": [1, 2]}"#.len()
        );
    }

    macro_rules! secret_case {
        ($name:ident, $secret:expr) => {
            #[test]
            fn $name() {
                assert_eq!(safe_text($secret), "[redacted]", "secret must be removed");
            }
        };
    }
    secret_case!(github_app_token, "ghs_fixture1234567890");
    secret_case!(github_user_token, "ghu_fixture1234567890");
    secret_case!(github_refresh_token, "ghr_fixture1234567890");
    secret_case!(github_installation_token, "ghp_fixture1234567890");
    secret_case!(github_oauth_token, "gho_fixture1234567890");
    secret_case!(github_fine_grained_token, "github_pat_fixture1234567890");
    secret_case!(aws_token, "AKIAIOSFODNN7EXAMPLE");
    secret_case!(slack_token, "xoxb-fixture1234567890");
    secret_case!(slack_user_token, "xoxp-fixture1234567890");
    secret_case!(huggingface_token, "hf_fixture1234567890");
    secret_case!(gitlab_token, "glpat-fixture1234567890");
    secret_case!(model_token, "sk-fixture1234567890");
    secret_case!(hex_token, "f9c83a01b647de2390abc81456fed279");
    #[test]
    fn postgres_credentials() {
        assert_eq!(
            safe_text("postgres://user:fixture-pass@db.invalid/tl3"),
            "[redacted]db.invalid/tl3"
        );
    }
    secret_case!(prefixed_api_key, "SERVICE_API_KEY=fixture-value");
    secret_case!(prefixed_secret, "SERVICE_SECRET_VALUE=fixture-value");
    secret_case!(prefixed_token, "SERVICE_TOKEN=fixture-value");

    #[test]
    fn sensitive_suffixes() {
        for key in [
            "service_key",
            "SERVICE_KEY",
            "service_token",
            "service_secret",
            "dbpassword",
        ] {
            assert!(sensitive_key(key), "sensitive key {key}");
        }
    }
}
