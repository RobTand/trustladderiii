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
fn memory_id(value: &Value) -> Option<&str> {
    value.as_str().filter(|id| valid_id(id))
}
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 256
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_:.".contains(&b))
}
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
fn tokens(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !(c.is_alphanumeric() || matches!(c, '_' | '.' | ':' | '/' | '-')))
        .map(|token| token.trim_matches(|c| matches!(c, '.' | ':' | '/' | '-')))
        .filter(|token| !token.is_empty())
}
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
fn memory_attempt(store: &mut Store, lineage: &str) -> Result<()> {
    let held = memory_needs_rob(&store.load()?, lineage)?;
    // Persist identity before key/client/HTTP failures can produce an outcome.
    // A blocked operation restores its existing hold after independent work.
    store.set_memory_feed_attempt(lineage, held)?;
    if held {
        return Err(Error::Invalid(
            "memory delivery waits for Rob to clear its incident",
        ));
    }
    Ok(())
}
