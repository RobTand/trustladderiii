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
        LazyLock,
        mpsc::{self, SyncSender},
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

fn sensitive_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase().replace('-', "_");
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
    ) || key.ends_with("_password")
        || key.ends_with("_secret")
        || key.ends_with("_token")
}

fn safe_text(text: &str) -> String {
    static SECRET: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
        r"(?is)(?:bearer\s+\S+|-----BEGIN[^\r\n]*PRIVATE KEY|(?:sk-|ghp_|gho_|github_pat_)[a-zA-Z0-9_-]+|eyJ[a-zA-Z0-9_-]+\.[a-zA-Z0-9_-]+\.|https?://[^/\s:@]+:[^/\s@]+@|(?:api[_-]?key|password|passwd|secret|token|authorization|cookie)\s*[=:]\s*\S+)"
    ).expect("memory secret pattern")
    });
    if SECRET.is_match(text) {
        "[redacted]".into()
    } else if text.len() > 2048 {
        format!("{} [truncated]", clip(text, 2048))
    } else {
        text.into()
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
    let value = sanitize(value, 8, &mut 256);
    let encoded = value.to_string();
    if encoded.len() <= SECTION_BYTES {
        value
    } else {
        json!({"summary": clip(&encoded, SECTION_BYTES / 2), "truncated": true})
    }
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
    let mut hash = Sha256::digest(identity.to_string().as_bytes());
    hash[6] = (hash[6] & 0x0f) | 0x50;
    hash[8] = (hash[8] & 0x3f) | 0x80;
    let h: String = hash[..16].iter().map(|b| format!("{b:02x}")).collect();
    let id = format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    );
    // Infrastructure inputs and repository contents belong in the trace, not memory.
    let mut inputs = request["inputs"].clone();
    if let Some(inputs) = inputs.as_object_mut() {
        for field in ["tests", "hindsight", "forecast"] {
            inputs.remove(field);
        }
        if let Some(repo) = inputs.get_mut("repo").and_then(Value::as_object_mut) {
            repo.remove("materials");
        }
    }
    let mut record = json!({
        "role": role, "model": section(&request["model"]["name"]), "repo": repo,
        "issue": context["issue"], "request": request_id, "run": id,
        "inputs": section(&inputs), "activity": section(&summary["activity"]),
        "reply": section(&reply), "outcome": if succeeded { "succeeded" } else { "failed" },
        "reason": section(&reason),
    });
    let mut item = json!({"content": record.to_string(), "document_id": format!("tl3-run-{id}"),
        "context": "TL3 Deep Agents run summary", "tags": [format!("repo:{repo}"), format!("request:{request_id}"), format!("role:{role}")],
        "metadata": {"source": "tl3", "run": id}});
    if item.to_string().len() > RECORD_BYTES {
        for field in ["inputs", "activity", "reply", "reason", "model"] {
            record[field] =
                json!({"summary": clip(&record[field].to_string(), 512), "truncated": true});
        }
        item["content"] = json!(record.to_string());
    }
    store.queue_memory(&id, &item)?;
    Ok(())
}

/// Hindsight owns the durable async operation after acknowledgement. Retrying
/// after any ambiguous response reuses its UUID and cannot enqueue new work.
pub fn flush(store: &mut Store, settings: &tl3_config::Hindsight) -> Result<usize> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| Error::Adapter(Box::new(e)))?;
    let mut url = settings.endpoint.clone();
    url.path_segments_mut()
        .map_err(|_| Error::Invalid("invalid Hindsight endpoint"))?
        .clear()
        .extend(["v1", "default", "banks", &settings.bank, "memories"]);
    let mut after = 0;
    let mut count = 0;
    let mut failure = None;
    loop {
        let pending = store.pending_memories(after)?;
        if pending.is_empty() {
            break;
        }
        for (sequence, id, item) in pending {
            after = sequence;
            let result: Result<()> = crate::executor::runtime()?.block_on(async {
                let response = client
                    .post(url.clone())
                    .json(&json!({"async": true, "operation_id": id, "items": [item]}))
                    .send()
                    .await
                    .map_err(|e| Error::Adapter(Box::new(e)))?;
                if !response.status().is_success() {
                    return Err(Error::Invalid("Hindsight retain was not acknowledged"));
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
                Err(error) => {
                    failure = Some(error);
                }
            }
        }
    }
    match failure {
        Some(error) => Err(error),
        None => Ok(count),
    }
}

#[derive(Clone)]
pub(crate) struct Feed(SyncSender<()>);
impl Feed {
    pub(crate) fn start(store: tl3_config::Store, settings: tl3_config::Hindsight) -> Self {
        let (sender, receiver) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            while receiver.recv().is_ok() {
                if let Err(error) = Store::open(&store)
                    .map_err(Error::from)
                    .and_then(|mut store| flush(&mut store, &settings))
                {
                    eprintln!("Hindsight memory retained for retry: {error}");
                }
            }
        });
        let feed = Self(sender);
        feed.wake();
        feed
    }
    pub(crate) fn wake(&self) {
        let _ = self.0.try_send(());
    }
}
