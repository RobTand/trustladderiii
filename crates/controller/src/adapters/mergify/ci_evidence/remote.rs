use crate::findings::{CiGapReason as Gap, CiNodeStatus, CiRetrieval, CiTargetResult, CiTestNode};
use reqwest::redirect::Policy;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeSet, time::Duration};
use tokio::time::Instant;

const MAX_CALLS: usize = 32;
const MAX_BYTES: usize = 1024 * 1024;
const MAX_ACTIONS: usize = 16;
const MAX_NODES: usize = 200;

pub(super) struct Budget {
    remaining: usize,
    deadline: Instant,
}

impl Budget {
    pub(super) fn remaining_time(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            remaining: MAX_CALLS,
            deadline: Instant::now() + Duration::from_secs(20),
        }
    }
}

// No Debug or Serialize: credentials and transport objects stay in this boundary.
pub(super) struct Client {
    pub(super) api: String,
    pub(super) key: String,
}

impl Client {
    pub(super) fn configured(config: &tl3_config::BuildBuddy) -> Result<Self, Gap> {
        let key = config
            .diagnostic_key()
            .map_err(|_| Gap::AccessUnavailable)?
            .ok_or(Gap::AccessUnavailable)?;
        Ok(Self {
            api: "https://app.buildbuddy.io/api/v1/".into(),
            key,
        })
    }

    pub(super) fn remove_key(&self, text: &str) -> String {
        super::terminal_text(text).replace(&self.key, "[redacted]")
    }

    pub(super) async fn resolve(
        &self,
        repo: &str,
        sha: &str,
        binding: &tl3_github::WorkflowJob,
        parent: &str,
        expected: &BTreeSet<String>,
        budget: &mut Budget,
    ) -> Resolution {
        let mut session = Session {
            client: self,
            http: reqwest::Client::builder()
                .redirect(Policy::none())
                .timeout(Duration::from_secs(5))
                .build()
                .ok(),
            budget,
            check: binding.id,
            binding,
            retrievals: Vec::new(),
            results: Vec::new(),
        };
        let gap = session.collect(repo, sha, parent, expected).await.err();
        Resolution {
            results: session.results,
            retrievals: session.retrievals,
            gap,
        }
    }
}

pub(super) struct Resolution {
    pub(super) results: Vec<CiTargetResult>,
    pub(super) retrievals: Vec<CiRetrieval>,
    pub(super) gap: Option<Gap>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Id {
    #[serde(alias = "invocation_id")]
    invocation_id: String,
    #[serde(default, alias = "target_id")]
    target_id: String,
    #[serde(default, alias = "configuration_id")]
    configuration_id: String,
    #[serde(default, alias = "action_id")]
    action_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Invocation {
    id: Id,
    #[serde(alias = "repo_url")]
    repo_url: String,
    #[serde(alias = "commit_sha")]
    commit_sha: String,
    invocation_status: String,
    command: String,
    #[serde(default, alias = "child_invocations")]
    child_invocations: Vec<Id>,
    #[serde(default, alias = "build_metadata")]
    build_metadata: Vec<Metadata>,
}

#[derive(Deserialize)]
struct Metadata {
    key: String,
    value: String,
}

#[derive(Deserialize)]
struct Target {
    id: Id,
    label: String,
    status: String,
}

#[derive(Deserialize)]
struct File {
    name: String,
    uri: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Action {
    id: Id,
    #[serde(alias = "target_label")]
    target_label: String,
    #[serde(default, deserialize_with = "index")]
    shard: u64,
    #[serde(default, deserialize_with = "index")]
    run: u64,
    #[serde(default, deserialize_with = "index")]
    attempt: u64,
    #[serde(default)]
    file: Vec<File>,
}

fn index<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Index {
        Integer(u64),
        String(String),
    }
    match Index::deserialize(deserializer)? {
        Index::Integer(value) => Ok(value),
        Index::String(value) => value.parse().map_err(serde::de::Error::custom),
    }
}

struct Session<'a> {
    client: &'a Client,
    http: Option<reqwest::Client>,
    budget: &'a mut Budget,
    check: u64,
    binding: &'a tl3_github::WorkflowJob,
    retrievals: Vec<CiRetrieval>,
    results: Vec<CiTargetResult>,
}

impl Session<'_> {
    async fn call(&mut self, method: &str, body: Value) -> Result<Vec<u8>, Gap> {
        if self.budget.remaining == 0 || Instant::now() >= self.budget.deadline {
            return Err(Gap::RetrievalLimit);
        }
        self.budget.remaining -= 1;
        let result = self.request(method, body).await;
        self.retrievals.push(CiRetrieval {
            check: self.check,
            method: method.into(),
            attempt: self.retrievals.len() + 1,
            error: result.as_ref().err().copied(),
        });
        result
    }

    async fn request(&self, method: &str, body: Value) -> Result<Vec<u8>, Gap> {
        let http = self.http.as_ref().ok_or(Gap::Unavailable)?;
        let mut response = http
            .post(format!("{}{method}", self.client.api))
            .header("x-buildbuddy-api-key", &self.client.key)
            .timeout(
                self.budget
                    .deadline
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_secs(5)),
            )
            .json(&body)
            .send()
            .await
            .map_err(|_| Gap::Unavailable)?;
        if !response.status().is_success() {
            return Err(Gap::Unavailable);
        }
        if response
            .content_length()
            .is_some_and(|size| size > MAX_BYTES as u64)
        {
            return Err(Gap::Oversized);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| Gap::Unavailable)? {
            if bytes.len() + chunk.len() > MAX_BYTES {
                return Err(Gap::Oversized);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }

    async fn list<T: serde::de::DeserializeOwned>(
        &mut self,
        method: &str,
        selector: Value,
        field: &str,
        cap: usize,
    ) -> Result<Vec<T>, Gap> {
        let mut entries = Vec::new();
        let mut page = String::new();
        let mut seen = BTreeSet::new();
        for _ in 0..4 {
            let mut request = json!({"selector":selector,"page_token":page});
            if method == "GetInvocation" {
                request["include_child_invocations"] = json!(true);
                request["include_metadata"] = json!(true);
            }
            let bytes = self.call(method, request).await?;
            let mut value: Value =
                serde_json::from_slice(&bytes).map_err(|_| Gap::MalformedResponse)?;
            let batch: Vec<T> =
                serde_json::from_value(value.get_mut(field).map(Value::take).unwrap_or(json!([])))
                    .map_err(|_| Gap::MalformedResponse)?;
            if entries.len() + batch.len() > cap {
                return Err(Gap::RetrievalLimit);
            }
            entries.extend(batch);
            page = value
                .get("nextPageToken")
                .or_else(|| value.get("next_page_token"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            if page.is_empty() {
                return Ok(entries);
            }
            if page.len() > 4096 || !seen.insert(page.clone()) {
                return Err(Gap::RetrievalLimit);
            }
        }
        Err(Gap::RetrievalLimit)
    }

    async fn invocation(&mut self, id: &str, repo: &str, sha: &str) -> Result<Invocation, Gap> {
        if !identifier(id) {
            return Err(Gap::WrongInvocation);
        }
        let mut found: Vec<Invocation> = self
            .list(
                "GetInvocation",
                json!({"invocation_id":id}),
                "invocation",
                1,
            )
            .await?;
        let invocation = found.pop().ok_or(Gap::MissingInvocation)?;
        if invocation.id.invocation_id != id {
            return Err(Gap::WrongInvocation);
        }
        if !invocation
            .repo_url
            .trim_end_matches('/')
            .trim_end_matches(".git")
            .eq_ignore_ascii_case(&format!("https://github.com/{repo}"))
        {
            return Err(Gap::WrongRepository);
        }
        if invocation.commit_sha != sha {
            return Err(Gap::StaleSha);
        }
        if invocation.invocation_status != "COMPLETE_INVOCATION_STATUS" {
            return Err(Gap::IncompleteInvocation);
        }
        Ok(invocation)
    }

    async fn collect(
        &mut self,
        repo: &str,
        sha: &str,
        parent: &str,
        expected: &BTreeSet<String>,
    ) -> Result<(), Gap> {
        let invocation = self.invocation(parent, repo, sha).await?;
        if invocation.child_invocations.is_empty() || invocation.child_invocations.len() > 8 {
            return Err(Gap::MissingInvocation);
        }
        let mut failed = BTreeSet::new();
        let mut children = BTreeSet::new();
        for child in invocation.child_invocations {
            if !children.insert(child.invocation_id.clone()) {
                return Err(Gap::WrongInvocation);
            }
            let child = self.invocation(&child.invocation_id, repo, sha).await?;
            if child.command != "test" {
                continue;
            }
            for (key, expected) in [
                ("TL3_CI_RUN_ID", self.binding.run_id.to_string()),
                ("TL3_CI_RUN_ATTEMPT", self.binding.run_attempt.to_string()),
                ("TL3_CI_CHECK_NAME", self.binding.name.clone()),
            ] {
                let mut values = child.build_metadata.iter().filter(|entry| entry.key == key);
                if values.next().map(|entry| &entry.value) != Some(&expected)
                    || values.next().is_some()
                {
                    return Err(Gap::WrongCiRun);
                }
            }
            let id = &child.id.invocation_id;
            // Read the complete inventory, so a truncated job log cannot hide another failed target.
            let targets: Vec<Target> = self
                .list("GetTarget", json!({"invocation_id":id}), "target", 2000)
                .await?;
            for target in targets {
                if target.id.invocation_id != *id || !label(&target.label) {
                    return Err(Gap::WrongTarget);
                }
                match target.status.as_str() {
                    "PASSED" | "SKIPPED" | "BUILT" | "FLAKY" => continue,
                    "FAILED" => {}
                    _ => return Err(Gap::IncompleteInvocation),
                }
                failed.insert(target.label.clone());
                let before = self.results.len();
                let actions: Vec<Action> = self
                    .list(
                        "GetAction",
                        json!({"invocation_id":id,"target_label":target.label,
                    "target_id":target.id.target_id}),
                        "action",
                        MAX_ACTIONS + 1,
                    )
                    .await?;
                if actions.is_empty() {
                    return Err(Gap::MissingArtifacts);
                }
                let mut seen = BTreeSet::new();
                for action in actions {
                    if action.id.invocation_id != *id
                        || action.id.target_id != target.id.target_id
                        || action.target_label != target.label
                    {
                        return Err(Gap::WrongTarget);
                    }
                    // GetAction also returns the target's build output. Run and attempt
                    // are populated only for tests; build output has no test artifacts.
                    if action.run == 0
                        && action.attempt == 0
                        && !action
                            .file
                            .iter()
                            .any(|file| matches!(file.name.as_str(), "test.xml" | "test.log"))
                    {
                        continue;
                    }
                    if !identifier(&action.id.action_id)
                        || !identifier(&action.id.configuration_id)
                        || !seen.insert((
                            action.id.configuration_id,
                            action.shard,
                            action.run,
                            action.attempt,
                        ))
                    {
                        return Err(Gap::MalformedResponse);
                    }
                    if self.results.len() >= MAX_ACTIONS {
                        return Err(Gap::RetrievalLimit);
                    }
                    let xml = self.file(&action.file, "test.xml").await?;
                    let log = self.file(&action.file, "test.log").await?;
                    let mut nodes = nodes(&xml)?;
                    for node in &mut nodes {
                        node.test = self.client.remove_key(&node.test);
                        node.message = self.client.remove_key(&node.message);
                    }
                    self.results.push(CiTargetResult {
                        repository: repo.into(),
                        check: self.check,
                        ci_run: self.binding.run_id,
                        ci_run_attempt: self.binding.run_attempt,
                        check_name: self.binding.name.clone(),
                        sha: sha.into(),
                        parent_invocation: parent.into(),
                        invocation: id.clone(),
                        target: target.label.clone(),
                        shard: action.shard,
                        run: action.run,
                        attempt: action.attempt,
                        nodes,
                        excerpt: self.client.remove_key(&excerpt(&log)),
                    });
                }
                if !self.results[before..]
                    .iter()
                    .flat_map(|result| &result.nodes)
                    .any(|node| {
                        matches!(node.status, CiNodeStatus::Failed | CiNodeStatus::Errored)
                            && !node.message.trim().is_empty()
                    })
                {
                    return Err(Gap::MissingDiagnostic);
                }
            }
        }
        if failed.is_empty() || !expected.is_subset(&failed) {
            return Err(Gap::MissingTarget);
        }
        Ok(())
    }

    async fn file(&mut self, files: &[File], name: &str) -> Result<String, Gap> {
        let mut found = files.iter().filter(|file| file.name == name);
        let file = found.next().ok_or(Gap::MissingArtifacts)?;
        if found.next().is_some() || file.uri.len() > 4096 || !file.uri.starts_with("bytestream://")
        {
            return Err(Gap::MissingArtifacts);
        }
        let bytes = self.call("GetFile", json!({"uri":file.uri})).await?;
        String::from_utf8(bytes).map_err(|_| Gap::MalformedResponse)
    }
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || b"-_.".contains(&ch))
}

fn label(value: &str) -> bool {
    value.starts_with("//")
        && value.contains(':')
        && value.len() <= 1024
        && value
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || b"_-/.:+@=".contains(&ch))
}

fn excerpt(log: &str) -> String {
    let lines: Vec<_> = log.lines().collect();
    let mut selected = Vec::new();
    let mut until = 0;
    for (index, line) in lines.iter().enumerate() {
        if [
            "FAIL",
            "ERROR",
            "Traceback",
            "Error",
            "assert ",
            "SKIP",
            "prerequisite",
        ]
        .iter()
        .any(|word| line.contains(word))
        {
            until = index + 7;
        }
        if index < until || index < 3 {
            selected.push(*line);
            if selected.iter().map(|line| line.len() + 1).sum::<usize>() >= 4000 {
                break;
            }
        }
    }
    selected.join("\n")
}

fn nodes(xml: &str) -> Result<Vec<CiTestNode>, Gap> {
    use quick_xml::{Reader, events::Event};
    let mut reader = Reader::from_str(xml);
    let mut stack = Vec::new();
    let mut nodes = Vec::new();
    let mut current: Option<CiTestNode> = None;
    let mut root = false;
    let mut suites = Vec::new();
    loop {
        let event = reader.read_event().map_err(|_| Gap::MalformedXml)?;
        let empty = matches!(event, Event::Empty(_));
        match event {
            Event::Start(event) | Event::Empty(event) => {
                let name = event.name().0.to_owned();
                if stack.is_empty() {
                    if root || !matches!(name.as_str(), "testsuite" | "testsuites") {
                        return Err(Gap::MalformedXml);
                    }
                    root = true;
                }
                if stack.len() >= 32 {
                    return Err(Gap::MalformedXml);
                }
                let attrs = event
                    .attributes()
                    .map(|attr| {
                        let attr = attr.map_err(|_| Gap::MalformedXml)?;
                        Ok((
                            attr.key.0.to_owned(),
                            attr.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                                .map_err(|_| Gap::MalformedXml)?
                                .into_owned(),
                        ))
                    })
                    .collect::<Result<std::collections::BTreeMap<_, _>, Gap>>()?;
                if matches!(name.as_str(), "testsuite" | "testsuites") {
                    suites.push(SuiteCounts::read(&attrs, nodes.len())?);
                }
                if name == "testcase" {
                    if stack.last().map(String::as_str) != Some("testsuite")
                        || current.is_some()
                        || nodes.len() >= MAX_NODES
                    {
                        return Err(Gap::MalformedXml);
                    }
                    let test = attrs
                        .get("name")
                        .filter(|name| !name.is_empty())
                        .ok_or(Gap::MalformedXml)?;
                    let class = attrs
                        .get("classname")
                        .map(String::as_str)
                        .unwrap_or_default();
                    let mut classes = String::new();
                    let path = attrs.get("file").cloned().unwrap_or_else(|| {
                        if class.starts_with("tests.") {
                            let parts: Vec<_> = class.split('.').collect();
                            let end = parts
                                .iter()
                                .position(|part| part.starts_with(char::is_uppercase))
                                .unwrap_or(parts.len());
                            if end < parts.len() {
                                classes = format!("{}::", parts[end..].join("::"));
                            }
                            format!("{}.py", parts[..end].join("/"))
                        } else {
                            class.into()
                        }
                    });
                    current = Some(CiTestNode {
                        test: format!("{path}::{classes}{test}"),
                        status: CiNodeStatus::Passed,
                        message: String::new(),
                    });
                } else if let Some(node) = current.as_mut() {
                    let status = match name.as_str() {
                        "failure" => Some(CiNodeStatus::Failed),
                        "error" => Some(CiNodeStatus::Errored),
                        "skipped" => Some(CiNodeStatus::Skipped),
                        _ => None,
                    };
                    if let Some(status) = status {
                        if node.status != CiNodeStatus::Passed {
                            return Err(Gap::MalformedXml);
                        }
                        node.status = status;
                        node.message = attrs.get("message").cloned().unwrap_or_default();
                    }
                }
                if empty {
                    if matches!(name.as_str(), "testsuite" | "testsuites") {
                        suites.pop().ok_or(Gap::MalformedXml)?.check(&nodes)?;
                    }
                    if name == "testcase" {
                        nodes.push(current.take().ok_or(Gap::MalformedXml)?);
                    }
                } else {
                    stack.push(name);
                }
            }
            Event::End(event) => {
                let name = event.name().0;
                if stack.pop().as_deref() != Some(name) {
                    return Err(Gap::MalformedXml);
                }
                if matches!(name, "testsuite" | "testsuites") {
                    suites.pop().ok_or(Gap::MalformedXml)?.check(&nodes)?;
                }
                if name == "testcase" {
                    nodes.push(current.take().ok_or(Gap::MalformedXml)?);
                }
            }
            Event::Text(text) => {
                if let Some(node) = current.as_mut() {
                    node.message.push_str(&text.xml10_content());
                }
            }
            Event::CData(text) => {
                if let Some(node) = current.as_mut() {
                    node.message.push_str(&text.xml10_content());
                }
            }
            Event::GeneralRef(text) => {
                if let Some(node) = current.as_mut() {
                    let reference = format!("&{};", &*text);
                    node.message.push_str(
                        &quick_xml::escape::unescape(&reference).map_err(|_| Gap::MalformedXml)?,
                    );
                }
            }
            Event::DocType(_) => return Err(Gap::MalformedXml),
            Event::Eof => break,
            _ => {}
        }
    }
    if !root || !stack.is_empty() || current.is_some() || nodes.is_empty() {
        return Err(Gap::MalformedXml);
    }
    Ok(nodes)
}

struct SuiteCounts {
    start: usize,
    tests: Option<usize>,
    failures: Option<usize>,
    errors: Option<usize>,
    skipped: Option<usize>,
}

impl SuiteCounts {
    fn read(attrs: &std::collections::BTreeMap<String, String>, start: usize) -> Result<Self, Gap> {
        let count = |key| {
            attrs
                .get(key)
                .map(|value| value.parse().map_err(|_| Gap::MalformedXml))
                .transpose()
        };
        Ok(Self {
            start,
            tests: count("tests")?,
            failures: count("failures")?,
            errors: count("errors")?,
            skipped: count("skipped")?,
        })
    }
    fn check(self, nodes: &[CiTestNode]) -> Result<(), Gap> {
        let nodes = &nodes[self.start..];
        for (expected, actual) in [
            (self.tests, nodes.len()),
            (
                self.failures,
                nodes
                    .iter()
                    .filter(|node| node.status == CiNodeStatus::Failed)
                    .count(),
            ),
            (
                self.errors,
                nodes
                    .iter()
                    .filter(|node| node.status == CiNodeStatus::Errored)
                    .count(),
            ),
            (
                self.skipped,
                nodes
                    .iter()
                    .filter(|node| node.status == CiNodeStatus::Skipped)
                    .count(),
            ),
        ] {
            if expected.is_some_and(|count| count != actual) {
                return Err(Gap::MalformedXml);
            }
        }
        Ok(())
    }
}
