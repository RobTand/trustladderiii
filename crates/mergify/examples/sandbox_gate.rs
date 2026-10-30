//! Live sandbox operations. Credentials and installation tokens stay in process.
use chrono::Utc;
use serde_json::json;
use std::{fs, path::PathBuf};
use tl3_github::{
    AppConfig, CheckConclusion, CheckStatus, ClientOptions, CloseReason, GitHub, NewCheckRun,
    NewPullRequest,
};
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
    if operation == "deliveries" {
        let destination = PathBuf::from(subject);
        fs::create_dir_all(&destination)?;
        let mut url = "https://api.github.com/app/hook/deliveries?per_page=100".to_owned();
        for page in 1..=20 {
            let response = http
                .get(&url)
                .header("User-Agent", "tl3-sandbox-gate")
                .header("X-GitHub-Api-Version", tl3_github::API_VERSION)
                .bearer_auth(&jwt)
                .send()
                .await?;
            let next = response
                .headers()
                .get(reqwest::header::LINK)
                .and_then(|value| value.to_str().ok())
                .and_then(|links| links.split(',').find(|link| link.contains("rel=\"next\"")))
                .and_then(|link| {
                    link.split_once('<')?
                        .1
                        .split_once('>')
                        .map(|(url, _)| url.to_owned())
                });
            let status = response.status();
            let body: serde_json::Value = response.json().await?;
            if !status.is_success() {
                let record = json!({"status": status.as_u16(), "message": body["message"], "errors": body["errors"]});
                fs::write(
                    destination.join("unavailable.json"),
                    serde_json::to_vec_pretty(&record)?,
                )?;
                println!("delivery API unavailable: {record}");
                return Err("delivery API unavailable".into());
            }
            let deliveries: Vec<serde_json::Value> = serde_json::from_value(body)?;
            fs::write(
                destination.join(format!("page-{page}.json")),
                serde_json::to_vec_pretty(&deliveries)?,
            )?;
            for delivery in &deliveries {
                if !matches!(
                    delivery["event"].as_str(),
                    Some("check_run" | "pull_request" | "issues")
                ) {
                    continue;
                }
                let id = delivery["id"].as_u64().ok_or("delivery ID missing")?;
                let path = destination.join(format!("{id}.json"));
                if path.exists() {
                    continue;
                }
                let detail: serde_json::Value = http
                    .get(format!("https://api.github.com/app/hook/deliveries/{id}"))
                    .header("User-Agent", "tl3-sandbox-gate")
                    .header("X-GitHub-Api-Version", tl3_github::API_VERSION)
                    .bearer_auth(&jwt)
                    .send()
                    .await?
                    .error_for_status()?
                    .json()
                    .await?;
                let payload = &detail["request"]["payload"];
                if payload["repository"]["full_name"] != "RobTand/tl3-sandbox" {
                    continue;
                }
                fs::write(
                    path,
                    serde_json::to_vec_pretty(&json!({
                        "id": id, "event": delivery["event"], "action": delivery["action"],
                        "delivered_at": delivery["delivered_at"],
                        "status_code": delivery["status_code"], "payload": payload
                    }))?,
                )?;
                println!("recorded delivery {id}");
            }
            match next {
                Some(next) if next.starts_with("https://api.github.com/app/hook/deliveries?") => {
                    url = next
                }
                None => return Ok(()),
                Some(_) => return Err("unexpected delivery pagination URL".into()),
            }
        }
        return Err("delivery pagination limit reached".into());
    }
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
        "open" => {
            let request = trial_pull_request(
                subject,
                head.ok_or("title required")?,
                args.next().ok_or("trial body required")?,
            );
            let pull = github.open_pull_request(repo, &request).await?;
            println!("sandbox PR {}", pull.number);
        }
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
        "close-issue" => {
            github
                .close_issue(repo, subject.parse()?, CloseReason::NotPlanned)
                .await?;
            println!("trial issue closed");
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
    if let Err(error) = run().await {
        eprintln!("Sandbox operation failed: {error}. Inspect REST state before retrying.");
        std::process::exit(1);
    }
}

fn trial_pull_request(branch: String, title: String, body: String) -> NewPullRequest {
    NewPullRequest {
        head: branch,
        base: "main".into(),
        title,
        body,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_request_preserves_the_callers_trial_description() {
        let request = trial_pull_request(
            "tl3/issue-17-node-201".into(),
            "Verify the approved head while main advances".into(),
            "Merge a separate approved PR, then confirm this head is unchanged.".into(),
        );
        assert_eq!(request.head, "tl3/issue-17-node-201");
        assert_eq!(request.base, "main");
        assert_eq!(
            request.title,
            "Verify the approved head while main advances"
        );
        assert_eq!(
            request.body,
            "Merge a separate approved PR, then confirm this head is unchanged."
        );
    }
}
