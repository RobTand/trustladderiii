//! Deployment-only REST operations. Credentials never leave this process.
use reqwest::{Client, Method};
use serde_json::{Value, json};
use std::{fs, io::Write, path::Path, time::Duration};
use tl3_github::{AppConfig, ClientOptions, GitHub};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const MARKER: &str = "<!-- tl3:operations:v1 -->";
const TITLE: &str = "TL3 operations";

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let [command, path, rest @ ..] = args.as_slice() else {
        return Err("usage: tl3-deploy COMMAND CONFIG [HOSTNAME]".into());
    };
    let text = fs::read_to_string(path).map_err(|_| "cannot read config")?;
    let config = tl3_config::Config::parse(&text)?;
    match command.as_str() {
        "parse" => println!("config: valid offline"),
        "validate" => {
            tl3_config::Config::load(path)?;
            println!("config: Config::load passed");
        }
        "check" => {
            let home = std::env::var_os("HOME").ok_or("HOME is required")?;
            let home = Path::new(&home);
            if Path::new(path) != home.join(".config/tl3/tl3.toml") {
                return Err("check requires the installed ~/.config/tl3/tl3.toml".into());
            }
            let status = std::process::Command::new("python3")
                .arg(home.join(".local/share/tl3/deploy/deploy.py"))
                .arg("check")
                .status()?;
            std::process::exit(status.code().unwrap_or(1));
        }
        "database" => database(&tl3_config::Config::load_status(path)?)?,
        "store-check" => {
            let config = tl3_config::Config::load_status(path)?;
            tl3_store::Reader::open(&config.store)?;
            println!("Postgres: reachable, migrations match this binary");
        }
        "app-check" | "operations" | "webhook-configure" => {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?
                .block_on(app(command, &config, rest))?;
        }
        _ => return Err("unknown deploy command".into()),
    }
    Ok(())
}

fn database(config: &tl3_config::Config) -> Result<()> {
    let settings = &config.store;
    if settings.database != "tl3" || settings.user != "tl3" {
        return Err("deployment initializes only database and user tl3".into());
    }
    let mut options = postgres::Config::new();
    options
        .host(&settings.host)
        .port(settings.port)
        .user(&settings.user)
        .dbname("postgres")
        .connect_timeout(Duration::from_secs(10));
    if let Some(path) = &settings.password_file {
        let password = fs::read_to_string(path).map_err(|_| "cannot load database credential")?;
        options.password(password.trim_end());
    }
    let mut client = options
        .connect(postgres::NoTls)
        .map_err(|_| "database connection failed")?;
    if client
        .query_opt("SELECT 1 FROM pg_database WHERE datname = 'tl3'", &[])
        .map_err(|_| "database lookup failed")?
        .is_none()
    {
        client
            .batch_execute("CREATE DATABASE tl3 OWNER tl3")
            .map_err(|_| "database creation failed; reconcile before retrying")?;
    }
    tl3_store::Store::open(settings)?;
    println!("Postgres: database exists, fsync on, migrations applied");
    Ok(())
}

struct Api {
    http: Client,
    root: String,
}

#[derive(Debug)]
struct HttpFailure(u16);

impl std::fmt::Display for HttpFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "GitHub HTTP {}", self.0)
    }
}

impl std::error::Error for HttpFailure {}

impl Api {
    async fn request(
        &self,
        method: Method,
        path: &str,
        token: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        let mut request = self
            .http
            .request(method, format!("{}{path}", self.root))
            .bearer_auth(token)
            .header("User-Agent", "tl3-deploy")
            .header("Accept", "application/vnd.github+json")
            .header("Cache-Control", "no-cache")
            .header("X-GitHub-Api-Version", tl3_github::API_VERSION);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .await
            .map_err(|_| "GitHub transport failed; reconcile before retrying")?;
        if !response.status().is_success() {
            return Err(HttpFailure(response.status().as_u16()).into());
        }
        response
            .json()
            .await
            .map_err(|_| "GitHub response schema invalid".into())
    }

    async fn get(&self, path: &str, token: &str) -> Result<Value> {
        self.request(Method::GET, path, token, None).await
    }
}

async fn app(command: &str, config: &tl3_config::Config, args: &[String]) -> Result<()> {
    let credentials = || -> Result<AppConfig> {
        let bytes = fs::read(&config.github.app_credentials_file)
            .map_err(|_| "cannot load app credentials")?;
        serde_json::from_slice(&bytes).map_err(|_| "invalid app credential JSON".into())
    };
    let github = GitHub::new(
        credentials()?,
        config.github.repos[0].installation_id,
        ClientOptions::default(),
    )?;
    let jwt = github.app_jwt(chrono::Utc::now())?;
    let api = Api {
        root: "https://api.github.com/".into(),
        http: Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|_| "cannot build GitHub client")?,
    };
    let info = api.get("app", &jwt).await?;
    if info["id"] != 5542171 || info["slug"] != "tl3-robtand" {
        return Err("deployment expects the existing tl3-robtand app".into());
    }
    let manifest: Value = serde_json::from_str(include_str!("../github-app/manifest.json"))?;
    for (permission, level) in manifest["default_permissions"]
        .as_object()
        .ok_or("invalid manifest")?
    {
        if info["permissions"][permission] != *level {
            return Err("app permission mismatch".into());
        }
    }
    for event in manifest["default_events"]
        .as_array()
        .ok_or("invalid manifest")?
    {
        if !info["events"]
            .as_array()
            .ok_or("missing app events")?
            .contains(event)
        {
            return Err("app event mismatch".into());
        }
    }
    let hook = api.get("app/hook/config", &jwt).await?;
    // GET /app and GET /app/hook/config do not expose hook active. Do not infer
    // inactivity from the placeholder URL. Rob must confirm it in settings.
    println!("App: permissions and events match manifest");
    println!(
        "Webhook: {}",
        if hook["url"] == "https://tl3-webhook.invalid/github/events" {
            "placeholder URL unchanged; inactive at wizard creation, active flag not exposed by REST"
        } else {
            "URL changed; active flag not exposed by REST"
        }
    );
    let mut installation = None;
    let mut page = 1;
    loop {
        let rows = api
            .get(&format!("app/installations?per_page=100&page={page}"), &jwt)
            .await?;
        let rows = rows.as_array().ok_or("invalid installations")?;
        for row in rows {
            if row["account"]["login"] == "RobTand" {
                installation = row["id"].as_u64();
            }
        }
        if rows.len() < 100 {
            break;
        }
        page += 1;
    }
    let installation = installation.ok_or("RobTand installation missing")?;
    let github = GitHub::new(credentials()?, installation, ClientOptions::default())?;
    let token = github.installation_token().await?;
    let mut repos = Vec::new();
    let mut page = 1;
    loop {
        let response = api
            .get(
                &format!("installation/repositories?per_page=100&page={page}"),
                &token,
            )
            .await?;
        let rows = response["repositories"]
            .as_array()
            .ok_or("invalid repository list")?;
        repos.extend(
            rows.iter()
                .filter_map(|row| row["full_name"].as_str().map(str::to_owned)),
        );
        if rows.len() < 100 {
            break;
        }
        page += 1;
    }
    if !repos.iter().any(|repo| repo == "RobTand/tl3-sandbox") {
        return Err("sandbox installation missing".into());
    }
    println!("Installation: {installation}; tl3-sandbox covered");
    if command == "operations" {
        if !repos.iter().any(|repo| repo == "RobTand/trustladderiii") {
            return Err("operations repository installation missing".into());
        }
        let state = Path::new(&config.worktrees_root)
            .parent()
            .ok_or("invalid worktrees root")?
            .join("app-state.json");
        let number = operations(&api, &token, github.app_slug(), installation, &state).await?;
        println!("Operations: https://github.com/RobTand/trustladderiii/issues/{number}");
    } else if command == "webhook-configure" {
        let [host] = args else {
            return Err("webhook-configure needs a tunnel hostname".into());
        };
        if host.is_empty()
            || !host
                .bytes()
                .all(|ch| ch.is_ascii_alphanumeric() || b".-".contains(&ch))
        {
            return Err("invalid tunnel hostname".into());
        }
        api.request(
            Method::PATCH,
            "app/hook/config",
            &jwt,
            Some(json!({
                "url": format!("https://{host}/github/events"), "content_type": "json",
                "insecure_ssl": "0", "secret": credentials()?.webhook_secret,
            })),
        )
        .await?;
        let hook = api.get("app/hook/config", &jwt).await?;
        if hook["url"] != format!("https://{host}/github/events") {
            return Err("webhook URL did not persist".into());
        }
        println!("Webhook URL and existing secret configured. Enable Active in the app settings.");
    }
    Ok(())
}

fn operations_match(row: &Value) -> bool {
    row.get("pull_request").is_none_or(Value::is_null)
        && row["title"] == TITLE
        && row["body"]
            .as_str()
            .is_some_and(|body| body.lines().any(|line| line == MARKER))
}

fn save_state(path: &Path, value: Value) -> Result<()> {
    let parent = path.parent().ok_or("invalid state path")?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(&serde_json::to_vec(&value)?)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .map_err(|_| "cannot publish app state")?;
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

async fn operations(
    api: &Api,
    token: &str,
    slug: &str,
    installation: u64,
    state: &Path,
) -> Result<u64> {
    let previous: Value = if state.exists() {
        serde_json::from_slice(&fs::read(state)?)?
    } else {
        json!({})
    };
    let mut found = std::collections::BTreeMap::new();
    // List results can lag a successful creation. Read the persisted issue by
    // number before consulting the list; a stale list cannot authorize a POST.
    if let Some(number) = previous["operations_issue"].as_u64() {
        let issue = api
            .get(
                &format!("repos/RobTand/trustladderiii/issues/{number}"),
                token,
            )
            .await?;
        if !operations_match(&issue) {
            return Err("saved operations issue no longer matches; reconcile manually".into());
        }
        found.insert(number, issue);
    }
    let mut page = 1;
    loop {
        let rows = api
            .get(
                &format!("repos/RobTand/trustladderiii/issues?state=all&per_page=100&page={page}"),
                token,
            )
            .await?;
        let rows = rows.as_array().ok_or("invalid issues")?;
        for row in rows {
            if operations_match(row) {
                found.insert(
                    row["number"].as_u64().ok_or("invalid issue number")?,
                    row.clone(),
                );
            }
        }
        if rows.len() < 100 {
            break;
        }
        page += 1;
    }
    let number = if let Some(number) = found.keys().next() {
        *number
    } else {
        if previous["creation_pending"] == true {
            return Err(
                "operations creation outcome unknown; rerun discovery, never blindly retry POST"
                    .into(),
            );
        }
        save_state(
            state,
            json!({"installation_id": installation, "creation_pending": true}),
        )?;
        let creation = api.request(Method::POST, "repos/RobTand/trustladderiii/issues", token,
            Some(json!({"title": TITLE, "body": format!("{MARKER}\n\nTL3 posts incidents here. Questions about a request stay on that request's issue.")})))
            .await;
        if let Err(error) = &creation {
            if error
                .downcast_ref::<HttpFailure>()
                .is_some_and(|error| (400..500).contains(&error.0))
            {
                save_state(state, json!({"installation_id": installation}))?;
            }
        }
        creation?["number"]
            .as_u64()
            .ok_or("missing new issue number")?
    };
    if found.len() > 1
        && found
            .values()
            .any(|row| row["user"]["login"] != format!("{slug}[bot]"))
    {
        return Err("multiple operations issues include another author; reconcile manually".into());
    }
    save_state(
        state,
        json!({"installation_id": installation, "operations_issue": number}),
    )?;
    for duplicate in found.keys().filter(|other| **other != number) {
        api.request(
            Method::PATCH,
            &format!("repos/RobTand/trustladderiii/issues/{duplicate}"),
            token,
            Some(json!({"state": "closed", "state_reason": "not_planned",
                "title": format!("Duplicate of TL3 operations #{number}"),
                "body": format!("<!-- tl3:operations:duplicate:v1 -->\n\nSee #{number}.")})),
        )
        .await?;
    }
    Ok(number)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    fn issue(number: u64) -> Value {
        json!({"number": number, "title": TITLE, "body": format!("{MARKER}\n\nOperations"), "user": {"login": "tl3-robtand[bot]"}})
    }

    #[tokio::test]
    async fn stale_list_cannot_duplicate_a_saved_creation() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/RobTand/trustladderiii/issues/81"))
            .respond_with(ResponseTemplate::new(200).set_body_json(issue(81)))
            .expect(2)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/RobTand/trustladderiii/issues"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .expect(2)
            .mount(&server)
            .await;
        let api = Api {
            http: Client::new(),
            root: format!("{}/", server.uri()),
        };
        let scratch = tempfile::tempdir().unwrap();
        let state = scratch.path().join("state.json");
        save_state(&state, json!({"operations_issue":81})).unwrap();
        for _ in 0..2 {
            assert_eq!(
                operations(&api, "fixture", "tl3-robtand", 1, &state)
                    .await
                    .unwrap(),
                81
            );
        }
    }

    #[tokio::test]
    async fn uncertain_creation_never_blindly_posts_again() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/RobTand/trustladderiii/issues"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .expect(2)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/repos/RobTand/trustladderiii/issues"))
            .respond_with(ResponseTemplate::new(503))
            .expect(1)
            .mount(&server)
            .await;
        let api = Api {
            http: Client::new(),
            root: format!("{}/", server.uri()),
        };
        let scratch = tempfile::tempdir().unwrap();
        let state = scratch.path().join("state.json");
        for _ in 0..2 {
            assert!(
                operations(&api, "fixture", "tl3-robtand", 1, &state)
                    .await
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn rejected_creation_can_retry_after_permissions_are_fixed() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/RobTand/trustladderiii/issues"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .mount(&server)
            .await;
        let rejected = Mock::given(method("POST"))
            .and(path("/repos/RobTand/trustladderiii/issues"))
            .respond_with(ResponseTemplate::new(403))
            .expect(1)
            .mount_as_scoped(&server)
            .await;
        let api = Api {
            http: Client::new(),
            root: format!("{}/", server.uri()),
        };
        let scratch = tempfile::tempdir().unwrap();
        let state = scratch.path().join("state.json");
        assert!(
            operations(&api, "fixture", "tl3-robtand", 1, &state)
                .await
                .is_err()
        );
        let saved: Value = serde_json::from_slice(&fs::read(&state).unwrap()).unwrap();
        assert_ne!(saved["creation_pending"], true);
        drop(rejected);
        Mock::given(method("POST"))
            .and(path("/repos/RobTand/trustladderiii/issues"))
            .respond_with(ResponseTemplate::new(201).set_body_json(issue(81)))
            .expect(1)
            .mount(&server)
            .await;
        assert_eq!(
            operations(&api, "fixture", "tl3-robtand", 1, &state)
                .await
                .unwrap(),
            81
        );
    }

    #[tokio::test]
    async fn duplicates_keep_the_oldest_app_issue() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/RobTand/trustladderiii/issues"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([issue(82), issue(81)])))
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/repos/RobTand/trustladderiii/issues/82"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .expect(1)
            .mount(&server)
            .await;
        let api = Api {
            http: Client::new(),
            root: format!("{}/", server.uri()),
        };
        let scratch = tempfile::tempdir().unwrap();
        let state = scratch.path().join("state.json");
        assert_eq!(
            operations(&api, "fixture", "tl3-robtand", 1, &state)
                .await
                .unwrap(),
            81
        );
    }
}
