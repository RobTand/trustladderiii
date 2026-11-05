use super::*;
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
    let succeeded = retain::run_succeeded(
        exit["code"] == 0,
        summary["outcome"] == "failed",
        !reply.is_null(),
    );
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
    let hash = Sha256::digest(identity.to_string().as_bytes());
    let (high, low) = retain::identity_bits(
        u64::from_be_bytes(hash[..8].try_into().expect("SHA-256 prefix")),
        u64::from_be_bytes(hash[8..16].try_into().expect("SHA-256 prefix")),
    );
    let id = format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        high >> 32,
        (high >> 16) & 0xffff,
        high & 0xffff,
        low >> 48,
        low & 0xffffffffffff,
    );
    if retain::review_eligible(succeeded, matches!(role.as_str(), "ingest" | "planner")) {
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
        && retain::head_eligible(
            true,
            head.len(),
            head.bytes().all(|b| b.is_ascii_hexdigit()),
        )
    {
        record["head_sha"] = json!(head);
    }
    let mut item = json!({"content": record.to_string(), "document_id": format!("tl3-run-{id}"),
        "context": "TL3 Deep Agents run summary", "tags": [format!("repo:{repo}"), format!("request:{request_id}"), format!("role:{role}"), "kind:run", "source:run"],
        "metadata": {"source": "tl3", "run": id}});
    if !retain::record_fits(stored_json_bytes(&item)) {
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
pub fn flush(store: &mut Store, settings: &tl3_config::Hindsight) -> Result<usize> {
    store.expire_parked_memories()?;
    // A wrong memory misleads every run until Hindsight retires it, so curations go first.
    let mut count = curate(store, settings)?;
    let mut pending = store.pending_memories(0)?;
    if pending.is_empty() {
        if retain::made_progress(count) {
            store.memory_delivery_ack()?;
        }
        return Ok(count);
    }
    memory_attempt(store, &format!("memory:retain:{}", pending[0].1))?;
    let key = settings.retain_key()?;
    let client = client(Duration::from_secs(10))?;
    let url = bank_url(settings, &[])?;
    let mut after = 0;
    loop {
        for (sequence, id, item) in pending {
            // Q7: a preceding curation cannot authorize this operation's retry.
            memory_attempt(store, &format!("memory:retain:{id}"))?;
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
                if !retain::acknowledged(
                    body["success"] == true,
                    body["async"] == true,
                    body["operation_id"] == id,
                    body["bank_id"] == settings.bank,
                    body["items_count"] == 1,
                ) {
                    return Err(Error::Invalid("invalid Hindsight retain acknowledgement"));
                }
                Ok(())
            });
            let status = match &result {
                Err(Error::HindsightStatus(status)) => Some(*status),
                _ => None,
            };
            match retain::retain_action(result.is_ok(), status) {
                MemoryAction::Done => {
                    store.forget_memory(&id)?;
                    count += 1;
                }
                MemoryAction::RejectKey => {
                    let status = status.expect("key rejection has an HTTP status");
                    store.reject_memory_key(status)?;
                    return Err(Error::HindsightStatus(status));
                }
                MemoryAction::Park => {
                    store.park_memory(&id, status.expect("parked memory has an HTTP status"))?;
                }
                MemoryAction::Retry => return Err(result.expect_err("retry requires a failure")),
            }
        }
        pending = store.pending_memories(after)?;
        if pending.is_empty() {
            if !retain::made_progress(count) || store.replay_capacity_memories()? == 0 {
                break;
            }
            after = 0;
            pending = store.pending_memories(after)?;
        }
    }
    if retain::made_progress(count) {
        store.memory_delivery_ack()?;
    }
    Ok(count)
}
fn curate(store: &mut Store, settings: &tl3_config::Hindsight) -> Result<usize> {
    let mut pending = store.pending_memory_curations()?;
    if pending.is_empty() {
        return Ok(0);
    }
    memory_attempt(store, &format!("memory:curation:{}", pending[0].0))?;
    let key = settings.retain_key()?;
    let client = client(Duration::from_secs(10))?;
    let mut count = 0;
    while !pending.is_empty() {
        for (sequence, memory_id, state, reason) in pending {
            memory_attempt(store, &format!("memory:curation:{sequence}"))?;
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
            match retain::http_action(status) {
                MemoryAction::Done => {
                    store.settle_memory_curation(sequence, None)?;
                    count += 1;
                }
                MemoryAction::RejectKey => {
                    store.reject_memory_key(status)?;
                    return Err(Error::HindsightStatus(status));
                }
                MemoryAction::Park => store.settle_memory_curation(sequence, Some(status))?,
                MemoryAction::Retry => return Err(Error::HindsightStatus(status)),
            }
        }
        pending = store.pending_memory_curations()?;
    }
    Ok(count)
}
