#![allow(dead_code, unused_imports)] // Verbatim pre-extraction functions retain their APIs.
use crate::{Error, Repository, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use tl3_controller::adapters::github::writer_input;
use tl3_controller::adapters::workers::writer::{
    MessageKind, TextDraft, WriteRequest, Writer, WriterReply, checked_draft, fixed_question,
};
use tl3_controller::executor::{
    CommentKind, Completion, Delivery, Observation, Output as Out, Reconciliation,
};
use tl3_github::{CloseReason, message::Policy};
use tl3_kernel::{EffectKind, Notice, Outcome};
mod io;
#[path = "worker_inputs.rs"]
pub mod worker_inputs;
use io::{GitHub, Path, PathBuf, Runtime};
mod records {
    use super::io::mock_std as std;
    include!("records.rs");
}
use records::Records;
#[path = "evidence.rs"]
pub mod evidence;
#[path = "goals.rs"]
pub mod goals;
#[path = "pr_body.rs"]
pub mod pr_body;
#[path = "failure.rs"]
pub mod publication_failure;
include!("message.rs");
pub mod executor {
    pub use tl3_controller::executor::*;
}
pub fn decode_stored<T: serde::de::DeserializeOwned>(value: Value) -> Result<T> {
    Ok(serde_json::from_value(value)?)
}
pub mod approval_delivery {
    use super::*;
    pub fn is_approval(d: &Delivery) -> bool {
        matches!(
            d.output,
            Out::Effect {
                kind: EffectKind::PostApproval,
                ..
            }
        )
    }
}
pub mod question_findings {
    use tl3_kernel::QKind;
    pub fn fixed_decision(kind: QKind) -> Option<&'static str> {
        match kind {
            QKind::AuditDisagreement => Some(
                "Should I go ahead with this reading of your request even though its independent check still has open objections?",
            ),
            QKind::NeedsSplitting => Some("How would you like to narrow the request?"),
            QKind::CantMeet => {
                Some("What should I change about the approach so the request can be met?")
            }
            QKind::OutOfCredit => {
                Some("Should I spend more of the request's allowance on another attempt?")
            }
            QKind::EffectFailed => Some("Can you fix the cause so that I can continue?"),
            QKind::CleanupFailed => None,
        }
    }
}
struct Branches;
impl Branches {
    fn branch(&self, _: &Delivery) -> Result<tl3_worktrees::BranchName> {
        Ok(tl3_worktrees::BranchName::new("topic")?)
    }
    fn base(&self, _: &Repository) -> Result<&str> {
        Ok("main")
    }
    fn matches(&self, _: &Delivery, branch: &str) -> Result<bool> {
        Ok(branch == "topic")
    }
}
macro_rules! adapter {
    () => {
        struct Adapter {
            clients: BTreeMap<u64, GitHub>,
            records: Records,
            runtime: Runtime,
            epoch: i64,
            branches: Branches,
            writer: Box<dyn Writer>,
            operations: Option<tl3_config::OperationsIssue>,
            policy: Policy,
            body_lines: Vec<(Repository, Vec<tl3_config::BodyLine>, Vec<String>)>,
        }
        impl Adapter {
            fn new() -> Self {
                Self {
                    clients: [(3, GitHub::new())].into(),
                    records: Records::new(&PathBuf::from("root")),
                    runtime: Runtime,
                    epoch: io::owner_epoch(),
                    branches: Branches,
                    writer: Box::new(io::RecordedWriter),
                    policy: Policy::default(),
                    operations: io::operations(),
                    body_lines: vec![(
                        Repository {
                            owner: "owner".into(),
                            name: "sandbox".into(),
                            installation: 3,
                        },
                        vec![tl3_config::BodyLine {
                            label: "Change type".into(),
                            values: vec!["fix".into()],
                            default: "fix".into(),
                        }],
                        vec![],
                    )],
                }
            }
        }
    };
}
macro_rules! entry {
    ($a:ident, $e:ident, $d:ident) => {
        match $e {
            io::Entry::Reconcile => io::reconciliation($a.reconcile($d)),
            io::Entry::Publish => io::completed($a.publish($d)),
            io::Entry::Execute => io::completed($a.execute_delivery($d)),
            io::Entry::Repair => match $a.repair_approval_copies($d) { Ok((r,n))=>json!({"result":io::reconciliation(Ok(r)),"copies":n}), Err(e)=>json!({"error":e.to_string()}) },
        }
    }
}
mod reference {
    adapter!();
    include!("reference.rs");
    mod publishing {
        use crate::publication_io::io::mock_std as std;
        include!("reference-publishing.rs");
    }
    pub(super) fn run(e: io::Entry, d: &Delivery) -> Value {
        let mut a = Adapter::new();
        entry!(a, e, d)
    }
}
mod current {
    adapter!();
    include!("current.rs");
    mod publishing {
        use crate::publication_io::io::mock_std as std;
        include!("current-publishing.rs");
    }
    pub(super) fn run(e: io::Entry, d: &Delivery) -> Value {
        let mut a = Adapter::new();
        let _ = (&mut a, e, d);
        json!("E1 extraction stub")
    }
}
#[path = "properties.rs"]
mod properties;
