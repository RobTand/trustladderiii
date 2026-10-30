use crate::*;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Context {
    pub repository: Repository,
    pub installation_id: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum Event {
    IssueLabeled {
        context: Context,
        issue: Issue,
        label: Label,
        actor: Actor,
    },
    CommentCreated {
        context: Context,
        issue: Issue,
        comment: Comment,
        actor: Actor,
    },
    PullRequestClosed {
        context: Context,
        pull_request: PullRequest,
        actor: Actor,
    },
    PullRequestMerged {
        context: Context,
        pull_request: PullRequest,
        actor: Actor,
    },
    PullRequestSynchronized {
        context: Context,
        pull_request: PullRequest,
        before: String,
        after: String,
        actor: Actor,
    },
    PullRequestUnlabeled {
        context: Context,
        pull_request: PullRequest,
        label: Label,
        actor: Actor,
    },
    MergeGroup {
        context: Context,
        merge_group: MergeGroup,
    },
    CheckRunCompleted {
        context: Context,
        check_run: CheckRun,
    },
    CheckSuiteCompleted {
        context: Context,
        check_suite: CheckSuite,
    },
}

impl Event {
    /// The controller still checks the served repository and installation.
    pub fn starts_request(&self, rob_user_id: u64) -> bool {
        matches!(self, Self::IssueLabeled { issue, label, actor, .. }
            if issue.pull_request.is_none() && label.name == "tl3" && actor.id == rob_user_id)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Delivery {
    pub id: String,
    pub event: Option<Event>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WebhookError {
    #[error("invalid webhook signature")]
    Signature,
    #[error("invalid webhook delivery id")]
    DeliveryId,
    #[error("invalid webhook payload")]
    Payload,
}

/// Implement with an atomic unique insert of both the delivery id and event.
/// A failed insert must leave neither committed. A duplicate returns false.
pub trait Inbox {
    type Error;
    fn insert(&mut self, delivery: Delivery) -> Result<bool, Self::Error>;
}

#[derive(Debug, thiserror::Error)]
pub enum ReceiveError<E> {
    #[error(transparent)]
    Webhook(#[from] WebhookError),
    #[error("could not commit webhook to inbox")]
    Inbox(E),
}

pub fn receive<I: Inbox>(
    secret: &[u8],
    signature: &str,
    delivery_id: &str,
    event_name: &str,
    body: &[u8],
    inbox: &mut I,
) -> Result<bool, ReceiveError<I::Error>> {
    let delivery = verify_and_parse(secret, signature, delivery_id, event_name, body)?;
    inbox.insert(delivery).map_err(ReceiveError::Inbox)
}

pub fn verify_and_parse(
    secret: &[u8],
    signature: &str,
    delivery_id: &str,
    event_name: &str,
    body: &[u8],
) -> Result<Delivery, WebhookError> {
    if secret.is_empty() {
        return Err(WebhookError::Signature);
    }
    let signature = signature
        .strip_prefix("sha256=")
        .ok_or(WebhookError::Signature)?;
    let signature = hex::decode(signature).map_err(|_| WebhookError::Signature)?;
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).map_err(|_| WebhookError::Signature)?;
    mac.update(body);
    mac.verify_slice(&signature)
        .map_err(|_| WebhookError::Signature)?;
    if delivery_id.is_empty()
        || delivery_id.len() > 128
        || !delivery_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err(WebhookError::DeliveryId);
    }
    let value: serde_json::Value =
        serde_json::from_slice(body).map_err(|_| WebhookError::Payload)?;
    if !value.is_object() {
        return Err(WebhookError::Payload);
    }
    let action = value.get("action").and_then(|v| v.as_str()).unwrap_or("");
    let relevant = matches!(
        (event_name, action),
        ("issues", "labeled")
            | ("issue_comment", "created")
            | ("pull_request", "closed" | "synchronize" | "unlabeled")
            | ("merge_group", "checks_requested" | "destroyed")
            | ("check_run" | "check_suite", "completed")
    );
    if !relevant {
        return Ok(Delivery {
            id: delivery_id.into(),
            event: None,
        });
    }
    #[derive(Deserialize)]
    struct Installation {
        id: u64,
    }
    #[derive(Deserialize)]
    struct Common {
        repository: Repository,
        installation: Installation,
    }
    let common: Common =
        serde_json::from_value(value.clone()).map_err(|_| WebhookError::Payload)?;
    let context = Context {
        repository: common.repository,
        installation_id: common.installation.id,
    };
    fn field<T: serde::de::DeserializeOwned>(
        value: &serde_json::Value,
        name: &str,
    ) -> Result<T, WebhookError> {
        serde_json::from_value(value.get(name).cloned().ok_or(WebhookError::Payload)?)
            .map_err(|_| WebhookError::Payload)
    }
    let event = match (event_name, action) {
        ("issues", "labeled") => Event::IssueLabeled {
            context,
            issue: field(&value, "issue")?,
            label: field(&value, "label")?,
            actor: field(&value, "sender")?,
        },
        ("issue_comment", "created") => Event::CommentCreated {
            context,
            issue: field(&value, "issue")?,
            comment: field(&value, "comment")?,
            actor: field(&value, "sender")?,
        },
        ("pull_request", "closed") => {
            let pull_request = field(&value, "pull_request")?;
            let actor = field(&value, "sender")?;
            if value["pull_request"]["merged"]
                .as_bool()
                .ok_or(WebhookError::Payload)?
            {
                Event::PullRequestMerged {
                    context,
                    pull_request,
                    actor,
                }
            } else {
                Event::PullRequestClosed {
                    context,
                    pull_request,
                    actor,
                }
            }
        }
        ("pull_request", "synchronize") => Event::PullRequestSynchronized {
            context,
            pull_request: field(&value, "pull_request")?,
            before: field(&value, "before")?,
            after: field(&value, "after")?,
            actor: field(&value, "sender")?,
        },
        ("pull_request", "unlabeled") => Event::PullRequestUnlabeled {
            context,
            pull_request: field(&value, "pull_request")?,
            label: field(&value, "label")?,
            actor: field(&value, "sender")?,
        },
        ("merge_group", _) => Event::MergeGroup {
            context,
            merge_group: field(&value, "merge_group")?,
        },
        ("check_run", "completed") => Event::CheckRunCompleted {
            context,
            check_run: field(&value, "check_run")?,
        },
        ("check_suite", "completed") => Event::CheckSuiteCompleted {
            context,
            check_suite: field(&value, "check_suite")?,
        },
        _ => unreachable!(),
    };
    Ok(Delivery {
        id: delivery_id.into(),
        event: Some(event),
    })
}
