use super::record::Records;
use crate::{
    Result,
    executor::{Completion, Delivery, Executor, Look, Observation, Reconciliation},
};
use std::path::Path;
use tl3_kernel::{Ack, EffectKind, Out};

pub struct Fake(Records);
impl Fake {
    pub fn new(root: &Path) -> Self {
        Self(Records::new(root))
    }
}
impl Executor for Fake {
    fn execute(&mut self, delivery: &Delivery) -> Result<Completion> {
        let node = delivery.node();
        let observations = match delivery.output {
            Out::Effect {
                kind: EffectKind::OpenPr,
                ..
            } => vec![Observation::PullRequest {
                node,
                number: node.0 + 100,
            }],
            Out::Effect {
                kind: EffectKind::PostApproval | EffectKind::ClosePr,
                ..
            }
            | Out::Notice { .. } => vec![],
            _ => return Err(crate::Error::Invalid("unsupported GitHub effect")),
        };
        self.0
            .deliver(delivery, Completion::confirmed(observations))
    }
    fn reconcile(&mut self, delivery: &Delivery) -> Result<Reconciliation> {
        self.0.reconcile(delivery)
    }
}

use super::workers::writer::{MessageKind, WriteRequest, Writer, checked_draft};
use crate::Error;
use serde_json::json;
use std::collections::BTreeMap;
use tl3_github::{
    CloseReason, GitHub,
    message::{OptionDraft, Policy},
};
use tl3_kernel::{NoteKind, Notice, Outcome, QKind};

/// The REST client owns markers, paging and deletion of duplicate app comments.
/// PR effects stay on the tracer fake until ticket 13 replaces them here.
pub struct Adapter {
    clients: BTreeMap<u64, GitHub>,
    writer: Box<dyn Writer>,
    epoch: i64,
    policy: Policy,
    runtime: tokio::runtime::Runtime,
    fake: Fake,
    operations: Option<tl3_config::OperationsIssue>,
}
impl Adapter {
    pub fn new(
        clients: BTreeMap<u64, GitHub>,
        writer: impl Writer + 'static,
        epoch: i64,
        policy: Policy,
        simulation: &Path,
    ) -> Result<Self> {
        Ok(Self {
            clients,
            writer: Box::new(writer),
            epoch,
            policy,
            runtime: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?,
            fake: Fake::new(simulation),
            operations: None,
        })
    }
    pub fn with_operations(mut self, operations: Option<tl3_config::OperationsIssue>) -> Self {
        self.operations = operations;
        self
    }

    fn fence(&mut self, delivery: &Delivery) -> Result<()> {
        if delivery.epoch < self.epoch {
            return Err(Error::Invalid("GitHub effect epoch is fenced"));
        }
        self.fake.0.fence(delivery.epoch)?;
        self.epoch = delivery.epoch;
        Ok(())
    }
    fn client(&self, delivery: &Delivery) -> Result<GitHub> {
        self.clients
            .get(&delivery.repository.installation)
            .cloned()
            .ok_or(Error::Invalid("missing GitHub installation client"))
    }
    fn request_notice(delivery: &Delivery) -> Option<Notice> {
        match delivery.output {
            Out::Notice {
                notice:
                    notice @ (Notice::Ask { .. }
                    | Notice::MeasuresPosted
                    | Notice::Summary(_)
                    | Notice::Note(_)
                    | Notice::Incident { .. }),
                ..
            } => Some(notice),
            Out::Notice {
                notice: Notice::FileReplacement,
                ..
            }
            | Out::Effect { .. } => None,
        }
    }
    fn close_reason(delivery: &Delivery) -> Option<CloseReason> {
        match delivery.output {
            Out::Notice {
                notice: Notice::Summary(Outcome::Merged | Outcome::AlreadyDone),
                ..
            } => Some(CloseReason::Completed),
            Out::Notice {
                notice:
                    Notice::Summary(Outcome::Stopped | Outcome::Duplicate | Outcome::NotActionable),
                ..
            } => Some(CloseReason::NotPlanned),
            _ => None,
        }
    }
}
impl Executor for Adapter {
    fn execute(&mut self, delivery: &Delivery) -> Result<Completion> {
        self.fence(delivery)?;
        let Some(notice) = Self::request_notice(delivery) else {
            return self.fake.execute(delivery);
        };
        if matches!(notice, Notice::Incident { .. })
            && !self.operations.as_ref().is_some_and(|o| {
                o.owner.eq_ignore_ascii_case(&delivery.repository.owner)
                    && o.name.eq_ignore_ascii_case(&delivery.repository.name)
                    && o.installation_id == delivery.repository.installation
                    && o.number == delivery.issue
            })
        {
            return Err(Error::Invalid(
                "incident delivery needs the configured operations issue",
            ));
        }
        let client = self.client(delivery)?;
        let repo = format!("{}/{}", delivery.repository.owner, delivery.repository.name);
        if self
            .runtime
            .block_on(client.reconcile_comments(&repo, delivery.issue, &delivery.key))?
            .is_none()
        {
            let issue = self.runtime.block_on(client.issue(&repo, delivery.issue))?;
            let input = writer_input(
                delivery,
                notice,
                &issue.title,
                issue.body.as_deref().unwrap_or(""),
            )?;
            let mut policy = self.policy.clone();
            policy
                .internal_terms
                .extend([delivery.key.clone(), "TL3".into()]);
            if let Some(sha) = &delivery.head_sha {
                policy.internal_terms.push(sha.clone());
            }
            policy.known_paths.extend(
                delivery
                    .paths
                    .iter()
                    .filter(|path| path.contains('/'))
                    .cloned(),
            );
            let draft = match checked_draft(self.writer.as_mut(), &delivery.key, input, &policy) {
                Ok(draft) => draft,
                Err(Error::MessageRejected) => {
                    return Ok(Completion {
                        ack: Ack::Permanent,
                        observations: vec![],
                    });
                }
                Err(error) => return Err(error),
            };
            self.fence(delivery)?;
            self.runtime.block_on(client.post_comment(
                &repo,
                delivery.issue,
                &delivery.key,
                &draft,
            ))?;
        }
        if let Some(reason) = Self::close_reason(delivery) {
            self.runtime
                .block_on(client.close_issue(&repo, delivery.issue, reason))?;
        }
        Ok(Completion::confirmed(vec![]))
    }
    fn recover_deliveries(&mut self, after: u64) -> Result<Option<u64>> {
        let Some(client) = self.clients.values().next().cloned() else {
            return Ok(None);
        };
        let mut cursor = None;
        let mut newest = after;
        let mut oldest = u64::MAX;
        let mut pending = None;
        loop {
            let page = self
                .runtime
                .block_on(client.hook_deliveries(cursor.as_deref()))?;
            let mut reached = false;
            let previous_oldest = oldest;
            for delivery in page.deliveries {
                newest = newest.max(delivery.id);
                oldest = oldest.min(delivery.id);
                if delivery.id <= after {
                    reached = true;
                    continue;
                }
                let Some(status) = delivery.status_code else {
                    pending = Some(pending.map_or(delivery.id, |old: u64| old.min(delivery.id)));
                    continue;
                };
                if !(200..300).contains(&status) {
                    self.runtime.block_on(client.redeliver_hook(delivery.id))?;
                }
            }
            if reached {
                break;
            }
            match page.next_cursor {
                Some(next) if oldest < previous_oldest && cursor.as_ref() != Some(&next) => {
                    cursor = Some(next)
                }
                Some(_) => return Err(Error::Invalid("GitHub delivery cursor did not advance")),
                None => break,
            }
        }
        Ok(Some(
            pending
                .map_or(newest, |id| newest.min(id.saturating_sub(1)))
                .max(after),
        ))
    }

    fn look(&mut self, delivery: &Delivery) -> Result<Look> {
        match self.reconcile(delivery)? {
            Reconciliation::Found(c) => return Ok(Look::Found(c)),
            Reconciliation::Absent => return Ok(Look::Absent),
            Reconciliation::Pending => {}
        }
        let Some(number) = delivery.pull_request else {
            return Ok(Look::Pending);
        };
        let client = self.client(delivery)?;
        let repo = format!("{}/{}", delivery.repository.owner, delivery.repository.name);
        let pull = self.runtime.block_on(client.pull_request(&repo, number))?;
        let checks = self
            .runtime
            .block_on(client.check_runs(&repo, &pull.head.sha))?;
        Ok(Look::Alive {
            progress: serde_json::to_string(&(
                pull.head.sha,
                pull.state,
                pull.merged_at,
                checks
                    .iter()
                    .filter(|c| c.status == "completed")
                    .map(|c| (c.id, &c.conclusion))
                    .collect::<Vec<_>>(),
            ))?,
        })
    }

    fn reconcile(&mut self, delivery: &Delivery) -> Result<Reconciliation> {
        // The current owner fences mutations while reading the original effect epoch.
        self.fake.0.fence(self.epoch)?;
        if Self::request_notice(delivery).is_none() {
            return self.fake.reconcile(delivery);
        }
        let client = self.client(delivery)?;
        let repo = format!("{}/{}", delivery.repository.owner, delivery.repository.name);
        match self
            .runtime
            .block_on(client.reconcile_comments(&repo, delivery.issue, &delivery.key))
        {
            Ok(Some(_)) => {
                if Self::close_reason(delivery).is_some() {
                    match self.runtime.block_on(client.issue(&repo, delivery.issue)) {
                        Ok(issue) if issue.state == "closed" => {}
                        Ok(_) => {
                            if self
                                .runtime
                                .block_on(client.close_issue(
                                    &repo,
                                    delivery.issue,
                                    Self::close_reason(delivery).expect("summary reason"),
                                ))
                                .is_err()
                            {
                                return Ok(Reconciliation::Pending);
                            }
                        }
                        Err(_) => return Ok(Reconciliation::Pending),
                    }
                }
                Ok(Reconciliation::Found(Completion::confirmed(vec![])))
            }
            Ok(None) => Ok(Reconciliation::Absent),
            Err(_) => Ok(Reconciliation::Pending),
        }
    }
}

fn writer_input(
    delivery: &Delivery,
    notice: Notice,
    title: &str,
    request: &str,
) -> Result<WriteRequest> {
    let request = if request.trim().is_empty() {
        title
    } else {
        request
    };
    let (kind, facts) = match notice {
        Notice::Ask { question } => {
            let binding = delivery
                .question
                .filter(|binding| binding.id == question)
                .ok_or(Error::Invalid("question notice needs its kernel question"))?;
            let (obstacle, actions): (&str, &[(&str, &str)]) = match binding.kind {
                QKind::AuditDisagreement => (
                    "Two readings of your request still disagree.",
                    &[
                        (
                            "Continue with the original reading",
                            "I will continue with the original understanding of your request.",
                        ),
                        (
                            "Change the request",
                            "I will stop this work and start from your new request.",
                        ),
                        ("Stop", "I will stop this work."),
                    ],
                ),
                QKind::NeedsSplitting => (
                    "This request needs more than one change, so I need you to narrow it.",
                    &[
                        (
                            "Change the request",
                            "I will start again from the smaller request you provide.",
                        ),
                        ("Stop", "I will stop this work."),
                    ],
                ),
                QKind::CantMeet => (
                    "I could not finish your request with the approach I tried.",
                    &[
                        (
                            "Try again with guidance",
                            "I will try again using the guidance you provide.",
                        ),
                        (
                            "Change the request",
                            "I will stop this work and start from your new request.",
                        ),
                        ("Stop", "I will stop this work."),
                    ],
                ),
                QKind::OutOfCredit => (
                    "This request has used its agreed allowance for further attempts.",
                    &[
                        (
                            "Add credit",
                            "I will continue with the extra allowance you provide.",
                        ),
                        (
                            "Change the request",
                            "I will stop this work and start from your new request.",
                        ),
                        ("Stop", "I will stop this work."),
                    ],
                ),
                QKind::EffectFailed => (
                    "A required action needs your help before work can continue.",
                    &[
                        (
                            "Continue after fixing the cause",
                            "I will continue once you confirm the cause is fixed.",
                        ),
                        ("Stop", "I will stop this work."),
                    ],
                ),
                QKind::CleanupFailed => (
                    "I need your help to finish cleaning up this work.",
                    &[
                        (
                            "Retry after fixing the cause",
                            "I will try the cleanup again.",
                        ),
                        (
                            "Mark the step handled",
                            "I will accept that you have handled this cleanup step.",
                        ),
                    ],
                ),
            };
            let options: Vec<_> = actions
                .iter()
                .map(|(action, outcome)| OptionDraft {
                    action: (*action).into(),
                    outcome: (*outcome).into(),
                })
                .collect();
            (
                MessageKind::Question,
                json!({"issue_title": title, "request": request, "obstacle": obstacle,
                "options": options, "recommended_option": 0,
                "recommendation_reason": "This option gives your request a clear next step.",
                "how_to_answer": "Reply to this question on the issue with your chosen option and any requested details."}),
            )
        }
        Notice::Incident { .. } => (
            MessageKind::Note,
            json!({"issue_title":title, "request": request,
                "change": delivery.note.as_deref().unwrap_or("Work needs help before it can continue.")}),
        ),
        Notice::Summary(outcome) => (
            MessageKind::Summary,
            json!({"issue_title": title, "request": request,
            "outcome": match outcome {
                Outcome::Merged => "Your change was merged after its checks and review passed.",
                Outcome::AlreadyDone => "The requested work was already done.",
                Outcome::Duplicate => "This request duplicates work already tracked elsewhere.",
                Outcome::NotActionable => "This request could not be acted on.",
                Outcome::Stopped => "Work on this request has stopped.",
                Outcome::Replaced => "This request has been replaced. Work continues on your new request.",
            }, "unmet_goals": if matches!(outcome, Outcome::Merged | Outcome::AlreadyDone) { vec![] } else { vec!["The original request was not completed by this work."] }}),
        ),
        notice => (
            MessageKind::Note,
            json!({"issue_title": title, "request": request, "change": match notice {
                Notice::MeasuresPosted => delivery.note.as_deref().unwrap_or("The reading of your request passed its independent check. Work will continue without waiting for your approval."),
                Notice::Note(NoteKind::UnapprovedMerge) => "A change was merged without the required approval. I have recorded the merge and the problem.",
                Notice::Note(NoteKind::Replanned) => "The earlier approach did not work. I will try a different plan for your request.",
                Notice::Note(NoteKind::PrClosed) => "The proposed change was closed. Work on this request is stopping.",
                Notice::Note(NoteKind::PrDequeued) => "The proposed change was taken out of the merge queue. Work on this request is stopping.",
                Notice::Note(NoteKind::OutsidePush) => "Someone changed the proposed code. It needs fresh checks and review.",
                Notice::Note(NoteKind::LateMerge) => "The change was merged after work stopped. I have recorded what was delivered.",
                Notice::Ask { .. } | Notice::Summary(_) | Notice::FileReplacement | Notice::Incident { .. } => unreachable!(),
            }}),
        ),
    };
    Ok(WriteRequest {
        kind,
        facts,
        violations: vec![],
    })
}
