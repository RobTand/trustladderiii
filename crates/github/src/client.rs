use crate::models::*;
use chrono::{DateTime, Timelike, Utc};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use reqwest::{Method, StatusCode, Url, header::HeaderMap};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{collections::BTreeSet, sync::Arc, time::Duration};
use tokio::{sync::Mutex, time::Instant};

pub const API_VERSION: &str = "2022-11-28";

/// Matches deploy/github-app/README.md. Deliberately has no Debug or Serialize.
#[derive(Deserialize)]
pub struct AppConfig {
    pub id: u64,
    pub slug: String,
    pub pem: String,
    pub webhook_secret: String,
}

#[derive(Clone, Debug)]
pub struct ClientOptions {
    pub api_url: Url,
    pub max_retries: u32,
    pub max_wait: Duration,
    pub request_timeout: Duration,
    pub mutation_interval: Duration,
}

impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            api_url: Url::parse("https://api.github.com/").unwrap(),
            max_retries: 3,
            max_wait: Duration::from_secs(60),
            request_timeout: Duration::from_secs(30),
            mutation_interval: Duration::from_secs(1),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid configuration: {0}")]
    Config(&'static str),
    #[error("could not sign app JWT")]
    Jwt,
    #[error("GitHub transport failed; reconcile writes before retrying")]
    Transport,
    #[error("GitHub returned HTTP {status}")]
    Http { status: u16 },
    #[error("GitHub rate limited requests; retry after {retry_after:?}")]
    RateLimited { retry_after: Duration },
    #[error("GitHub response did not match its schema")]
    Response,
}

struct Token {
    value: String,
    expires_at: DateTime<Utc>,
}

struct Inner {
    app_id: u64,
    slug: String,
    key: EncodingKey,
    installation: u64,
    http: reqwest::Client,
    options: ClientOptions,
    token: Mutex<Option<Token>>,
    requests: Mutex<Throttle>,
    comments: Mutex<()>,
}

struct Throttle {
    blocked_until: Instant,
    next_mutation: Instant,
}

#[derive(Clone)]
pub struct GitHub(Arc<Inner>);

impl GitHub {
    pub fn app_slug(&self) -> &str {
        &self.0.slug
    }

    pub fn new(
        config: AppConfig,
        installation: u64,
        options: ClientOptions,
    ) -> Result<Self, Error> {
        let url = &options.api_url;
        let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
        if (url.scheme() != "https" && !(url.scheme() == "http" && loopback))
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || !url.path().ends_with('/')
        {
            return Err(Error::Config(
                "API URL must be HTTPS with a trailing slash, or loopback HTTP",
            ));
        }
        if config.id == 0 || installation == 0 || config.slug.is_empty() {
            return Err(Error::Config(
                "app id, slug and installation id are required",
            ));
        }
        let key = EncodingKey::from_rsa_pem(config.pem.as_bytes())
            .map_err(|_| Error::Config("invalid RSA PEM"))?;
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(options.request_timeout)
            .build()
            .map_err(|_| Error::Config("could not build HTTP client"))?;
        Ok(Self(Arc::new(Inner {
            app_id: config.id,
            slug: config.slug,
            key,
            installation,
            http,
            options,
            token: Mutex::new(None),
            requests: Mutex::new(Throttle {
                blocked_until: Instant::now(),
                next_mutation: Instant::now(),
            }),
            comments: Mutex::new(()),
        })))
    }

    pub fn installation_id(&self) -> u64 {
        self.0.installation
    }

    pub fn app_id(&self) -> u64 {
        self.0.app_id
    }

    pub fn app_jwt(&self, now: DateTime<Utc>) -> Result<String, Error> {
        #[derive(Serialize)]
        struct Claims {
            iat: i64,
            exp: i64,
            iss: String,
        }
        encode(
            &Header::new(Algorithm::RS256),
            &Claims {
                iat: now.timestamp() - 60,
                exp: now.timestamp() + 540,
                iss: self.0.app_id.to_string(),
            },
            &self.0.key,
        )
        .map_err(|_| Error::Jwt)
    }

    /// Installation credential for GitHub HTTPS Git operations. Never log or persist it.
    pub async fn installation_token(&self) -> Result<String, Error> {
        let mut cache = self.0.token.lock().await;
        if let Some(token) = cache.as_ref()
            && token.expires_at > Utc::now() + chrono::Duration::seconds(60)
        {
            return Ok(token.value.clone());
        }
        #[derive(Deserialize)]
        struct TokenResponse {
            token: String,
            expires_at: DateTime<Utc>,
        }
        let jwt = self.app_jwt(Utc::now())?;
        let path = format!("app/installations/{}/access_tokens", self.0.installation);
        // Issuing another token is safe. Creating PRs, comments and check runs is not.
        let value = self
            .send(Method::POST, &path, &[], Some(json!({})), &jwt, true)
            .await?;
        let token: TokenResponse = serde_json::from_value(value).map_err(|_| Error::Response)?;
        if token.token.is_empty() || token.expires_at <= Utc::now() {
            return Err(Error::Response);
        }
        *cache = Some(Token {
            value: token.token.clone(),
            expires_at: token.expires_at,
        });
        Ok(token.token)
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        query: &[(String, String)],
        body: Option<Value>,
        token: &str,
        retry_safe: bool,
    ) -> Result<Value, Error> {
        let mut url = self.0.options.api_url.clone();
        // Segments are already encoded by endpoint builders. Never follow an upstream URL.
        url.set_path(&format!("{}{}", url.path(), path));
        if !query.is_empty() {
            url.query_pairs_mut()
                .extend_pairs(query.iter().map(|(k, v)| (k, v)));
        }
        let mut throttle = self.0.requests.lock().await;
        for attempt in 0..=self.0.options.max_retries {
            let mut until = throttle.blocked_until;
            if method != Method::GET {
                until = until.max(throttle.next_mutation);
            }
            let wait = until.saturating_duration_since(Instant::now());
            if wait > self.0.options.max_wait {
                return Err(Error::RateLimited { retry_after: wait });
            }
            tokio::time::sleep(wait).await;
            let mut request = self
                .0
                .http
                .request(method.clone(), url.clone())
                .header("Accept", "application/vnd.github+json")
                .header("X-GitHub-Api-Version", API_VERSION)
                .header("User-Agent", "tl3-github")
                .bearer_auth(token);
            if let Some(body) = &body {
                request = request.json(body);
            }
            let response = request.send().await;
            if method != Method::GET {
                throttle.next_mutation = Instant::now() + self.0.options.mutation_interval;
            }
            let response = match response {
                Ok(response) => response,
                Err(_) if retry_safe && attempt < self.0.options.max_retries => {
                    throttle.blocked_until =
                        Instant::now() + Duration::from_secs(1 << attempt.min(6));
                    continue;
                }
                Err(_) => return Err(Error::Transport),
            };
            let status = response.status();
            let headers = response.headers().clone();
            let remaining_zero = headers
                .get("x-ratelimit-remaining")
                .and_then(|h| h.to_str().ok())
                == Some("0");
            let bytes = response.bytes().await.map_err(|_| Error::Transport)?;
            let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            let secondary = value
                .get("message")
                .and_then(Value::as_str)
                .is_some_and(|m| {
                    let m = m.to_ascii_lowercase();
                    m.contains("secondary rate limit") || m.contains("abuse detection")
                });
            let rate_limited = status == StatusCode::TOO_MANY_REQUESTS
                || (status == StatusCode::FORBIDDEN
                    && (remaining_zero || headers.contains_key("retry-after") || secondary));
            if remaining_zero || rate_limited {
                let delay = rate_delay(&headers, attempt);
                throttle.blocked_until =
                    Instant::now().checked_add(delay).ok_or(Error::Response)?;
                if rate_limited {
                    if retry_safe
                        && attempt < self.0.options.max_retries
                        && delay <= self.0.options.max_wait
                    {
                        continue;
                    }
                    return Err(Error::RateLimited { retry_after: delay });
                }
            }
            if status.is_success() {
                if !bytes.is_empty() && value.is_null() {
                    return Err(Error::Response);
                }
                return Ok(value);
            }
            if retry_safe
                && matches!(status.as_u16(), 500 | 502 | 503 | 504)
                && attempt < self.0.options.max_retries
            {
                throttle.blocked_until = Instant::now() + Duration::from_secs(1 << attempt.min(6));
                continue;
            }
            return Err(Error::Http {
                status: status.as_u16(),
            });
        }
        unreachable!()
    }

    async fn request<T: DeserializeOwned>(
        &self,
        method: Method,
        path: String,
        query: Vec<(String, String)>,
        body: Option<Value>,
    ) -> Result<T, Error> {
        let token = self.installation_token().await?;
        let safe = method == Method::GET || method == Method::PATCH || method == Method::DELETE;
        let result = self.send(method, &path, &query, body, &token, safe).await;
        if matches!(result, Err(Error::Http { status: 401 })) {
            let mut cached = self.0.token.lock().await;
            if cached.as_ref().is_some_and(|t| t.value == token) {
                *cached = None;
            }
        }
        serde_json::from_value(result?).map_err(|_| Error::Response)
    }

    async fn list<T: DeserializeOwned>(
        &self,
        path: String,
        query: Vec<(String, String)>,
        field: Option<&str>,
    ) -> Result<Vec<T>, Error> {
        let mut all = Vec::new();
        for page in 1.. {
            let mut query = query.clone();
            query.extend([
                ("per_page".into(), "100".into()),
                ("page".into(), page.to_string()),
            ]);
            let value: Value = self.request(Method::GET, path.clone(), query, None).await?;
            let rows = if let Some(field) = field {
                value.get(field).cloned().ok_or(Error::Response)?
            } else {
                value
            };
            let rows: Vec<T> = serde_json::from_value(rows).map_err(|_| Error::Response)?;
            let done = rows.len() < 100;
            all.extend(rows);
            if done {
                return Ok(all);
            }
        }
        unreachable!()
    }

    pub async fn issue(&self, repo: &str, number: u64) -> Result<Issue, Error> {
        self.request(
            Method::GET,
            format!("{}/issues/{number}", repo_path(repo)?),
            vec![],
            None,
        )
        .await
    }

    pub async fn reconcile_issue(
        &self,
        repo: &str,
        effect_key: &str,
        since: &str,
    ) -> Result<Option<Issue>, Error> {
        let marker = Self::marker(effect_key);
        let since = DateTime::parse_from_rfc3339(since)
            .map_err(|_| Error::Config("invalid request filing timestamp"))?
            - chrono::Duration::seconds(1);
        let issues: Vec<Value> = self
            .list(
                format!("{}/issues", repo_path(repo)?),
                vec![
                    ("state".into(), "all".into()),
                    ("since".into(), since.to_rfc3339()),
                    ("sort".into(), "created".into()),
                    ("direction".into(), "desc".into()),
                ],
                None,
            )
            .await?;
        let found = issues
            .into_iter()
            .filter(|issue| {
                issue.get("pull_request").is_none()
                    && issue["user"]["login"] == format!("{}[bot]", self.0.slug)
                    && issue["body"]
                        .as_str()
                        .is_some_and(|body| body.lines().any(|line| line == marker))
            })
            .min_by_key(|issue| issue["number"].as_u64());
        found
            .map(serde_json::from_value)
            .transpose()
            .map_err(|_| Error::Response)
    }

    /// A lost POST response is reconciled by marker before another create.
    pub async fn create_request_issue(
        &self,
        repo: &str,
        effect_key: &str,
        since: &str,
        title: &str,
        body: &str,
    ) -> Result<Issue, Error> {
        let body = marked_body(effect_key, body)?;
        let _guard = self.0.comments.lock().await;
        if let Some(issue) = self.reconcile_issue(repo, effect_key, since).await? {
            return Ok(issue);
        }
        self.request(
            Method::POST,
            format!("{}/issues", repo_path(repo)?),
            vec![],
            Some(json!({"title": title, "body": body, "labels": ["tl3"]})),
        )
        .await
    }

    /// GitHub leaves an existing label unchanged.
    pub async fn add_label(
        &self,
        repo: &str,
        number: u64,
        label: &str,
    ) -> Result<Vec<Label>, Error> {
        self.request(
            Method::POST,
            format!("{}/issues/{number}/labels", repo_path(repo)?),
            vec![],
            Some(json!({"labels": [label]})),
        )
        .await
    }

    pub async fn remove_label(&self, repo: &str, number: u64, label: &str) -> Result<(), Error> {
        match self
            .request::<Value>(
                Method::DELETE,
                format!(
                    "{}/issues/{number}/labels/{}",
                    repo_path(repo)?,
                    segment(label)?
                ),
                vec![],
                None,
            )
            .await
        {
            Ok(_) | Err(Error::Http { status: 404 }) => Ok(()),
            Err(e) => Err(e),
        }
    }

    pub async fn comments(&self, repo: &str, number: u64) -> Result<Vec<Comment>, Error> {
        self.list(
            format!("{}/issues/{number}/comments", repo_path(repo)?),
            vec![],
            None,
        )
        .await
    }

    pub fn marker(effect_key: &str) -> String {
        format!("<!-- tl3:effect:{} -->", hex::encode(effect_key.as_bytes()))
    }

    fn owns_comment(&self, comment: &Comment) -> bool {
        comment.user.login == format!("{}[bot]", self.0.slug)
    }

    /// Serializes posts across clones. Cross-process races converge via reconciliation.
    pub async fn post_comment(
        &self,
        repo: &str,
        number: u64,
        effect_key: &str,
        body: &str,
    ) -> Result<Comment, Error> {
        let body = marked_body(effect_key, body)?;
        let _guard = self.0.comments.lock().await;
        if let Some(comment) = self.reconcile_comments(repo, number, effect_key).await? {
            return Ok(comment);
        }
        self.request(
            Method::POST,
            format!("{}/issues/{number}/comments", repo_path(repo)?),
            vec![],
            Some(json!({"body": body})),
        )
        .await
    }

    pub async fn reconcile_comments(
        &self,
        repo: &str,
        number: u64,
        effect_key: &str,
    ) -> Result<Option<Comment>, Error> {
        let marker = Self::marker(effect_key);
        let mut matches: Vec<_> = self
            .comments(repo, number)
            .await?
            .into_iter()
            .filter(|c| self.owns_comment(c) && c.body.lines().any(|line| line == marker))
            .collect();
        matches.sort_by_key(|c| c.id);
        for duplicate in matches.iter().skip(1) {
            self.delete_comment(repo, duplicate.id).await?;
        }
        Ok(matches.into_iter().next())
    }

    pub async fn edit_comment(
        &self,
        repo: &str,
        id: u64,
        effect_key: &str,
        body: &str,
    ) -> Result<Comment, Error> {
        let body = marked_body(effect_key, body)?;
        self.request(
            Method::PATCH,
            format!("{}/issues/comments/{id}", repo_path(repo)?),
            vec![],
            Some(json!({"body": body})),
        )
        .await
    }

    pub async fn delete_comment(&self, repo: &str, id: u64) -> Result<(), Error> {
        match self
            .request::<Value>(
                Method::DELETE,
                format!("{}/issues/comments/{id}", repo_path(repo)?),
                vec![],
                None,
            )
            .await
        {
            Ok(_) | Err(Error::Http { status: 404 }) => Ok(()),
            Err(e) => Err(e),
        }
    }

    pub async fn close_issue(
        &self,
        repo: &str,
        number: u64,
        reason: CloseReason,
    ) -> Result<Issue, Error> {
        self.request(
            Method::PATCH,
            format!("{}/issues/{number}", repo_path(repo)?),
            vec![],
            Some(json!({"state":"closed","state_reason":reason})),
        )
        .await
    }

    pub async fn create_check_run(
        &self,
        repo: &str,
        check: &NewCheckRun,
    ) -> Result<CheckRun, Error> {
        if check.external_id.trim().is_empty()
            || matches!(check.status, CheckStatus::Completed) != check.conclusion.is_some()
        {
            return Err(Error::Config(
                "check needs an external id and a conclusion exactly when completed",
            ));
        }
        if let Some(existing) = self
            .check_runs(repo, &check.head_sha)
            .await?
            .into_iter()
            .find(|r| {
                r.external_id.as_deref() == Some(&check.external_id)
                    && r.app.as_ref().is_some_and(|app| app.id == self.0.app_id)
            })
        {
            return Ok(existing);
        }
        self.request(
            Method::POST,
            format!("{}/check-runs", repo_path(repo)?),
            vec![],
            Some(serde_json::to_value(check).map_err(|_| Error::Response)?),
        )
        .await
    }

    pub async fn check_runs(&self, repo: &str, head: &str) -> Result<Vec<CheckRun>, Error> {
        self.list(
            format!("{}/commits/{}/check-runs", repo_path(repo)?, segment(head)?),
            vec![("filter".into(), "all".into())],
            Some("check_runs"),
        )
        .await
    }

    pub async fn check_suites(&self, repo: &str, head: &str) -> Result<Vec<CheckSuite>, Error> {
        self.list(
            format!(
                "{}/commits/{}/check-suites",
                repo_path(repo)?,
                segment(head)?
            ),
            vec![],
            Some("check_suites"),
        )
        .await
    }

    pub async fn pull_request(&self, repo: &str, number: u64) -> Result<PullRequest, Error> {
        self.request(
            Method::GET,
            format!("{}/pulls/{number}", repo_path(repo)?),
            vec![],
            None,
        )
        .await
    }

    pub async fn pull_requests_for_branch(
        &self,
        repo: &str,
        owner: &str,
        branch: &str,
    ) -> Result<Vec<PullRequest>, Error> {
        self.list(
            format!("{}/pulls", repo_path(repo)?),
            vec![
                ("state".into(), "all".into()),
                ("head".into(), format!("{owner}:{branch}")),
            ],
            None,
        )
        .await
    }

    pub async fn open_pull_request(
        &self,
        repo: &str,
        pull: &NewPullRequest,
    ) -> Result<PullRequest, Error> {
        self.request(
            Method::POST,
            format!("{}/pulls", repo_path(repo)?),
            vec![],
            Some(serde_json::to_value(pull).map_err(|_| Error::Response)?),
        )
        .await
    }

    pub async fn close_pull_request(&self, repo: &str, number: u64) -> Result<PullRequest, Error> {
        self.request(
            Method::PATCH,
            format!("{}/pulls/{number}", repo_path(repo)?),
            vec![],
            Some(json!({"state":"closed"})),
        )
        .await
    }

    pub async fn reopen_pull_request(&self, repo: &str, number: u64) -> Result<PullRequest, Error> {
        self.request(
            Method::PATCH,
            format!("{}/pulls/{number}", repo_path(repo)?),
            vec![],
            Some(json!({"state":"open"})),
        )
        .await
    }

    pub async fn branch(&self, repo: &str, branch: &str) -> Result<Option<Branch>, Error> {
        match self
            .request(
                Method::GET,
                format!("{}/branches/{}", repo_path(repo)?, segment(branch)?),
                vec![],
                None,
            )
            .await
        {
            Ok(branch) => Ok(Some(branch)),
            Err(Error::Http { status: 404 }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub async fn delete_branch(&self, repo: &str, branch: &str) -> Result<(), Error> {
        match self
            .request::<Value>(
                Method::DELETE,
                format!("{}/git/refs/heads/{}", repo_path(repo)?, segment(branch)?),
                vec![],
                None,
            )
            .await
        {
            Ok(_) | Err(Error::Http { status: 404 }) => Ok(()),
            Err(Error::Http { status: 422 }) if self.branch(repo, branch).await?.is_none() => {
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    pub async fn issue_events(&self, repo: &str, number: u64) -> Result<Vec<IssueActivity>, Error> {
        self.list(
            format!("{}/issues/{number}/events", repo_path(repo)?),
            vec![],
            None,
        )
        .await
    }

    pub async fn catch_up(
        &self,
        served: &[ServedRepo],
        since: DateTime<Utc>,
    ) -> Result<CatchUp, Error> {
        let started_at = Utc::now();
        let next_cursor = started_at.with_nanosecond(0).ok_or(Error::Response)?;
        if since > started_at {
            return Err(Error::Config("catch-up cursor is in the future"));
        }
        let lookback = since
            .checked_sub_signed(chrono::Duration::seconds(1))
            .ok_or(Error::Config("catch-up cursor is out of range"))?;
        let mut repositories = Vec::new();
        for repo in served {
            let path = repo_path(&repo.full_name)?;
            let query = vec![("since".into(), lookback.to_rfc3339())];
            let mut issues_query = query.clone();
            issues_query.push(("state".into(), "all".into()));
            let issues: Vec<Issue> = self
                .list(format!("{path}/issues"), issues_query, None)
                .await?;
            let comments = self
                .list(format!("{path}/issues/comments"), query, None)
                .await?;
            let mut labels = Vec::new();
            let mut closed = Vec::new();
            let mut pull_requests = Vec::new();
            for page in 1.. {
                let pulls: Vec<PullRequest> = self
                    .request(
                        Method::GET,
                        format!("{path}/pulls"),
                        vec![
                            ("state".into(), "all".into()),
                            ("sort".into(), "updated".into()),
                            ("direction".into(), "desc".into()),
                            ("per_page".into(), "100".into()),
                            ("page".into(), page.to_string()),
                        ],
                        None,
                    )
                    .await?;
                let done = pulls.len() < 100 || pulls.iter().any(|pull| pull.updated_at < since);
                pull_requests.extend(
                    pulls
                        .into_iter()
                        .take_while(|pull| pull.updated_at >= since),
                );
                if done {
                    break;
                }
            }
            let mut heads: BTreeSet<_> = repo.tracked_heads.iter().cloned().collect();
            heads.extend(pull_requests.iter().map(|pull| pull.head.sha.clone()));
            // GitHub documents no ordering here. Stop early only while ids strictly descend.
            let mut descending = true;
            let mut previous_id = None;
            for page in 1.. {
                let events: Vec<Value> = self
                    .request(
                        Method::GET,
                        format!("{path}/issues/events"),
                        vec![
                            ("per_page".into(), "100".into()),
                            ("page".into(), page.to_string()),
                        ],
                        None,
                    )
                    .await?;
                let short_page = events.len() < 100;
                let mut all_older = true;
                for event in events {
                    let id = event.get("id").and_then(Value::as_u64);
                    descending &= id.is_some()
                        && previous_id
                            .zip(id)
                            .is_none_or(|(previous, id)| previous > id);
                    previous_id = id;
                    all_older &= event
                        .get("created_at")
                        .and_then(Value::as_str)
                        .and_then(|value| value.parse::<DateTime<Utc>>().ok())
                        .is_some_and(|created_at| created_at < since);
                    if event.get("event").and_then(Value::as_str) == Some("closed") {
                        if id.is_none() {
                            continue;
                        }
                        let Some(number) = event["issue"]["number"].as_u64() else {
                            continue;
                        };
                        let activity: PullClosed =
                            serde_json::from_value(event.clone()).map_err(|_| Error::Response)?;
                        if activity.created_at >= since {
                            closed.push((number, activity));
                        }
                    }
                    if matches!(
                        event.get("event").and_then(Value::as_str),
                        Some("labeled" | "unlabeled")
                    ) {
                        let number = event
                            .get("issue")
                            .and_then(|issue| issue.get("number"))
                            .and_then(Value::as_u64)
                            .ok_or(Error::Response)?;
                        if event["actor"].is_null() {
                            continue;
                        }
                        let event: LabelChange =
                            serde_json::from_value(event).map_err(|_| Error::Response)?;
                        if event.created_at >= since {
                            labels.push((number, event));
                        }
                    }
                }
                if short_page || (descending && all_older) {
                    break;
                }
            }
            let mut check_runs = Vec::new();
            let mut check_suites = Vec::new();
            // Check completions do not necessarily update an issue's updated_at.
            for head in heads {
                check_runs.extend(self.check_runs(&repo.full_name, &head).await?);
                check_suites.extend(self.check_suites(&repo.full_name, &head).await?);
            }
            repositories.push(RepoChanges {
                repository: repo.full_name.clone(),
                issues,
                comments,
                labels,
                closed,
                pull_requests,
                check_runs,
                check_suites,
            });
        }
        Ok(CatchUp {
            next_cursor,
            repositories,
        })
    }
}

fn marked_body(effect_key: &str, body: &str) -> Result<String, Error> {
    if effect_key.is_empty() || body.contains("<!-- tl3:effect:") {
        return Err(Error::Config(
            "comment needs a nonempty effect key and no embedded markers",
        ));
    }
    // A dangling Markdown code block or HTML comment could expose or swallow the marker.
    Ok(format!("{}\n\n{body}", GitHub::marker(effect_key)))
}

fn repo_path(repo: &str) -> Result<String, Error> {
    let parts: Vec<_> = repo.split('/').collect();
    if parts.len() != 2 {
        return Err(Error::Config("repository must be owner/name"));
    }
    Ok(format!(
        "repos/{}/{}",
        segment(parts[0])?,
        segment(parts[1])?
    ))
}

fn segment(value: &str) -> Result<String, Error> {
    if value.is_empty() || value == "." || value == ".." {
        return Err(Error::Config("empty or relative path segment"));
    }
    let mut url = Url::parse("https://github.invalid/").unwrap();
    url.path_segments_mut().unwrap().push(value);
    Ok(url.path().trim_start_matches('/').to_owned())
}

fn rate_delay(headers: &HeaderMap, attempt: u32) -> Duration {
    let retry = headers
        .get("retry-after")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());
    let reset = headers
        .get("x-ratelimit-reset")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.parse::<i64>().ok())
        .map(|s| s.saturating_sub(Utc::now().timestamp()).max(1) as u64);
    Duration::from_secs(
        retry
            .into_iter()
            .chain(reset)
            .max()
            .unwrap_or(60 * (1 << attempt.min(6))),
    )
}
