use super::decide;
use super::fixture::*;
type Adapter = Shell<false>;
impl Adapter {
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
            | Out::Effect { .. }
            | Out::Comment { .. } => None,
        }
    }
    fn close_reason(delivery: &Delivery) -> Option<CloseReason> {
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
    fn reconcile(&mut self, delivery: &Delivery) -> Result<Reconciliation> {
        // The current owner fences mutations while reading the original effect epoch.
        self.records.fence(self.epoch)?;
        if crate::approval_delivery::is_approval(delivery) {
            // A failed read proves neither absence nor an outstanding POST.
            return self.reconcile_publication(delivery);
        }
        if publishing::handles(delivery) {
            return match self.reconcile_publication(delivery) {
                Err(Error::Permanent {
                    class: "pr_evidence",
                    ..
                }) => Ok(Reconciliation::Found(Box::new(Completion::permanent(
                    "pr_evidence",
                )))),
                // The controller records the refusal while leaving reconciliation pending.
                Err(error @ Error::Permanent { .. }) => Err(error),
                result => Ok(result.unwrap_or(Reconciliation::Pending)),
            };
        }
        if let Some(crate::steering::Task::FileRequest {
            correction: true,
            since,
            ..
        }) = &delivery.steering
        {
            let client = self.client(delivery)?;
            let repo = format!("{}/{}", delivery.repository.owner, delivery.repository.name);
            return Ok(
                match self.runtime.block_on(client.reconcile_issue(
                    &repo,
                    delivery.marker_key(),
                    since,
                )) {
                    Ok(Some(issue)) => {
                        Reconciliation::Found(Box::new(Self::linked(delivery, issue)))
                    }
                    Ok(None) => Reconciliation::Absent,
                    Err(_) => Reconciliation::Pending,
                },
            );
        }
        // A restart only reads the issue and reopens it, so running it again is safe.
        if let Some(crate::steering::Task::Restart { .. }) = &delivery.steering {
            return Ok(Reconciliation::Absent);
        }
        if let Some(crate::steering::Task::ConsoleNote { .. }) = &delivery.steering {
            let client = self.client(delivery)?;
            let repo = format!("{}/{}", delivery.repository.owner, delivery.repository.name);
            return Ok(
                match self.runtime.block_on(client.reconcile_comments(
                    &repo,
                    delivery.issue,
                    delivery.marker_key(),
                )) {
                    Ok(Some(_)) => Reconciliation::Found(Box::new(Completion::confirmed(vec![]))),
                    Ok(None) => Reconciliation::Absent,
                    Err(_) => Reconciliation::Pending,
                },
            );
        }
        if Self::request_notice(delivery).is_none()
            && !matches!(
                delivery.output,
                Out::Comment {
                    kind: CommentKind::Clarify | CommentKind::ReplacementDropped,
                    ..
                }
            )
        {
            return self.records.reconcile(delivery);
        }
        let client = self.client(delivery)?;
        let repo = format!("{}/{}", delivery.repository.owner, delivery.repository.name);
        match self.runtime.block_on(client.reconcile_comments(
            &repo,
            delivery.issue,
            delivery.marker_key(),
        )) {
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
                if self.close_original(delivery).is_err() {
                    return Ok(Reconciliation::Pending);
                }
                Ok(Reconciliation::Found(Box::new(Completion::confirmed(
                    vec![],
                ))))
            }
            Ok(None) => Ok(Reconciliation::Absent),
            Err(_) => Ok(Reconciliation::Pending),
        }
    }
    fn execute_delivery(&mut self, delivery: &Delivery) -> Result<Completion> {
        self.fence(delivery)?;
        if publishing::handles(delivery) {
            return self.publish(delivery);
        }
        if let Some(crate::steering::Task::FileRequest { .. }) = &delivery.steering {
            return self.file_request(delivery);
        }
        if let Some(crate::steering::Task::Restart { .. }) = &delivery.steering {
            return self.restart(delivery);
        }
        if let Some(crate::steering::Task::ConsoleNote { text }) = &delivery.steering {
            return self.console_note(delivery, text);
        }
        let notice = Self::request_notice(delivery);
        if notice.is_none()
            && !matches!(
                delivery.output,
                Out::Comment {
                    kind: CommentKind::Clarify | CommentKind::ReplacementDropped,
                    ..
                }
            )
        {
            return self
                .records
                .deliver(delivery, Completion::confirmed(vec![]));
        }
        if matches!(notice, Some(Notice::Incident { .. }))
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
        if decide::should_post(
            self.runtime
                .block_on(client.reconcile_comments(&repo, delivery.issue, delivery.marker_key()))?
                .is_some(),
        ) {
            let mut policy = self.policy.clone();
            // These posts can't read the request's issue: it lives elsewhere or is gone.
            let input = if matches!(notice, Some(Notice::Incident { .. }))
                || matches!(
                    delivery.steering,
                    Some(crate::steering::Task::RestartDropped { .. })
                ) {
                let context = delivery
                    .question_context
                    .as_ref()
                    .ok_or(Error::Invalid("incident needs its request context"))?;
                let title = context["issue_title"]
                    .as_str()
                    .ok_or(Error::Invalid("incident request title missing"))?;
                policy.request_title = Some(title.into());
                policy.request_body.clear();
                writer_input(
                    delivery,
                    notice,
                    title,
                    context["request"]
                        .as_str()
                        .ok_or(Error::Invalid("incident request link missing"))?,
                )?
            } else {
                let issue = self.runtime.block_on(client.issue(&repo, delivery.issue))?;
                policy.request_title = Some(issue.title.clone());
                policy.request_body = issue.body.clone().unwrap_or_default();
                writer_input(
                    delivery,
                    notice,
                    &issue.title,
                    issue.body.as_deref().unwrap_or(""),
                )?
            };
            policy.own_repository = Some(repo.clone());
            policy.own_issue = Some(delivery.issue);
            policy.own_pull_request = delivery.pull_request;
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
            let mut draft = match question_draft(self.writer.as_mut(), delivery, input, &policy) {
                Ok(draft) => draft,
                Err(Error::MessageRejected)
                    if matches!(
                        delivery.output,
                        Out::Comment {
                            kind: CommentKind::Clarify,
                            ..
                        }
                    ) =>
                {
                    let fallback = "I couldn't act on your comment. Could you rephrase it?";
                    tl3_github::message::check_prose(fallback, &policy)
                        .map_err(|_| Error::MessageRejected)?;
                    fallback.into()
                }
                Err(Error::MessageRejected) => {
                    return Ok(Completion::permanent("message_rejected"));
                }
                Err(error) => return Err(error),
            };
            if let Some(text) = fixed_notice_text(delivery)? {
                draft.push_str("\n\n");
                draft.push_str(&text);
            }
            self.fence(delivery)?;
            self.runtime.block_on(client.post_comment(
                &repo,
                delivery.issue,
                delivery.marker_key(),
                &draft,
            ))?;
        }
        if let Some(reason) = Self::close_reason(delivery) {
            self.runtime
                .block_on(client.close_issue(&repo, delivery.issue, reason))?;
        }
        self.close_original(delivery)?;
        Ok(Completion::confirmed(vec![]))
    }
    fn recover_deliveries(&mut self, after: u64, since_ms: u64) -> Result<Option<u64>> {
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
                if delivery.id <= after
                    || delivery.delivered_at.timestamp_millis() < since_ms as i64
                {
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
    fn fence(&mut self, delivery: &Delivery) -> Result<()> {
        if delivery.epoch < self.epoch {
            return Err(Error::Invalid("GitHub effect epoch is fenced"));
        }
        self.records.fence(delivery.epoch)?;
        self.epoch = delivery.epoch;
        Ok(())
    }
    fn client(&self, delivery: &Delivery) -> Result<GitHub> {
        self.clients
            .get(&delivery.repository.installation)
            .cloned()
            .ok_or(Error::Invalid("missing GitHub installation client"))
    }
    fn linked(delivery: &Delivery, issue: tl3_github::Issue) -> Completion {
        Completion::confirmed(vec![Observation::LinkedRequest {
            repository: delivery.repository.clone(),
            issue,
            original: delivery.node(),
        }])
    }
    fn file_request(&mut self, delivery: &Delivery) -> Result<Completion> {
        let Some(crate::steering::Task::FileRequest {
            text,
            comment,
            correction: true,
            revert,
            since,
            comment_issue,
            ..
        }) = &delivery.steering
        else {
            unreachable!()
        };
        let client = self.client(delivery)?;
        let repo = format!("{}/{}", delivery.repository.owner, delivery.repository.name);
        let title = format!("Correction for #{}", delivery.issue);
        let comment_path = if *comment_issue == delivery.issue {
            "issues"
        } else {
            "pull"
        };
        let body = format!(
            "{text}\n\nCorrects #{}, authorized by the owner's [comment](https://github.com/{repo}/{comment_path}/{}#issuecomment-{comment}).{}",
            delivery.issue,
            comment_issue,
            if *revert {
                "\n\nThe owner requested a revert as part of this correction."
            } else {
                ""
            }
        );
        let issue = self.runtime.block_on(client.create_request_issue(
            &repo,
            delivery.marker_key(),
            since,
            &title,
            &body,
        ))?;
        Ok(Self::linked(delivery, issue))
    }
    fn console_note(&mut self, delivery: &Delivery, text: &str) -> Result<Completion> {
        let text = crate::redaction::text(text)?;
        let client = self.client(delivery)?;
        let repo = format!("{}/{}", delivery.repository.owner, delivery.repository.name);
        if decide::should_post(
            self.runtime
                .block_on(client.reconcile_comments(&repo, delivery.issue, delivery.marker_key()))?
                .is_some(),
        ) {
            self.fence(delivery)?;
            self.runtime.block_on(client.post_comment(
                &repo,
                delivery.issue,
                delivery.marker_key(),
                &text,
            ))?;
        }
        Ok(Completion::confirmed(vec![]))
    }
    fn restart(&mut self, delivery: &Delivery) -> Result<Completion> {
        let client = self.client(delivery)?;
        let repo = format!("{}/{}", delivery.repository.owner, delivery.repository.name);
        let mut issue = self.runtime.block_on(client.issue(&repo, delivery.issue))?;
        if issue.state == "closed" {
            let app = format!("{}[bot]", client.app_slug());
            let events = self
                .runtime
                .block_on(client.issue_events(&repo, delivery.issue))?;
            if events
                .iter()
                .rev()
                .find(|event| event.event == "closed")
                .and_then(|event| event.actor.as_ref())
                .is_some_and(|actor| actor.login == app)
            {
                issue = self
                    .runtime
                    .block_on(client.reopen_issue(&repo, delivery.issue))?;
            }
        }
        Ok(Self::linked(delivery, issue))
    }
}
pub fn writer_input(
    delivery: &Delivery,
    notice: Option<Notice>,
    title: &str,
    request: &str,
) -> Result<WriteRequest> {
    let mut input = writer_facts(delivery, notice, title, request)?;
    if let Some(context) = &delivery.question_context
        && let Some(diagnostic) = context
            .get("publication_diagnostic")
            .filter(|value| !value.is_null())
    {
        input.facts["publication_diagnostic"] = diagnostic.clone();
        input.facts["occurrences"] = context["occurrences"].clone();
    }
    if let Some(failure) = delivery
        .question_context
        .as_ref()
        .and_then(|context| context.get("worker_input_failure"))
        .filter(|value| !value.is_null())
    {
        input.facts["worker_input_failure"] = failure.clone();
    }
    // #427: the draft is written for this delivery's request.
    input.request = Some(super::workers::writer::WriteFor {
        node: delivery.node().0,
        generation: delivery.generation,
        repo: format!("{}/{}", delivery.repository.owner, delivery.repository.name),
        issue: delivery.issue,
        attempt: delivery.attempt,
        round: delivery.round,
    });
    Ok(input)
}
fn writer_facts(
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

pub fn reconcile(shell: &mut Adapter, delivery: &Delivery) -> Result<Reconciliation> {
    shell.reconcile(delivery)
}
pub fn execute(shell: &mut Adapter, delivery: &Delivery) -> Result<Completion> {
    shell.execute_delivery(delivery)
}
pub fn recover(shell: &mut Adapter) -> Result<Option<u64>> {
    shell.recover_deliveries(4, 0)
}
