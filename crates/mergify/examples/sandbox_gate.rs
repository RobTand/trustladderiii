//! Live sandbox operations. Credentials and installation tokens stay in process.
use chrono::Utc;
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
};
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
                let detail = github_json(
                    http.get(format!("https://api.github.com/app/hook/deliveries/{id}"))
                        .header("User-Agent", "tl3-sandbox-gate")
                        .header("X-GitHub-Api-Version", tl3_github::API_VERSION)
                        .bearer_auth(&jwt)
                        .send()
                        .await?,
                )
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
    let installation = github_json(
        http.get("https://api.github.com/repos/RobTand/tl3-sandbox/installation")
            .header("User-Agent", "tl3-sandbox-gate")
            .header("X-GitHub-Api-Version", tl3_github::API_VERSION)
            .bearer_auth(jwt)
            .send()
            .await?,
    )
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
                "main",
            );
            let pull = github.open_pull_request(repo, &request).await?;
            println!("sandbox PR {}", pull.number);
        }
        "tree" | "commit" => {
            let sha = post_git_object_file(
                &http,
                &github,
                "https://api.github.com",
                &operation,
                Path::new(&subject),
            )
            .await?;
            println!("{sha}");
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
            github_json(response).await?;
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

fn trial_pull_request(branch: String, title: String, body: String, _base: &str) -> NewPullRequest {
    NewPullRequest {
        head: branch,
        base: "main".into(),
        title,
        body,
    }
}

async fn github_json(
    response: reqwest::Response,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let status = response.status();
    let url = response.url().clone();
    let body = response.text().await?;
    if !status.is_success() {
        let message = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|body| body["message"].as_str().map(str::to_owned))
            .unwrap_or_else(|| "GitHub returned no JSON error message".into());
        return Err(format!("GitHub request failed ({status}) at {url}: {message}").into());
    }
    Ok(serde_json::from_str(&body)?)
}

async fn post_git_object_file(
    http: &reqwest::Client,
    github: &GitHub,
    api_url: &str,
    operation: &str,
    payload_path: &Path,
) -> Result<String, Box<dyn std::error::Error>> {
    let resource = match operation {
        "tree" => "trees",
        "commit" => "commits",
        _ => return Err("unknown Git object operation".into()),
    };
    let payload = serde_json::from_slice(&fs::read(payload_path)?)?;
    post_git_object(http, github, api_url, resource, &payload).await
}

async fn post_git_object(
    http: &reqwest::Client,
    github: &GitHub,
    api_url: &str,
    resource: &str,
    payload: &serde_json::Value,
) -> Result<String, Box<dyn std::error::Error>> {
    let object = github_json(
        http.post(format!(
            "{api_url}/repos/RobTand/tl3-sandbox/git/{resource}"
        ))
        .header("User-Agent", "tl3-sandbox-gate")
        .header("X-GitHub-Api-Version", tl3_github::API_VERSION)
        .bearer_auth(github.installation_token().await?)
        .json(payload)
        .send()
        .await?,
    )
    .await?;
    Ok(object["sha"]
        .as_str()
        .ok_or("Git object SHA missing")?
        .into())
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
            "master",
        );
        assert_eq!(request.head, "tl3/issue-17-node-201");
        assert_eq!(request.base, "master");
        assert_eq!(
            request.title,
            "Verify the approved head while main advances"
        );
        assert_eq!(
            request.body,
            "Merge a separate approved PR, then confirm this head is unchanged."
        );
    }

    async fn assert_app_git_write(operation: &str, resource: &str, payload: serde_json::Value) {
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{body_json, header, method, path},
        };

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/app/installations/3/access_tokens"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "token": "fake-app-installation-token",
                "expires_at": "2099-01-01T00:00:00Z"
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("/repos/RobTand/tl3-sandbox/git/{resource}")))
            .and(header(
                "authorization",
                "Bearer fake-app-installation-token",
            ))
            .and(header("X-GitHub-Api-Version", tl3_github::API_VERSION))
            .and(body_json(&payload))
            .respond_with(
                ResponseTemplate::new(201).set_body_json(json!({"sha": "created-object"})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let github = GitHub::new(
            AppConfig {
                id: 123,
                slug: "factory".into(),
                pem: include_str!("../tests/fixtures/throwaway-key.pem").into(),
                webhook_secret: "fake".into(),
            },
            3,
            ClientOptions {
                api_url: format!("{}/", server.uri()).parse().unwrap(),
                max_retries: 0,
                ..ClientOptions::default()
            },
        )
        .unwrap();
        let payload_path = std::env::temp_dir().join(format!(
            "tl3-sandbox-gate-{}-{}.json",
            std::process::id(),
            server.address().port()
        ));
        fs::write(&payload_path, serde_json::to_vec(&payload).unwrap()).unwrap();
        let result = post_git_object_file(
            &reqwest::Client::new(),
            &github,
            &server.uri(),
            operation,
            &payload_path,
        )
        .await;
        fs::remove_file(payload_path).unwrap();
        let sha = result.unwrap();
        assert_eq!(sha, "created-object");
        server.verify().await;
    }

    #[tokio::test]
    async fn tree_post_uses_app_installation_token_and_preserves_content() {
        assert_app_git_write("tree", "trees", json!({
            "base_tree": "original-tree",
            "tree": [{"path": "trial.txt", "mode": "100644", "type": "blob", "content": "trial\n"}]
        })).await;
    }

    #[tokio::test]
    async fn commit_post_uses_app_installation_token_and_preserves_parent() {
        assert_app_git_write(
            "commit",
            "commits",
            json!({
                "message": "Sandbox trial", "tree": "new-tree", "parents": ["original-main"]
            }),
        )
        .await;
    }
    #[tokio::test]
    async fn refused_git_write_reports_github_message() {
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{method, path},
        };
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/app/installations/3/access_tokens"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "token": "fake-app-installation-token",
                "expires_at": "2099-01-01T00:00:00Z"
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/repos/RobTand/tl3-sandbox/git/trees"))
            .respond_with(ResponseTemplate::new(422).set_body_json(json!({
                "message": "Tree does not exist"
            })))
            .expect(1)
            .mount(&server)
            .await;
        let github = GitHub::new(
            AppConfig {
                id: 123,
                slug: "factory".into(),
                pem: include_str!("../tests/fixtures/throwaway-key.pem").into(),
                webhook_secret: "fake".into(),
            },
            3,
            ClientOptions {
                api_url: format!("{}/", server.uri()).parse().unwrap(),
                max_retries: 0,
                ..ClientOptions::default()
            },
        )
        .unwrap();
        let error = post_git_object(
            &reqwest::Client::new(),
            &github,
            &server.uri(),
            "trees",
            &json!({"base_tree": "absent"}),
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(error.contains("422"), "{error}");
        assert!(
            error.contains("/repos/RobTand/tl3-sandbox/git/trees"),
            "{error}"
        );
        assert!(error.contains("Tree does not exist"), "{error}");
        server.verify().await;
    }
}
