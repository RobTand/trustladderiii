#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
};
use url::Url;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{field}: {message}")]
    Invalid { field: String, message: String },
    #[error("{field}: cannot inspect {path}: {source}")]
    File {
        field: String,
        path: PathBuf,
        source: std::io::Error,
    },
    // TOML errors can contain source lines. Do not retain or display those lines.
    #[error("config TOML: {0}")]
    Parse(String),
}

fn invalid(field: impl Into<String>, message: impl Into<String>) -> Error {
    Error::Invalid {
        field: field.into(),
        message: message.into(),
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub operations: Option<Operations>,
    #[serde(default)]
    pub store: Store,
    pub limits: Limits,
    pub github: GitHub,
    pub models: Models,
    pub worktrees_root: PathBuf,
    #[serde(default)]
    pub workers: Workers,
    #[serde(default)]
    pub planning: Planning,
    pub prismabuild: PrismaBuild,
    pub hindsight: Hindsight,
    pub status: Status,
    #[serde(default)]
    pub console: Option<Console>,
    pub allowances: Allowances,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Operations {
    pub owner: String,
    pub repo: String,
    pub issue: u64,
    #[serde(default)]
    pub delegated_attributions: Vec<String>,
}

/// The protected paths in force for one repository (#388): the slice-one
/// defaults, plus `planning.protected_paths`, plus the repository's own list.
/// The configured lists only add: nothing can remove a default.
pub fn protected_paths(global: &[String], repository: &[String]) -> Vec<String> {
    let mut paths = Planning::default().protected_paths;
    paths.extend_from_slice(global);
    paths.extend_from_slice(repository);
    paths.sort();
    paths.dedup();
    paths
}

/// Match repository-relative paths with the same glob separators as scope trimming.
pub fn path_is_protected(path: &str, patterns: &[String]) -> bool {
    let options = glob::MatchOptions {
        case_sensitive: true,
        require_literal_separator: true,
        require_literal_leading_dot: false,
    };
    patterns.iter().any(|pattern| {
        let pattern = glob::Pattern::new(pattern).expect("validated protected path pattern");
        pattern.matches_with(path, options)
            || (path.ends_with('/')
                && pattern.matches_with(&format!("{path}__tl3_path__"), options))
    })
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Planning {
    #[serde(default = "evidence_tools")]
    pub tool_capabilities: Vec<ToolCapability>,
    /// Calibration seam: replace these named estimates with measured overheads later.
    pub overhead_tokens: BTreeMap<String, u32>,
    pub protected_paths: Vec<String>,
}

impl Planning {
    /// The defaults plus this file's additions. Nothing removes a default.
    pub fn effective_protected_paths(&self) -> Vec<String> {
        protected_paths(&self.protected_paths, &[])
    }

    /// Native-thread support follows the repository's effective runner CPU demand.
    pub fn effective_tool_capabilities(&self, cpus: u32) -> Vec<ToolCapability> {
        let mut tools = self.tool_capabilities.clone();
        for tool in &mut tools {
            if tool.tool == "run_tests" && tool.executor == "prismabuild_pytest" {
                for feature in &mut tool.runner_features {
                    if feature.starts_with("native_threads_") {
                        *feature = format!("native_threads_{cpus}");
                    }
                }
            }
        }
        tools
    }

    /// Defaults this file doesn't list. They stay protected; a newer release can
    /// add a default without the live file repeating it.
    pub fn defaults_not_listed(&self) -> Vec<String> {
        Planning::default()
            .protected_paths
            .into_iter()
            .filter(|path| !self.protected_paths.contains(path))
            .collect()
    }
}

/// Evidence one installed tool can produce in a single execution protocol.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ToolCapability {
    pub tool: String,
    pub executor: String,
    pub runner_features: Vec<String>,
    pub artifacts: Vec<String>,
    pub methods: Vec<String>,
    pub protected_files: Vec<String>,
}

fn evidence_tools() -> Vec<ToolCapability> {
    vec![ToolCapability {
        tool: "run_tests".into(),
        executor: "prismabuild_pytest".into(),
        runner_features: [
            "base_head",
            "repeat",
            "parallel",
            "always_tests",
            "native_threads_1",
        ]
        .map(String::from)
        .to_vec(),
        artifacts: [
            "action_key",
            "cas_receipt",
            "sha",
            "logs",
            "terminal_verdict",
        ]
        .map(String::from)
        .to_vec(),
        methods: [
            "red_green",
            "declared_symptom",
            "head",
            "base_head",
            "subject_observer",
            "forced_reproduction.environment",
            "forced_reproduction.patches",
        ]
        .map(String::from)
        .to_vec(),
        protected_files: Vec::new(),
    }]
}

impl Default for Planning {
    fn default() -> Self {
        Self {
            tool_capabilities: evidence_tools(),
            overhead_tokens: [
                ("prompt".into(), 5_000),
                ("making".into(), 20_000),
                ("review".into(), 5_000),
            ]
            .into(),
            protected_paths: [
                ".github/**",
                ".mergify.yml",
                ".mergify.yaml",
                "**/conftest.py",
                "pytest.ini",
                ".pytest.ini",
                "tox.ini",
                "noxfile.py",
                "setup.cfg",
                "pyproject.toml",
            ]
            .map(String::from)
            .to_vec(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workers {
    pub python: PathBuf,
    pub runs: PathBuf,
    #[serde(default)]
    pub tiktoken_cache_dir: Option<PathBuf>,
    /// Arguments passed to the configured worker Python.
    pub command: Vec<String>,
    /// A command that starts each attempt's supervisor outside the controller's
    /// cgroup, such as `systemd-run --user --scope`. Empty starts it directly.
    #[serde(default)]
    pub launcher: Vec<String>,
}

impl Default for Workers {
    fn default() -> Self {
        Self {
            python: PathBuf::from("/srv/tl3/workers/.venv/bin/python"),
            runs: PathBuf::from("/srv/tl3/worker-runs"),
            tiktoken_cache_dir: None,
            command: vec!["-m".into(), "tl3_workers".into(), "run".into()],
            launcher: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Store {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub user: String,
    pub password_file: Option<PathBuf>,
    pub connect_timeout_secs: u32,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".into(),
            port: 5432,
            database: "tl3".into(),
            user: "tl3".into(),
            password_file: None,
            connect_timeout_secs: 10,
        }
    }
}

impl Store {
    pub fn validate(&self) -> Result<(), Error> {
        for (field, value) in [
            ("store.host", &self.host),
            ("store.database", &self.database),
            ("store.user", &self.user),
        ] {
            nonempty(field, value)?;
        }
        if !matches!(self.host.as_str(), "localhost" | "127.0.0.1" | "::1")
            && !Path::new(&self.host).is_absolute()
        {
            return Err(invalid(
                "store.host",
                "use localhost, 127.0.0.1, ::1, or an absolute Unix socket directory; store connections have no TLS",
            ));
        }
        if self.port == 0 || self.connect_timeout_secs == 0 {
            return Err(invalid(
                "store",
                "port and connect_timeout_secs must be greater than zero",
            ));
        }
        if let Some(path) = &self.password_file {
            absolute("store.password_file", path)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub fit_tokens: u32,
    pub context_tokens: u32,
    pub credit_reserve: u32,
    pub free_rechecks: u32,
    pub w: u32,
    #[serde(default = "default_agent_pool")]
    pub agent_pool: u8,
    /// Starting credit reserve by priority, keyed P0 to P3 (#416). Empty, the
    /// default, keeps the kernel's reserve of 3 for every request.
    #[serde(default)]
    pub priority_credit_reserve: BTreeMap<String, u8>,
    /// Q54 (#440): stop for TL3 review instead of retrying the work.
    #[serde(default, deserialize_with = "fail_fast_setting")]
    pub fail_fast: bool,
    /// #486: one postmortem run's whole wall-clock allowance, in seconds. A run
    /// past it fails with the `deadline` class.
    #[serde(default = "default_postmortem_deadline_secs")]
    pub postmortem_deadline_secs: u32,
    /// Q53 / #828: hints farther out than this use one shared hourly probe.
    #[serde(default = "default_model_quota_horizon_secs")]
    pub model_quota_horizon_secs: u32,
}

impl Limits {
    /// The configured reserve for a P0–P3 priority, if any.
    pub fn credit_reserve_for(&self, priority: Option<u8>) -> Option<u8> {
        self.priority_credit_reserve
            .get(&format!("P{}", priority?))
            .copied()
    }
}

fn default_model_quota_horizon_secs() -> u32 {
    6 * 3600
}

fn default_agent_pool() -> u8 {
    1
}

fn fail_fast_setting<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    bool::deserialize(deserializer)
        .map_err(|_| serde::de::Error::custom("limits.fail_fast must be true or false"))
}

fn default_postmortem_deadline_secs() -> u32 {
    20 * 60
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitHub {
    #[serde(default)]
    pub webhook_listen: Option<SocketAddr>,
    #[serde(default)]
    pub operations: Option<OperationsIssue>,
    pub app_credentials_file: PathBuf,
    pub rob: Identity,
    pub repos: Vec<Repository>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OperationsIssue {
    pub owner: String,
    pub name: String,
    pub installation_id: u64,
    pub number: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub login: String,
    pub id: u64,
}

fn generator_timeout_setting<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<u32>, D::Error> {
    Option::<u32>::deserialize(deserializer).map_err(|_| {
        serde::de::Error::custom("generator_timeout_s must be a whole number from 1 to 600 seconds")
    })
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Repository {
    #[serde(default)]
    pub protected_paths: Vec<String>,
    /// Generated outputs cost a regeneration allowance rather than a source edit.
    #[serde(default)]
    pub generated_files: Vec<String>,
    /// Fixed argv command for the admitted generator action; never model-selected.
    #[serde(default)]
    pub generator: Option<String>,
    /// Container startup plus command deadline. Unset keeps 120 seconds.
    #[serde(default, deserialize_with = "generator_timeout_setting")]
    pub generator_timeout_s: Option<u32>,
    #[serde(default)]
    pub container_image: Option<String>,
    /// CPU and memory reservation for this repository's PrismaBuild tests and generators.
    #[serde(default)]
    pub test_demand: Option<TestDemand>,
    /// Run up to this many existing tests that import the change, and pass on the base,
    /// as one more regression check (#359). Absent means off.
    #[serde(default)]
    pub selected_tests: Option<u32>,
    #[serde(default)]
    pub always_tests: Vec<String>,
    pub owner: String,
    pub name: String,
    pub installation_id: u64,
    #[serde(default = "default_branch_template")]
    pub branch_template: String,
    #[serde(default = "default_base_branch")]
    pub base_branch: String,
    /// Lines the repository's PR-body policy requires, such as PrismaBuild's
    /// `Change type:` (#447). TL3 writes and keeps one valid line per rule.
    #[serde(default)]
    pub pr_body_lines: Vec<BodyLine>,
    /// Workflow files, such as `gate.yml`, whose failed `pull_request` runs a
    /// PR-body heal reruns because they don't rerun on `edited` (#447). Empty
    /// means the heal reruns nothing.
    #[serde(default)]
    pub heal_rerun_workflows: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BodyLine {
    /// The line's label, written as `label: value`.
    pub label: String,
    pub values: Vec<String>,
    /// Used when TL3 can't tell the kind of change from its files.
    pub default: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TestDemand {
    pub cpus: u32,
    pub mem_gb: u32,
}

impl Default for TestDemand {
    fn default() -> Self {
        Self { cpus: 1, mem_gb: 2 }
    }
}

/// The harness's `MAX_SELECTED_TESTS`.
pub const MAX_SELECTED_TESTS: u32 = 1000;
pub const MAX_ALWAYS_TESTS: usize = 20;

fn default_branch_template() -> String {
    tl3_worktrees::branch_template::DEFAULT_TEMPLATE.into()
}
fn default_base_branch() -> String {
    tl3_worktrees::branch_template::DEFAULT_BASE.into()
}

fn validate_protected_paths(field: &str, patterns: &[String]) -> Result<(), Error> {
    for pattern in patterns {
        if pattern.trim().is_empty()
            || pattern.starts_with('/')
            || pattern.contains('\\')
            || pattern.chars().any(char::is_control)
            || pattern.split('/').any(|part| matches!(part, "." | ".."))
            || glob::Pattern::new(pattern).is_err()
        {
            return Err(invalid(
                field,
                "use nonempty repository-relative glob patterns without traversal",
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Models {
    pub base_url: Url,
    pub key_file: PathBuf,
    pub ingest: Vec<Model>,
    pub audit: Vec<Model>,
    pub planner: Vec<Model>,
    pub maker: Vec<Model>,
    #[serde(default)]
    pub reviewer: Vec<Model>,
    pub rebaser: Vec<Model>,
    pub comment_classifier: Vec<Model>,
    #[serde(default)]
    pub writer: Vec<Model>,
    /// #403: the first model writes postmortems; the second, if any, runs the
    /// rubric's grade judges. Defaults to the reviewer's and the writer's first.
    #[serde(default)]
    pub postmortem: Vec<Model>,
}

/// Rob, 2026-11-02: Fable is not used. TL3's models are limited to these.
/// Config loading does not enforce the list yet.
pub const ALLOWED_MODELS: &[&str] = &["claude-opus-5-5", "claude-sonnet-5-5", "gpt-6.1-sol"];

/// The allowed models the shipped defaults use now. Sol 6.1 is allowed but out
/// of quota on both accounts (Rob, 2026-11-02), so TL3 is Anthropic-only.
pub const ACTIVE_MODELS: &[&str] = &["claude-opus-5-5", "claude-sonnet-5-5"];

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Model {
    pub name: String,
    pub family: String,
    pub effort: Option<String>,
    pub budget_tokens: u32,
}

/// The model object in workers' protocol v1, without config-only family metadata.
#[derive(Clone, Debug, Serialize)]
pub struct WorkerModel<'a> {
    pub base_url: &'a Url,
    pub name: &'a str,
    pub effort: Option<&'a str>,
    pub budget_tokens: u32,
}

impl Models {
    /// Select the first reviewer from a different family for this maker attempt.
    pub fn reviewer_for(&self, maker_family: &str) -> Option<&Model> {
        self.reviewer
            .iter()
            .find(|model| !model.family.eq_ignore_ascii_case(maker_family))
    }

    pub fn worker_model<'a>(&'a self, model: &'a Model) -> WorkerModel<'a> {
        WorkerModel {
            base_url: &self.base_url,
            name: &model.name,
            effort: model.effort.as_deref(),
            budget_tokens: model.budget_tokens,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrismaBuild {
    pub tools_dir: PathBuf,
    pub container_image: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hindsight {
    pub endpoint: Url,
    pub bank: String,
    #[serde(default)]
    pub api_key_file: Option<PathBuf>,
}

impl Hindsight {
    /// Read for each retain or curation batch and each recall; it only ever goes
    /// into the Authorization header.
    pub fn retain_key(&self) -> Result<Option<String>, Error> {
        let Some(path) = &self.api_key_file else {
            return Ok(None);
        };
        validate_secret_file("hindsight.api_key_file", path)?;
        let key = fs::read_to_string(path).map_err(|source| Error::File {
            field: "hindsight.api_key_file".into(),
            path: path.clone(),
            source,
        })?;
        let key = key.trim();
        if key.is_empty() || !key.bytes().all(|byte| byte.is_ascii_graphic()) {
            return Err(invalid(
                "hindsight.api_key_file",
                "must contain one nonempty API key",
            ));
        }
        Ok(Some(key.to_owned()))
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Status {
    pub listen: SocketAddr,
}

/// PRD #327: the operator console, a service apart from tl3d and the status page.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Console {
    pub listen: SocketAddr,
    /// The built web app. Deploy builds and installs it, so Rust builds never need Node.
    pub assets: PathBuf,
    /// A Postgres login that may only read, used instead of `store.user`.
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub password_file: Option<PathBuf>,
}

impl Console {
    /// The store settings the console connects with.
    pub fn store(&self, store: &Store) -> Store {
        match &self.user {
            Some(user) => Store {
                user: user.clone(),
                password_file: self.password_file.clone(),
                ..store.clone()
            },
            None => store.clone(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Allowances {
    pub agent_worker: Allowance,
    pub prismabuild: Allowance,
    pub github_mergify: Allowance,
    pub model_calls: Allowance,
    pub hindsight: Allowance,
    pub worktrees: Allowance,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Allowance {
    pub silence_secs: u32,
    pub no_progress_secs: u32,
}

impl Default for Allowances {
    fn default() -> Self {
        Self {
            agent_worker: Allowance {
                silence_secs: 120,
                no_progress_secs: 900,
            },
            prismabuild: Allowance {
                silence_secs: 300,
                no_progress_secs: 900,
            },
            github_mergify: Allowance {
                silence_secs: 300,
                no_progress_secs: 1800,
            },
            model_calls: Allowance {
                silence_secs: 120,
                no_progress_secs: 900,
            },
            hindsight: Allowance {
                silence_secs: 60,
                no_progress_secs: 120,
            },
            worktrees: Allowance {
                silence_secs: 120,
                no_progress_secs: 900,
            },
        }
    }
}

impl Config {
    /// Parse and validate without accessing secrets, executables, or endpoints.
    pub fn parse(source: &str) -> Result<Self, Error> {
        let mut config: Self = toml::from_str(source).map_err(|error: toml::de::Error| {
            let position = error
                .span()
                .map(|span| {
                    let prefix = &source[..span.start];
                    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
                    let column = prefix.rsplit('\n').next().unwrap_or("").chars().count() + 1;
                    format!(" at line {line}, column {column}")
                })
                .unwrap_or_default();
            Error::Parse(format!("{}{position}", error.message()))
        })?;
        if config.models.reviewer.is_empty() {
            // The first and last maker rungs; validation then rejects an empty ladder.
            config.models.reviewer = config
                .models
                .maker
                .first()
                .into_iter()
                .chain(config.models.maker.last())
                .cloned()
                .collect();
        }
        if config.models.writer.is_empty() {
            config.models.writer = config.models.ingest.clone();
        }
        if config.models.postmortem.is_empty() {
            config.models.postmortem = (config.models.reviewer.first().into_iter())
                .chain(config.models.writer.first())
                .cloned()
                .collect();
        }
        config.validate()?;
        Ok(config)
    }

    /// Read the TOML file and check secret-file metadata, without reading secrets.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        let source = fs::read_to_string(path).map_err(|source| Error::File {
            field: "config".into(),
            path: path.into(),
            source,
        })?;
        let config = Self::parse(&source)?;
        config.validate_secret_files()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), Error> {
        self.store.validate()?;
        if let Some(operations) = &self.operations {
            github_name("operations.owner", &operations.owner, false)?;
            github_name("operations.repo", &operations.repo, true)?;
            if operations.delegated_attributions.len() > 32
                || operations.delegated_attributions.iter().any(|line| {
                    line.is_empty()
                        || line.len() > 1000
                        || line.trim() != line
                        || line.chars().any(char::is_control)
                })
                || operations
                    .delegated_attributions
                    .iter()
                    .collect::<BTreeSet<_>>()
                    .len()
                    != operations.delegated_attributions.len()
            {
                return Err(invalid(
                    "operations.delegated_attributions",
                    "needs at most 32 unique, nonempty single lines of at most 1000 bytes without surrounding whitespace",
                ));
            }
            if operations.issue == 0 {
                return Err(invalid(
                    "operations.issue",
                    "must be a positive issue number",
                ));
            }
        }
        let installed = evidence_tools().remove(0);
        for capability in &self.planning.tool_capabilities {
            if capability.tool != installed.tool
                || capability.executor != installed.executor
                || !capability
                    .runner_features
                    .iter()
                    .all(|v| installed.runner_features.contains(v))
                || !capability
                    .artifacts
                    .iter()
                    .all(|v| installed.artifacts.contains(v))
                || !capability
                    .methods
                    .iter()
                    .all(|v| installed.methods.contains(v))
                || !capability.protected_files.is_empty()
            {
                return Err(invalid(
                    "planning.tool_capabilities",
                    "must describe installed run_tests capabilities; configure a subset or disable with []",
                ));
            }
        }
        if self.planning.overhead_tokens.is_empty()
            || self
                .planning
                .overhead_tokens
                .keys()
                .any(|key| key.trim().is_empty())
        {
            return Err(invalid(
                "planning.overhead_tokens",
                "must contain named overhead estimates",
            ));
        }
        validate_protected_paths("planning.protected_paths", &self.planning.protected_paths)?;
        let limits = &self.limits;
        for (priority, reserve) in &limits.priority_credit_reserve {
            // The kernel's reserve is 3; a priority can only raise it.
            if !matches!(priority.as_str(), "P0" | "P1" | "P2" | "P3")
                || !(3..=100).contains(reserve)
            {
                return Err(invalid(
                    "limits.priority_credit_reserve",
                    "use keys P0 to P3 with reserves from 3 to 100",
                ));
            }
        }
        if limits.context_tokens == 0 {
            return Err(invalid(
                "limits.context_tokens",
                "must be greater than zero",
            ));
        }
        if limits.fit_tokens == 0 || limits.fit_tokens >= limits.context_tokens {
            return Err(invalid(
                "limits.fit_tokens",
                "must be greater than zero and less than limits.context_tokens",
            ));
        }
        let overhead = self
            .planning
            .overhead_tokens
            .values()
            .try_fold(0u32, |sum, value| sum.checked_add(*value));
        if overhead.is_none_or(|sum| sum >= limits.fit_tokens) {
            return Err(invalid(
                "planning.overhead_tokens",
                "sum must be less than limits.fit_tokens",
            ));
        }
        if !(1..=3).contains(&limits.w) {
            return Err(invalid("limits.w", "must be in 1..=3"));
        }
        if limits.postmortem_deadline_secs == 0 {
            return Err(invalid(
                "limits.postmortem_deadline_secs",
                "must be greater than zero",
            ));
        }
        if limits.agent_pool == 0 {
            return Err(invalid("limits.agent_pool", "must be greater than zero"));
        }
        absolute("worktrees_root", &self.worktrees_root)?;
        absolute(
            "github.app_credentials_file",
            &self.github.app_credentials_file,
        )?;
        absolute("models.key_file", &self.models.key_file)?;
        absolute("prismabuild.tools_dir", &self.prismabuild.tools_dir)?;
        if let Some(address) = self.github.webhook_listen
            && (!address.ip().is_loopback() || address.port() == 0)
        {
            return Err(invalid(
                "github.webhook_listen",
                "use a loopback address with a nonzero port; cloudflared forwards locally",
            ));
        }
        github_name("github.rob.login", &self.github.rob.login, false)?;
        if self.github.rob.id == 0 {
            return Err(invalid(
                "github.rob.id",
                "must be a positive GitHub user id; label authorization uses this id (Q2)",
            ));
        }
        if self.github.repos.is_empty() {
            return Err(invalid(
                "github.repos",
                "must contain at least one served repository",
            ));
        }
        let mut repos = BTreeSet::new();
        for (index, repo) in self.github.repos.iter().enumerate() {
            let field = format!("github.repos[{index}]");
            github_name(&format!("{field}.owner"), &repo.owner, false)?;
            github_name(&format!("{field}.name"), &repo.name, true)?;
            if let Some(demand) = repo.test_demand {
                if !(1..=16).contains(&demand.cpus) {
                    return Err(invalid(
                        format!("{field}.test_demand.cpus"),
                        "use 1 to 16 CPUs",
                    ));
                }
                if !(1..=64).contains(&demand.mem_gb) {
                    return Err(invalid(
                        format!("{field}.test_demand.mem_gb"),
                        "use 1 to 64 GiB",
                    ));
                }
            }
            if let Some(timeout) = repo.generator_timeout_s
                && !(1..=600).contains(&timeout)
            {
                return Err(invalid(
                    format!("{field}.generator_timeout_s"),
                    "use 1 to 600 seconds; unset keeps 120 seconds",
                ));
            }
            validate_protected_paths(&format!("{field}.protected_paths"), &repo.protected_paths)?;
            validate_protected_paths(&format!("{field}.generated_files"), &repo.generated_files)?;
            if let Some(command) = &repo.generator
                && (command.trim().is_empty() || command.chars().any(char::is_control))
            {
                return Err(invalid(
                    format!("{field}.generator"),
                    "use a nonempty single-line generator command",
                ));
            }
            for (line, rule) in repo.pr_body_lines.iter().enumerate() {
                let label = &rule.label;
                if label.trim().is_empty()
                    || label.contains([':', '\n', '\r'])
                    || rule.values.is_empty()
                    || rule
                        .values
                        .iter()
                        .any(|value| value.trim().is_empty() || value.contains(['\n', '\r']))
                    || !rule.values.contains(&rule.default)
                {
                    return Err(invalid(
                        format!("{field}.pr_body_lines[{line}]"),
                        "needs a label without a colon, nonempty values, and a default among them",
                    ));
                }
            }
            let mut workflows = BTreeSet::new();
            for (entry, file) in repo.heal_rerun_workflows.iter().enumerate() {
                if file.is_empty()
                    || file.contains(['/', '\\'])
                    || file.chars().any(char::is_whitespace)
                    || !workflows.insert(file)
                {
                    return Err(invalid(
                        format!("{field}.heal_rerun_workflows[{entry}]"),
                        "needs a distinct workflow file name, such as gate.yml, without a directory",
                    ));
                }
            }
            tl3_worktrees::branch_template::BranchTemplate::new(&repo.branch_template)
                .map_err(|error| invalid(format!("{field}.branch_template"), error.to_string()))?;
            tl3_worktrees::BranchName::new(&repo.base_branch)
                .map_err(|error| invalid(format!("{field}.base_branch"), error.to_string()))?;
            if repo
                .container_image
                .as_deref()
                .is_some_and(|image| !immutable_image(image))
            {
                return Err(invalid(
                    format!("{field}.container_image"),
                    "use sha256:<64 lowercase hex digits> or repository@sha256:<64 lowercase hex digits>, as harness.Container requires",
                ));
            }
            if repo
                .selected_tests
                .is_some_and(|limit| !(1..=MAX_SELECTED_TESTS).contains(&limit))
            {
                return Err(invalid(
                    format!("{field}.selected_tests"),
                    format!("must be 1 to {MAX_SELECTED_TESTS}, the harness's limit"),
                ));
            }
            if repo.always_tests.len() > MAX_ALWAYS_TESTS {
                return Err(invalid(
                    format!("{field}.always_tests"),
                    format!("always_tests accepts at most {MAX_ALWAYS_TESTS} entries"),
                ));
            }
            for (entry, test) in repo.always_tests.iter().enumerate() {
                let mut parts = test.split("::");
                let path = parts.next().unwrap_or_default();
                if !path.ends_with(".py")
                    || path.starts_with(['/', '-'])
                    || path.contains(['\\', ':', '*', '?', '[', ']'])
                    || path.split('/').any(|part| matches!(part, "" | "." | ".."))
                    || parts.any(str::is_empty)
                    || test.chars().any(char::is_control)
                {
                    return Err(invalid(
                        format!("{field}.always_tests[{entry}]"),
                        "use a repo-relative pytest file or node ID without globs or parent traversal",
                    ));
                }
            }
            if repo.installation_id == 0 {
                return Err(invalid(
                    format!("{field}.installation_id"),
                    "must be a positive app installation id, not the app id",
                ));
            }
            if !repos.insert((
                repo.owner.to_ascii_lowercase(),
                repo.name.to_ascii_lowercase(),
            )) {
                return Err(invalid(
                    field,
                    "duplicate served repository; GitHub names ignore case",
                ));
            }
        }
        for (field, path) in [
            ("workers.python", &self.workers.python),
            ("workers.runs", &self.workers.runs),
        ] {
            absolute(field, path)?;
        }
        if let Some(path) = &self.workers.tiktoken_cache_dir {
            absolute("workers.tiktoken_cache_dir", path)?;
        }
        if self.workers.command.is_empty() {
            return Err(invalid("workers.command", "must contain a Python command"));
        }
        for arg in &self.workers.command {
            nonempty("workers.command", arg)?;
        }
        for arg in &self.workers.launcher {
            nonempty("workers.launcher", arg)?;
        }
        endpoint("models.base_url", &self.models.base_url)?;
        endpoint("hindsight.endpoint", &self.hindsight.endpoint)?;
        if let Some(path) = &self.hindsight.api_key_file {
            absolute("hindsight.api_key_file", path)?;
        }
        if self.hindsight.bank.trim().is_empty() {
            return Err(invalid("hindsight.bank", "must not be empty"));
        }
        for (role, ladder) in [
            ("ingest", self.models.ingest.as_slice()),
            ("audit", &self.models.audit),
            ("planner", &self.models.planner),
            ("maker", &self.models.maker),
            ("reviewer", &self.models.reviewer),
            ("rebaser", &self.models.rebaser),
            ("comment_classifier", &self.models.comment_classifier),
            ("writer", &self.models.writer),
            ("postmortem", &self.models.postmortem),
        ] {
            if ladder.is_empty() {
                return Err(invalid(
                    format!("models.{role}"),
                    "must contain at least one model",
                ));
            }
            for (index, model) in ladder.iter().enumerate() {
                let field = format!("models.{role}[{index}]");
                nonempty(&format!("{field}.name"), &model.name)?;
                nonempty(&format!("{field}.family"), &model.family)?;
                if let Some(effort) = &model.effort {
                    nonempty(&format!("{field}.effort"), effort)?;
                }
                if model.budget_tokens == 0 || model.budget_tokens > limits.context_tokens {
                    return Err(invalid(
                        format!("{field}.budget_tokens"),
                        "must be greater than zero and at most limits.context_tokens",
                    ));
                }
            }
        }
        if self.models.postmortem.len() > 2 {
            return Err(invalid(
                "models.postmortem",
                "name at most two models: the postmortem's, then the grade judges' (#403)",
            ));
        }
        if self.models.planner.len() < 2 {
            return Err(invalid(
                "models.planner",
                "add a second model: a plan that can't meet re-runs with the next model on the planner's ladder",
            ));
        }
        // Q22, amended by Rob on 2026-11-02 (#416): attempt 2 escalates the model
        // line, not the effort. Attempts past the ladder use its last rung.
        let maker = &self.models.maker;
        if maker.len() < 2 || maker[1].family.eq_ignore_ascii_case(&maker[0].family) {
            return Err(invalid(
                "models.maker",
                "the maker ladder must escalate to a different model line by attempt 2 (Q22)",
            ));
        }
        for (index, maker) in self.models.maker.iter().enumerate() {
            if self.models.reviewer_for(&maker.family).is_none() {
                return Err(invalid(
                    "models.reviewer",
                    format!(
                        "add a reviewer from a different family than models.maker[{index}] ({}) (Q22)",
                        maker.family
                    ),
                ));
            }
        }
        if !immutable_image(&self.prismabuild.container_image) {
            return Err(invalid(
                "prismabuild.container_image",
                "use sha256:<64 lowercase hex digits> or repository@sha256:<64 lowercase hex digits>, as harness.Container requires",
            ));
        }
        if self.status.listen.port() == 0
            || self.status.listen.ip().is_multicast()
            || self.status.listen.ip().is_unspecified()
        {
            return Err(invalid(
                "status.listen",
                "use a unicast listen address with a fixed port from 1 to 65535 (Q24)",
            ));
        }
        if let Some(console) = &self.console {
            if console.listen.port() == 0
                || console.listen.ip().is_multicast()
                || console.listen.ip().is_unspecified()
            {
                return Err(invalid(
                    "console.listen",
                    "use a unicast listen address with a fixed port from 1 to 65535",
                ));
            }
            absolute("console.assets", &console.assets)?;
            match (&console.user, &console.password_file) {
                (None, None) => {}
                (Some(user), Some(path)) => {
                    // The deploy helper names this role in GRANT statements.
                    if user.is_empty()
                        || !user.starts_with(|c: char| c.is_ascii_lowercase() || c == '_')
                        || !user
                            .chars()
                            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
                    {
                        return Err(invalid(
                            "console.user",
                            "use a lowercase Postgres role name: letters, digits and underscores",
                        ));
                    }
                    absolute("console.password_file", path)?;
                }
                (Some(_), None) => {
                    return Err(invalid("console.password_file", "set it with console.user"));
                }
                (None, Some(_)) => {
                    return Err(invalid("console.user", "set it with console.password_file"));
                }
            }
        }
        if let Some(operations) = &self.github.operations {
            github_name("github.operations.owner", &operations.owner, false)?;
            github_name("github.operations.name", &operations.name, true)?;
            if operations.installation_id == 0 || operations.number == 0 {
                return Err(invalid(
                    "github.operations",
                    "installation_id and number must be greater than zero",
                ));
            }
        }
        for (name, allowance) in [
            ("agent_worker", &self.allowances.agent_worker),
            ("prismabuild", &self.allowances.prismabuild),
            ("github_mergify", &self.allowances.github_mergify),
            ("model_calls", &self.allowances.model_calls),
            ("hindsight", &self.allowances.hindsight),
            ("worktrees", &self.allowances.worktrees),
        ] {
            for (kind, seconds) in [
                ("silence_secs", allowance.silence_secs),
                ("no_progress_secs", allowance.no_progress_secs),
            ] {
                if seconds == 0 {
                    return Err(invalid(
                        format!("allowances.{name}.{kind}"),
                        "must be greater than zero; an allowance triggers a look, not a work deadline",
                    ));
                }
            }
        }
        Ok(())
    }

    /// Inspect only metadata. Symlinks are followed and the target must be private.
    pub fn validate_secret_files(&self) -> Result<(), Error> {
        for (field, path) in [
            (
                "github.app_credentials_file",
                &self.github.app_credentials_file,
            ),
            ("models.key_file", &self.models.key_file),
        ]
        .into_iter()
        .chain(
            self.store
                .password_file
                .as_ref()
                .map(|path| ("store.password_file", path)),
        ) {
            validate_secret_file(field, path)?;
        }
        Ok(())
    }

    /// The console needs only the store credential it connects with: its own
    /// login's when it has one, otherwise the store's.
    pub fn load_console(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        let text = fs::read_to_string(path).map_err(|source| Error::File {
            field: "config".into(),
            path: path.into(),
            source,
        })?;
        let config = Self::parse(&text)?;
        let console = config
            .console
            .as_ref()
            .ok_or_else(|| invalid("console", "add a [console] section"))?;
        match (&console.password_file, &config.store.password_file) {
            (Some(path), _) => validate_secret_file("console.password_file", path)?,
            (None, Some(path)) => validate_secret_file("store.password_file", path)?,
            (None, None) => {}
        }
        Ok(config)
    }

    /// Status needs only its store credential. It can run before model setup.
    pub fn load_status(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        let text = fs::read_to_string(path).map_err(|source| Error::File {
            field: "config".into(),
            path: path.into(),
            source,
        })?;
        let config = Self::parse(&text)?;
        if let Some(path) = &config.store.password_file {
            validate_secret_file("store.password_file", path)?;
        }
        Ok(config)
    }
}

fn validate_secret_file(field: &str, path: &Path) -> Result<(), Error> {
    let metadata = fs::metadata(path).map_err(|source| Error::File {
        field: field.into(),
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.is_file() {
        return Err(invalid(field, "must point to a regular secret file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = metadata.permissions().mode();
        if mode & 0o077 != 0 || mode & 0o400 == 0 {
            return Err(invalid(
                field,
                "secret file must be owner-readable and owner-only; run chmod 600 on the file",
            ));
        }
    }
    #[cfg(not(unix))]
    return Err(invalid(
        field,
        "owner-only secret-file checks require Unix permissions",
    ));
    Ok(())
}

fn nonempty(field: &str, value: &str) -> Result<(), Error> {
    if value.trim().is_empty() || value.trim() != value || value.chars().any(char::is_control) {
        return Err(invalid(
            field,
            "must be nonempty without surrounding whitespace or control characters",
        ));
    }
    Ok(())
}

fn absolute(field: &str, path: &Path) -> Result<(), Error> {
    if !path.is_absolute() || path.to_string_lossy().chars().any(char::is_control) {
        return Err(invalid(field, "use an absolute path; expand ~ yourself"));
    }
    Ok(())
}

fn github_name(field: &str, value: &str, repository: bool) -> Result<(), Error> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || !value.bytes().all(|c| {
            c.is_ascii_alphanumeric() || c == b'-' || (repository && matches!(c, b'_' | b'.'))
        })
    {
        return Err(invalid(
            field,
            "use a GitHub name, without slashes, spaces, or URL syntax",
        ));
    }
    Ok(())
}

fn endpoint(field: &str, url: &Url) -> Result<(), Error> {
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid(
            field,
            "use an HTTP or HTTPS endpoint without credentials, query, or fragment; keep secrets in files",
        ));
    }
    Ok(())
}

fn immutable_image(image: &str) -> bool {
    let reference = if let Some((repository, digest)) = image.split_once('@') {
        if repository.is_empty()
            || !repository
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_./:-".contains(&c))
        {
            return false;
        }
        digest
    } else {
        image
    };
    reference.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    })
}
