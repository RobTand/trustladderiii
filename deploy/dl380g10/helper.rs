//! Deployment-only REST operations. Credentials never leave this process.
use reqwest::{Client, Method};
use serde_json::{Value, json};
use std::{fs, path::Path, time::Duration};
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
}

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
            .request(method, format!("https://api.github.com/{path}"))
            .bearer_auth(token)
            .header("User-Agent", "tl3-deploy")
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", tl3_github::API_VERSION);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .await
            .map_err(|_| "GitHub transport failed; reconcile before retrying")?;
        if !response.status().is_success() {
            return Err(format!("GitHub HTTP {}", response.status().as_u16()).into());
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
        http: Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|_| "cannot build GitHub client")?,
    };
    let info = api.get("app", &jwt).await?;
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
        let mut found = Vec::new();
        let mut page = 1;
        loop {
            let rows = api
                .get(
                    &format!(
                        "repos/RobTand/trustladderiii/issues?state=all&per_page=100&page={page}"
                    ),
                    &token,
                )
                .await?;
            let rows = rows.as_array().ok_or("invalid issues")?;
            for row in rows {
                if row.get("pull_request").is_none()
                    && row["title"] == TITLE
                    && row["body"]
                        .as_str()
                        .is_some_and(|body| body.lines().any(|line| line == MARKER))
                {
                    found.push(row["number"].as_u64().ok_or("invalid issue number")?);
                }
            }
            if rows.len() < 100 {
                break;
            }
            page += 1;
        }
        let number = match found.as_slice() {
            [number] => *number,
            [] => api.request(Method::POST, "repos/RobTand/trustladderiii/issues", &token,
                Some(json!({"title": TITLE, "body": format!("{MARKER}\n\nTL3 posts incidents here. Questions about a request stay on that request's issue.")})))
                .await?["number"].as_u64().ok_or("missing new issue number")?,
            _ => return Err("multiple operations issues match; reconcile manually".into()),
        };
        let state = Path::new(&config.worktrees_root)
            .parent()
            .ok_or("invalid worktrees root")?
            .join("app-state.json");
        fs::write(
            state,
            serde_json::to_vec(
                &json!({"installation_id": installation, "operations_issue": number}),
            )?,
        )?;
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
