#[cfg(not(verus_keep_ghost))]
use super::*;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublicationRead {
    PrEvidence,
    OtherPermanent,
    Transient,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    PermanentCompletion,
    ReturnError,
    Pending,
}

/// State-table Effect/Permanent failure: pr_evidence remains a permanent ack.
/// The complete three-variant domain is covered; no trace bound is imposed.
#[cfg_attr(
    verus_keep_ghost,
    vstd::prelude::verus_spec(out => ensures
        out == match read {
            PublicationRead::PrEvidence => Refusal::PermanentCompletion,
            PublicationRead::OtherPermanent => Refusal::ReturnError,
            PublicationRead::Transient => Refusal::Pending,
        },
    )
)]
pub fn publication_refusal(read: PublicationRead) -> Refusal {
    match read {
        PublicationRead::PrEvidence => Refusal::PermanentCompletion,
        PublicationRead::OtherPermanent => Refusal::ReturnError,
        PublicationRead::Transient => Refusal::Pending,
    }
}

/// State-table Comment/message contract: a visible marker prevents another post.
/// Both booleans, unbounded in the number of calls.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == !found))]
pub fn should_post(found: bool) -> bool {
    !found
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookAction {
    Ignore,
    Pending,
    Redeliver,
    Delivered,
}

/// PRD reliability/catch-up: replay failed hook deliveries only within the window.
/// All u16 status codes and both cutoff results, with no event-count bound.
#[cfg_attr(
    verus_keep_ghost,
    vstd::prelude::verus_spec(out => ensures
        out == if outside_window { HookAction::Ignore } else {
            match status {
                None => HookAction::Pending,
                Some(code) => if 200 <= code && code < 300 { HookAction::Delivered }
                    else { HookAction::Redeliver },
            }
        },
    )
)]
pub fn hook_action(outside_window: bool, status: Option<u16>) -> HookAction {
    if outside_window {
        HookAction::Ignore
    } else {
        match status {
            None => HookAction::Pending,
            Some(code) => {
                if 200 <= code && code < 300 {
                    HookAction::Delivered
                } else {
                    HookAction::Redeliver
                }
            }
        }
    }
}

/// PRD reliability/catch-up: an unfinished delivery cannot be skipped by a cursor.
/// The complete u64 domain, including pending ID zero and the maximum cursor.
#[cfg_attr(
    verus_keep_ghost,
    vstd::prelude::verus_spec(out => ensures
        out >= after,
        pending.is_none() ==> out == if newest < after { after } else { newest },
        match pending {
            Some(id) => out == {
                let before = if id == 0 { 0 } else { id - 1 };
                let bounded = if newest < before { newest } else { before };
                if bounded < after { after } else { bounded }
            },
            None => true,
        },
    )
)]
pub fn recovery_cursor(after: u64, newest: u64, pending: Option<u64>) -> u64 {
    let candidate = match pending {
        Some(id) => {
            let before = if id == 0 { 0 } else { id - 1 };
            if newest < before { newest } else { before }
        }
        None => newest,
    };
    if candidate < after { after } else { candidate }
}

/// Anti-vacuity for the failed-final effect row and marker delivery contract.
#[cfg(verus_keep_ghost)]
#[vstd::prelude::verus_spec(out => ensures out.0 == Refusal::PermanentCompletion, out.1 == false)]
pub fn permanent_witness() -> (Refusal, bool) {
    (
        publication_refusal(PublicationRead::PrEvidence),
        should_post(true),
    )
}

/// Anti-vacuity: an unfinished zero-ID delivery does not underflow the cursor.
#[cfg(verus_keep_ghost)]
#[vstd::prelude::verus_spec(out => ensures out == 0)]
pub fn zero_pending_witness() -> u64 {
    recovery_cursor(0, 12, Some(0))
}

#[cfg(verus_keep_ghost)]
fn main() {}
#[cfg(not(verus_keep_ghost))]
pub(super) fn request_notice(delivery: &Delivery) -> Option<Notice> {
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
        | Out::Effect { .. }
        | Out::Comment { .. } => None,
    }
}

#[cfg(not(verus_keep_ghost))]
pub(super) fn close_reason(delivery: &Delivery) -> Option<CloseReason> {
    match delivery.output {
        Out::Notice {
            notice: Notice::Summary(Outcome::Merged | Outcome::AlreadyDone),
            ..
        } => Some(CloseReason::Completed),
        Out::Notice {
            notice: Notice::Summary(Outcome::Duplicate | Outcome::NotActionable),
            ..
        } => Some(CloseReason::NotPlanned),
        _ => None,
    }
}

#[cfg(not(verus_keep_ghost))]
pub(super) fn writer_facts(
    delivery: &Delivery,
    notice: Option<Notice>,
    title: &str,
    request: &str,
) -> Result<WriteRequest> {
    let request = delivery.request_text.as_deref().unwrap_or(request);
    let request = if request.trim().is_empty() {
        title
    } else {
        request
    };
    if let Some(crate::steering::Task::ReplacementDropped { text, outcome }) = &delivery.steering {
        let change = if *outcome == Outcome::Merged {
            "The old version merged before your change could start. Please file your change again."
        } else {
            "Work on the old version ended before your change could start. Please file your change again."
        };
        return Ok(WriteRequest {
            kind: MessageKind::Note,
            facts: json!({"issue_title": title, "request": request,
                "change": change, "change_quote": text}),
            violations: vec![],
            request: None,
        });
    }
    if let Some(crate::steering::Task::RestartDropped { text }) = &delivery.steering {
        return Ok(WriteRequest {
            kind: MessageKind::Note,
            facts: json!({"issue_title": title, "request": request,
                "change": "The issue for this request could not be found, so the restart was dropped. Nothing was filed.",
                "change_quote": text}),
            violations: vec![],
            request: None,
        });
    }
    if let Some(crate::steering::Task::ClarifyComment { question, excerpt }) = &delivery.steering {
        return Ok(WriteRequest {
            kind: MessageKind::Clarification,
            facts: json!({"issue_title": title, "request": request, "question": question, "comment_excerpt": excerpt}),
            violations: vec![],
            request: None,
        });
    }
    let (kind, mut facts) = match notice.ok_or(Error::Invalid("missing request notice"))? {
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
                            "I will continue with the original understanding of your request and set the check's open objections aside.",
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
            // #322: the deciding worker's findings, redacted by the worker process.
            let findings = delivery
                .question_context
                .as_ref()
                .and_then(|context| context.get("findings"))
                .filter(|findings| findings.is_object());
            let options: Vec<_> = actions
                .iter()
                .map(|(action, outcome)| OptionDraft {
                    action: (*action).into(),
                    outcome: if findings.is_some() && *action == "Try again with guidance" {
                        "I will try again using your answer to the question above and any guidance you provide.".into()
                    } else {
                        (*outcome).into()
                    },
                })
                .collect();
            let mut facts = json!({"issue_title": title, "request": request, "obstacle": if binding.clarified { "I could not tell which option you chose. Could you clarify your answer?" } else { obstacle },
                "options": options, "recommended_option": 0,
                "recommendation_reason": "This option gives your request a clear next step.",
                "how_to_answer": format!("Reply on the issue, naming question {question}, with your chosen option and any requested details.")});
            if let Some(findings) = findings {
                facts["findings"] = findings.clone();
            }
            (MessageKind::Question, facts)
        }
        Notice::Incident { .. } => (
            MessageKind::Note,
            json!({"issue_title":title, "request": request,
                "change": delivery.note.as_deref().unwrap_or("Work needs help before it can continue.")}),
        ),
        Notice::Summary(outcome) => {
            // Q54: a stop for TL3 review names its step and carries its evidence.
            let review_stop = delivery
                .question_context
                .as_ref()
                .filter(|_| outcome == Outcome::Stopped);
            let stopped = review_stop.and_then(crate::review_stops::outcome);
            let mut facts = json!({"issue_title": title, "request": request,
            "outcome": match outcome {
                Outcome::Merged => "Your change was merged.",
                Outcome::AlreadyDone => "The requested work was already done.",
                Outcome::Duplicate => "This request duplicates work already tracked elsewhere.",
                Outcome::NotActionable => "This request could not be acted on.",
                Outcome::Stopped => stopped.as_deref().unwrap_or("Work on this request has stopped."),
                Outcome::Replaced => "This request has been replaced. Work continues on your new request.",
            }, "unmet_goals": crate::evidence::unmet_goals(delivery.evidence.as_ref()),
            "evidence_status": if delivery.evidence.is_some() {"captured"} else {"not_captured"},
            "merge_status": if outcome == Outcome::Merged || delivery.evidence.as_ref().is_some_and(|e| e.pull_request_status == crate::evidence::PullRequestStatus::Merged) {"merged"} else {"not_recorded"},
            "evidence": delivery.evidence,
            "recorded_summary": crate::evidence::summary(delivery)?,
            "pull_request": delivery.pull_request.map(|number| json!({"number":number,
                "url":format!("https://github.com/{}/{}/pull/{number}",delivery.repository.owner,delivery.repository.name)}))});
            if let Some(findings) = review_stop.and_then(crate::review_stops::findings) {
                facts["findings"] = findings;
            }
            (MessageKind::Summary, facts)
        }
        notice => (
            MessageKind::Note,
            json!({"issue_title": title, "request": request, "change": match notice {
                Notice::MeasuresPosted => delivery.note.as_deref().unwrap_or("The reading of your request passed its independent check. Work will continue without waiting for your approval."),
                Notice::Note(NoteKind::UnapprovedMerge) => "A change was merged without the required approval. I have recorded the merge and the problem.",
                Notice::Note(NoteKind::Replanned) => "The earlier approach did not work. I will try a different plan for your request.",
                Notice::Note(NoteKind::PrClosed) => "The proposed change was closed. Work on this request is stopping.",
                Notice::Note(NoteKind::PrDequeued) => "The proposed change was taken out of the merge queue. Work on this request is stopping.",
                Notice::Note(NoteKind::OutsidePush) => "Someone changed the proposed code. It needs fresh checks and review.",
                Notice::Note(NoteKind::DependencyAbandoned { .. } | NoteKind::DependencyCycle { .. }) => delivery.note.as_deref().unwrap_or("A merge prerequisite prevents work from starting. Please change or stop the request."),
                Notice::Note(NoteKind::LateMerge) => "The change was merged after work stopped. I have recorded what was delivered.",
                Notice::Ask { .. } | Notice::Summary(_) | Notice::FileReplacement | Notice::Incident { .. } => unreachable!(),
            }}),
        ),
    };
    if kind == MessageKind::Summary
        && let Some(removed) = &delivery.scope_removals
    {
        facts["removed_changes"] = json!(removed);
    }
    Ok(WriteRequest {
        kind,
        facts,
        violations: vec![],
        request: None,
    })
}
