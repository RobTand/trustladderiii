use super::*;
use rand::rngs::OsRng;
use rsa::{
    RsaPrivateKey,
    pkcs8::{EncodePrivateKey, LineEnding},
};
use serde_json::json;
use std::{process::Command, time::Duration};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

#[test]
fn github_installation_and_synchronous_token_bridge() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let server = runtime.block_on(MockServer::start());
    let pem = RsaPrivateKey::new(&mut OsRng, 2048)
        .unwrap()
        .to_pkcs8_pem(LineEnding::LF)
        .unwrap()
        .to_string();
    let github = tl3_github::GitHub::new(
        tl3_github::AppConfig {
            id: 123,
            slug: "fixture".into(),
            pem,
            webhook_secret: "unused".into(),
        },
        1,
        tl3_github::ClientOptions {
            api_url: format!("{}/", server.uri()).parse().unwrap(),
            max_retries: 0,
            mutation_interval: Duration::ZERO,
            ..Default::default()
        },
    )
    .unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let repo = scratch.path().join("repo.git");
    assert!(
        Command::new("git")
            .args(["init", "--bare"])
            .arg(&repo)
            .output()
            .unwrap()
            .status
            .success()
    );
    let identity = Repository {
        owner: "owner".into(),
        name: "repo".into(),
        installation: 1,
    };
    let mut adapter = Real::new(scratch.path().join("worktrees"), 1).unwrap();
    let mut mismatch = identity.clone();
    mismatch.installation = 2;
    assert!(matches!(
        adapter.serve_github(mismatch, &repo, github.clone()),
        Err(Error::Invalid("worktree GitHub installation mismatch"))
    ));
    assert!(adapter.repositories.is_empty());
    assert!(
        runtime
            .block_on(server.received_requests())
            .unwrap()
            .is_empty()
    );
    runtime.block_on(
        Mock::given(method("POST"))
            .and(path("/app/installations/1/access_tokens"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "token": "fake-installation-token",
                "expires_at": "2099-01-01T00:00:00Z"
            })))
            .expect(1)
            .mount(&server),
    );
    adapter.serve_github(identity, &repo, github).unwrap();
    let served = &adapter.repositories[0];
    // Calls the adapter-owned runtime from the same synchronous context as execute.
    assert!(served.token().unwrap().is_some());
    assert!(served.token().unwrap().is_some());
    assert_eq!(
        runtime.block_on(server.received_requests()).unwrap().len(),
        1
    );
    runtime.block_on(server.verify());
}

#[test]
fn prepare_records_accept_legacy_ownership_fields() {
    for fields in [
        json!({}),
        json!({"branch_existed": false, "checkout_existed": false}),
    ] {
        let mut prepare = fields;
        prepare["base"] = json!("a".repeat(40));
        let intent: Intent = serde_json::from_value(json!({"Prepare": prepare})).unwrap();
        assert_eq!(
            serde_json::to_value(intent).unwrap(),
            json!({"Prepare": {"base": "a".repeat(40)}})
        );
    }
}

#[test]
fn worktree_error_classes_distinguish_permanent_failures() {
    use tl3_worktrees::Error as GitError;
    for (error, class) in [
        (GitError::BranchName, "invalid_branch"),
        (GitError::CommitId, "invalid_commit"),
        (GitError::Remote, "invalid_remote"),
        (GitError::WorktreePath, "invalid_worktree_path"),
        (GitError::Token, "invalid_token"),
        (GitError::Response, "invalid_response"),
        (GitError::MissingBranch, "missing_branch"),
        (GitError::MainBranch, "reserved_branch"),
        (GitError::WorktreeOccupied, "worktree_occupied"),
        (GitError::DirtyWorktree, "dirty_worktree"),
        (GitError::Recovery, "worktree_recovery"),
    ] {
        let error = adapter(error);
        assert!(matches!(error, Error::Permanent { .. }));
        assert_eq!(crate::looks::failure_class(&error), class);
    }
    for error in [
        GitError::Io,
        GitError::Spawn,
        GitError::Poisoned,
        GitError::Git {
            operation: "fetch",
            code: Some(128),
        },
    ] {
        let error = adapter(error);
        assert!(matches!(error, Error::Adapter(_)));
        assert_eq!(crate::looks::failure_class(&error), "transport");
    }
}
