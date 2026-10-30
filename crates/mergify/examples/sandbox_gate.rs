//! Live sandbox operations. Credentials and installation tokens stay in process.
use chrono::Utc;
use serde_json::json;
use std::{fs, path::PathBuf};
use tl3_github::{AppConfig, CheckConclusion, CheckStatus, ClientOptions, GitHub, NewCheckRun};
use tl3_mergify::Mergify;

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let operation = args.next().ok_or("operation required")?;
    let subject = args.next().ok_or("SHA, branch, or PR required")?;
    let head = args.next();
    let path = PathBuf::from(std::env::var("HOME")?).join(".config/tl3/github-app.json");
    let app: AppConfig = serde_json::from_slice(&fs::read(path)?)?;
    if app.id != 5542171 || app.slug != "tl3-robtand" {
        return Err("unexpected sandbox App identity".into());
    }
    let now = Utc::now().timestamp();
    let jwt = jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256),
        &json!({"iat": now - 60, "exp": now + 300, "iss": app.id}),
        &jsonwebtoken::EncodingKey::from_rsa_pem(app.pem.as_bytes())?,
    )?;
    let http = reqwest::Client::new();
    let installation: serde_json::Value = http
        .get("https://api.github.com/repos/RobTand/tl3-sandbox/installation")
        .header("User-Agent", "tl3-sandbox-gate")
        .header("X-GitHub-Api-Version", tl3_github::API_VERSION)
        .bearer_auth(jwt)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let github = GitHub::new(
        app,
        installation["id"].as_u64().ok_or("installation missing")?,
        ClientOptions::default(),
    )?;
    let repo = "RobTand/tl3-sandbox";
    match operation.as_str() {
        "approve" => {
            let check = github
                .create_check_run(
                    repo,
                    &NewCheckRun {
                        name: "TL3 approval".into(),
                        head_sha: subject.clone(),
                        external_id: format!("sandbox-gate-17:{subject}"),
                        status: CheckStatus::Completed,
                        conclusion: Some(CheckConclusion::Success),
                    },
                )
                .await?;
            println!("approval check {}, head {}", check.id, check.head_sha);
        }
        "branch" => {
            if !subject.starts_with("tl3/") {
                return Err("sandbox trial branches must use tl3/".into());
            }
            let response = http
                .post("https://api.github.com/repos/RobTand/tl3-sandbox/git/refs")
                .header("User-Agent", "tl3-sandbox-gate")
                .header("X-GitHub-Api-Version", tl3_github::API_VERSION)
                .bearer_auth(github.installation_token().await?)
                .json(&json!({"ref": format!("refs/heads/{subject}"), "sha": head.ok_or("SHA required")?}))
                .send()
                .await?;
            println!("branch creation HTTP {}", response.status());
            response.error_for_status()?;
        }
        "delete" => github.delete_branch(repo, &subject).await?,
        "queue" => {
            Mergify::new(github)
                .queue(repo, subject.parse()?, &head.ok_or("head required")?)
                .await?;
            println!("queue label applied");
        }
        "dequeue" => {
            Mergify::new(github).dequeue(repo, subject.parse()?).await?;
            println!("queue label removed");
        }
        _ => return Err("unknown sandbox operation".into()),
    }
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if run().await.is_err() {
        eprintln!("Sandbox operation failed; inspect REST state before retrying.");
        std::process::exit(1);
    }
}
