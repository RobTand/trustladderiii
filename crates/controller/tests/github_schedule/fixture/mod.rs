//! One recorded I/O boundary for the verbatim old and production shell bodies.
use serde_json::Value;
pub use serde_json::json;
use std::cell::RefCell;
pub use std::collections::BTreeMap;
pub use tl3_controller::adapters::workers::writer::{MessageKind, WriteRequest};
pub use tl3_controller::executor::Output as Out;
pub use tl3_controller::executor::{
    CommentKind, Completion, Delivery, Observation, Reconciliation,
};
pub use tl3_controller::{Error, Result};
pub use tl3_github::{
    CloseReason,
    message::{OptionDraft, Policy},
};
pub use tl3_kernel::{EffectKind, NoteKind, Notice, Outcome, QKind};
mod records;
pub use records::Records;
mod filesystem;

#[derive(Default, Debug, PartialEq)]
pub struct State {
    pub calls: Vec<Value>,
    pub files: BTreeMap<String, Vec<u8>>,
    pub writes: usize,
    pub crashed: bool,
    pub reads: usize,
    pub values: Vec<u16>,
    pub crash: Option<(usize, bool)>,
}
thread_local! { pub static IO:RefCell<State> = RefCell::new(State::default()); }
pub fn sample(call: Value) -> u16 {
    IO.with(|io| {
        let mut io = io.borrow_mut();
        io.calls.push(call);
        let value = io.values[io.reads % io.values.len()];
        io.reads += 1;
        value
    })
}
pub fn write(call: Value, key: String, bytes: Option<Vec<u8>>) -> std::io::Result<()> {
    IO.with(|io| {
        let mut io = io.borrow_mut();
        io.calls.push(call);
        let index = io.writes;
        io.writes += 1;
        if io.crashed || io.crash == Some((index, false)) {
            io.crashed = true;
            return Err(std::io::Error::other("scripted crash"));
        }
        if let Some(bytes) = bytes {
            io.files.insert(key, bytes);
        }
        if io.crash == Some((index, true)) {
            io.crashed = true;
            return Err(std::io::Error::other("scripted crash after write"));
        }
        Ok(())
    })
}
pub struct Shell<const OLD: bool> {
    pub records: Records,
    pub epoch: i64,
    pub clients: BTreeMap<u64, GitHub>,
    pub runtime: Runtime,
    pub policy: Policy,
    pub operations: Option<tl3_config::OperationsIssue>,
    pub writer: Box<Writer>,
}
pub struct Runtime;
impl Runtime {
    pub fn block_on<T>(&self, result: T) -> T {
        result
    }
}
pub struct Writer;
#[derive(Clone)]
pub struct GitHub;
impl GitHub {
    pub fn reconcile_comments(
        &self,
        repo: &str,
        issue: u64,
        key: &str,
    ) -> std::result::Result<Option<tl3_github::Comment>, tl3_github::Error> {
        let value = sample(json!(["list_comments", repo, issue, key]));
        if value % 5 == 4 {
            return Err(tl3_github::Error::Transport);
        }
        // The real list/sort/delete path has its own HTTP differential test.
        if value % 5 == 0 {
            return Ok(None);
        }
        if value % 5 == 3 {
            self.delete(repo, "duplicate")?;
        }
        Ok(Some(
            serde_json::from_value(super::comment(1, true, true)).unwrap(),
        ))
    }
    fn delete(&self, repo: &str, id: &str) -> std::result::Result<(), tl3_github::Error> {
        write(
            json!(["delete_comment", repo, id]),
            format!("remote/{id}"),
            Some(vec![]),
        )
        .map_err(|_| tl3_github::Error::Transport)
    }
    pub fn post_comment(
        &self,
        repo: &str,
        issue: u64,
        key: &str,
        body: &str,
    ) -> std::result::Result<(), tl3_github::Error> {
        if self.reconcile_comments(repo, issue, key)?.is_some() {
            return Ok(());
        }
        write(
            json!(["post_comment", repo, issue, key, body]),
            format!("remote/comment/{key}"),
            Some(body.as_bytes().to_vec()),
        )
        .map_err(|_| tl3_github::Error::Transport)
    }
    pub fn issue(
        &self,
        repo: &str,
        number: u64,
    ) -> std::result::Result<tl3_github::Issue, tl3_github::Error> {
        let value = sample(json!(["issue", repo, number]));
        if value % 5 == 4 {
            return Err(tl3_github::Error::Transport);
        }
        Ok(serde_json::from_value(json!({"number":number,"title":format!("Request {value}"),"body":format!("Text {value}"),
            "state":if value%2==0 {"open"} else {"closed"},"state_reason":null,"updated_at":"2026-10-30T00:00:00Z"})).unwrap())
    }
    pub fn close_issue(
        &self,
        repo: &str,
        number: u64,
        reason: CloseReason,
    ) -> std::result::Result<(), tl3_github::Error> {
        write(
            json!(["close_issue", repo, number, reason]),
            format!("remote/issue/{number}"),
            Some(serde_json::to_vec(&reason).unwrap()),
        )
        .map_err(|_| tl3_github::Error::Transport)
    }
    pub fn create_request_issue(
        &self,
        repo: &str,
        key: &str,
        since: &str,
        title: &str,
        body: &str,
    ) -> std::result::Result<tl3_github::Issue, tl3_github::Error> {
        if let Some(issue) = self.reconcile_issue(repo, key, since)? {
            return Ok(issue);
        }
        write(
            json!(["create_issue", repo, key, title, body]),
            format!("remote/issue/{key}"),
            Some(body.as_bytes().to_vec()),
        )
        .map_err(|_| tl3_github::Error::Transport)?;
        self.issue(repo, 18)
    }
    pub fn reconcile_issue(
        &self,
        repo: &str,
        key: &str,
        since: &str,
    ) -> std::result::Result<Option<tl3_github::Issue>, tl3_github::Error> {
        let value = sample(json!(["reconcile_issue", repo, key, since]));
        match value % 3 {
            0 => Ok(None),
            1 => self.issue(repo, 18).map(Some),
            _ => Err(tl3_github::Error::Transport),
        }
    }
    pub fn app_slug(&self) -> &str {
        "factory"
    }
    pub fn issue_events(
        &self,
        repo: &str,
        issue: u64,
    ) -> std::result::Result<Vec<tl3_github::IssueActivity>, tl3_github::Error> {
        let value = sample(json!(["issue_events", repo, issue]));
        Ok(vec![serde_json::from_value(json!({"id":1,"event":"closed","actor":{"id":99,"login":if value%2==0 {"factory[bot]"} else {"human"}},
            "created_at":"2026-10-30T00:00:00Z","label":null})).unwrap()])
    }
    pub fn reopen_issue(
        &self,
        repo: &str,
        issue: u64,
    ) -> std::result::Result<tl3_github::Issue, tl3_github::Error> {
        write(
            json!(["reopen_issue", repo, issue]),
            format!("remote/issue/{issue}"),
            Some(b"open".to_vec()),
        )
        .map_err(|_| tl3_github::Error::Transport)?;
        self.issue(repo, issue)
    }
    pub fn hook_deliveries(
        &self,
        cursor: Option<&str>,
    ) -> std::result::Result<tl3_github::HookDeliveryPage, tl3_github::Error> {
        let value = sample(json!(["hook_deliveries", cursor]));
        if value % 7 == 6 {
            return Err(tl3_github::Error::Transport);
        }
        let mut deliveries = vec![];
        for i in 0..3 {
            let id = u64::from(value % 30) + i;
            deliveries.push(tl3_github::HookDelivery {
                id,
                status_code: match (value + i as u16) % 3 {
                    0 => None,
                    1 => Some(200),
                    _ => Some(503),
                },
                delivered_at: chrono::DateTime::from_timestamp_millis(i as i64).unwrap(),
            });
        }
        Ok(tl3_github::HookDeliveryPage {
            deliveries,
            next_cursor: (cursor.is_none() && value % 2 == 1).then(|| "next".into()),
        })
    }
    pub fn redeliver_hook(&self, id: u64) -> std::result::Result<(), tl3_github::Error> {
        write(
            json!(["redeliver", id]),
            format!("remote/hook/{id}"),
            Some(b"redelivered".to_vec()),
        )
        .map_err(|_| tl3_github::Error::Transport)
    }
}
pub mod publishing {
    use super::*;
    pub fn handles(delivery: &Delivery) -> bool {
        matches!(
            delivery.output,
            Out::Effect {
                kind: EffectKind::PostApproval | EffectKind::OpenPr | EffectKind::ClosePr,
                ..
            }
        )
    }
}
impl<const OLD: bool> Shell<OLD> {
    pub fn new() -> Self {
        Self {
            records: Records::new(&filesystem::VPath("records".into())),
            epoch: 1,
            clients: BTreeMap::from([(3, GitHub)]),
            runtime: Runtime,
            policy: Policy::default(),
            operations: Some(tl3_config::OperationsIssue {
                owner: "owner".into(),
                name: "sandbox".into(),
                installation_id: 3,
                number: 12,
            }),
            writer: Box::new(Writer),
        }
    }
    pub fn reconcile_publication(&self, _delivery: &Delivery) -> Result<Reconciliation> {
        match sample(json!(["publication"])) % 6 {
            0 => Ok(Reconciliation::Found(Box::new(Completion::confirmed(
                vec![],
            )))),
            1 => Ok(Reconciliation::Absent),
            2 => Ok(Reconciliation::Pending),
            3 => Err(Error::Permanent {
                class: "pr_evidence",
                source: Box::new(std::io::Error::other("scripted permanent")),
            }),
            4 => Err(Error::Permanent {
                class: "body_policy",
                source: Box::new(std::io::Error::other("scripted permanent")),
            }),
            _ => Err(Error::Invalid("scripted transient")),
        }
    }
    pub fn publish(&mut self, delivery: &Delivery) -> Result<Completion> {
        write(
            json!(["publish", delivery.key]),
            "remote/publication".into(),
            Some(b"published".to_vec()),
        )?;
        Ok(Completion::confirmed(vec![]))
    }
    pub fn close_original(&self, delivery: &Delivery) -> Result<()> {
        if delivery.proposed.is_some() {
            write(
                json!(["close_original", delivery.key]),
                "remote/original".into(),
                Some(b"closed".to_vec()),
            )?;
        }
        Ok(())
    }
}
pub fn question_draft(
    _writer: &mut Writer,
    delivery: &Delivery,
    input: WriteRequest,
    policy: &Policy,
) -> Result<String> {
    let attribution = input
        .request
        .as_ref()
        .map(|r| json!([r.node, r.generation, r.repo, r.issue, r.attempt, r.round]));
    let value = sample(json!([
        "writer",
        delivery.key,
        input,
        attribution,
        policy.request_title,
        policy.request_body,
        policy.own_repository,
        policy.own_issue,
        policy.own_pull_request,
        policy.internal_terms,
        policy.known_paths
    ]));
    match value % 5 {
        3 => Err(Error::MessageRejected),
        4 => Err(Error::Invalid("writer failed")),
        _ => Ok(format!("Request draft {value}")),
    }
}

pub fn fixed_notice_text(delivery: &Delivery) -> Result<Option<String>> {
    Ok(delivery.note.clone())
}
