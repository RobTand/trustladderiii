#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
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
    pub store: Store,
    pub limits: Limits,
    pub github: GitHub,
    pub models: Models,
    pub worktrees_root: PathBuf,
    pub prismabuild: PrismaBuild,
    pub hindsight: Hindsight,
    pub status: Status,
    pub allowances: Allowances,
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
}

fn default_agent_pool() -> u8 {
    1
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitHub {
    pub app_credentials_file: PathBuf,
    pub rob: Identity,
    pub repos: Vec<Repository>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub login: String,
    pub id: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Repository {
    pub owner: String,
    pub name: String,
    pub installation_id: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Models {
    pub base_url: Url,
    pub key_file: PathBuf,
    pub ingest: Vec<Model>,
    pub audit: Vec<Model>,
    pub planner: Vec<Model>,
    pub maker: [Model; 3],
    #[serde(default)]
    pub reviewer: Vec<Model>,
    pub rebaser: Vec<Model>,
    pub comment_classifier: Vec<Model>,
}

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
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Status {
    pub listen: SocketAddr,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Allowances {
    pub agent_worker: Allowance,
    pub prismabuild: Allowance,
    pub github_mergify: Allowance,
    pub model_calls: Allowance,
    pub hindsight: Allowance,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Allowance {
    pub silence_secs: u32,
    pub no_progress_secs: u32,
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
            config.models.reviewer = vec![
                config.models.maker[0].clone(),
                config.models.maker[2].clone(),
            ];
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
        let limits = &self.limits;
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
        if limits.w != 1 {
            return Err(invalid("limits.w", "must be 1 in slice one (Q29)"));
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
        endpoint("models.base_url", &self.models.base_url)?;
        endpoint("hindsight.endpoint", &self.hindsight.endpoint)?;
        for (role, ladder) in [
            ("ingest", self.models.ingest.as_slice()),
            ("audit", &self.models.audit),
            ("planner", &self.models.planner),
            ("maker", &self.models.maker),
            ("reviewer", &self.models.reviewer),
            ("rebaser", &self.models.rebaser),
            ("comment_classifier", &self.models.comment_classifier),
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
        let [first, second, third] = &self.models.maker;
        if first.name != second.name || first.family != second.family {
            return Err(invalid(
                "models.maker[1]",
                "attempts 1 and 2 must use the same model and family (Q22)",
            ));
        }
        if second.budget_tokens < first.budget_tokens || second.effort.is_none() {
            return Err(invalid(
                "models.maker[1]",
                "set the proxy's maximum effort and a budget at least as large as attempt 1 (Q22)",
            ));
        }
        if first.family.eq_ignore_ascii_case(&third.family) {
            return Err(invalid(
                "models.maker[2].family",
                "attempt 3 must use a different family from attempt 1 (Q22)",
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
        for (name, allowance) in [
            ("agent_worker", &self.allowances.agent_worker),
            ("prismabuild", &self.allowances.prismabuild),
            ("github_mergify", &self.allowances.github_mergify),
            ("model_calls", &self.allowances.model_calls),
            ("hindsight", &self.allowances.hindsight),
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
            let metadata = fs::metadata(path).map_err(|source| Error::File {
                field: field.into(),
                path: path.clone(),
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
        }
        Ok(())
    }
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
