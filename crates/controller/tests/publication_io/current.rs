use super::*;
impl Adapter {
    pub(crate) fn body_lines(&self, repo: &crate::Repository) -> &[tl3_config::BodyLine] {
        self.body_lines
            .iter()
            .find(|(served, _, _)| served == repo)
            .map_or(&[], |(_, rules, _)| rules)
    }
    fn semantic_paths(&self, delivery: &Delivery) -> Vec<String> {
        let generated = self
            .body_lines
            .iter()
            .find(|(repo, _, _)| *repo == delivery.repository)
            .map_or(&[][..], |(_, _, generated)| generated.as_slice());
        delivery
            .paths
            .iter()
            .filter(|path| !tl3_config::path_is_protected(path, generated))
            .cloned()
            .collect()
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
        if self
            .runtime
            .block_on(client.reconcile_comments(&repo, delivery.issue, delivery.marker_key()))?
            .is_none()
        {
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
        let mut issue = self
            .runtime
            .block_on(client.issue(&repo, delivery.issue))
            .map_err(|error| {
                crate::input_resources::github(
                    error,
                    crate::input_resources::Resource::prerequisite(
                        &delivery.repository,
                        tl3_kernel::PrerequisiteKind::Issue,
                        delivery.issue,
                    ),
                    delivery.repository.installation,
                )
            })?;
        if issue.state == "closed" {
            let app = format!("{}[bot]", client.app_slug());
            let events = self
                .runtime
                .block_on(client.issue_events(&repo, delivery.issue))
                .map_err(|error| {
                    crate::input_resources::github(
                        error,
                        crate::input_resources::Resource::GithubEndpoint {
                            path: format!("repos/{repo}/issues/{}/events", delivery.issue),
                        },
                        delivery.repository.installation,
                    )
                })?;
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
        if !publication::message_delivery(notice.is_some(), Self::publication_comment(delivery)) {
            return self
                .records
                .deliver(delivery, Completion::confirmed(vec![]));
        }
        if !publication::incident_admitted(
            matches!(notice, Some(Notice::Incident { .. })),
            self.operations
                .as_ref()
                .map(|o| publication::OperationsTarget {
                    owner: o.owner.eq_ignore_ascii_case(&delivery.repository.owner),
                    repository: o.name.eq_ignore_ascii_case(&delivery.repository.name),
                    installation: o.installation_id == delivery.repository.installation,
                    issue: o.number == delivery.issue,
                }),
        ) {
            return Err(Error::Invalid(
                "incident delivery needs the configured operations issue",
            ));
        }
        let client = self.client(delivery)?;
        let repo = format!("{}/{}", delivery.repository.owner, delivery.repository.name);
        if self
            .runtime
            .block_on(client.reconcile_comments(&repo, delivery.issue, delivery.marker_key()))?
            .is_none()
        {
            let mut policy = self.policy.clone();
            // These posts can't read the request's issue: it lives elsewhere or is gone.
            let input = if matches!(
                publication::title_source(
                    matches!(notice, Some(Notice::Incident { .. })),
                    matches!(
                        delivery.steering,
                        Some(crate::steering::Task::RestartDropped { .. })
                    ),
                ),
                publication::TitleSource::Context
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
                let issue = self
                    .runtime
                    .block_on(client.issue(&repo, delivery.issue))
                    .map_err(|error| {
                        crate::input_resources::github(
                            error,
                            crate::input_resources::Resource::prerequisite(
                                &delivery.repository,
                                tl3_kernel::PrerequisiteKind::Issue,
                                delivery.issue,
                            ),
                            delivery.repository.installation,
                        )
                    })?;
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
                        publication::rejected_message(Self::publication_comment(delivery)),
                        publication::MessageRejection::Fallback
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
    fn reconcile(&mut self, delivery: &Delivery) -> Result<Reconciliation> {
        // The current owner fences mutations while reading the original effect epoch.
        self.records.fence(self.epoch)?;
        let route = Self::publication_route(delivery);
        if matches!(route, publication::ReconcileRoute::Approval) {
            // A failed read proves neither absence nor an outstanding POST.
            return self.reconcile_publication(delivery);
        }
        if matches!(route, publication::ReconcileRoute::Publication) {
            let result = self.reconcile_publication(delivery);
            let read = match &result {
                Ok(_) => publication::PublicationRead::Reconciled,
                Err(Error::Permanent {
                    class: "pr_evidence",
                    ..
                }) => publication::PublicationRead::EvidencePermanent,
                Err(Error::Permanent { .. }) => publication::PublicationRead::OtherPermanent,
                Err(_) => publication::PublicationRead::Failed,
            };
            return match publication::publication_reconciliation(read) {
                publication::ReconcileAction::FoundPermanent => Ok(Reconciliation::Found(
                    Box::new(Completion::permanent("pr_evidence")),
                )),
                publication::ReconcileAction::Preserve
                | publication::ReconcileAction::Propagate => result,
                publication::ReconcileAction::Pending => Ok(Reconciliation::Pending),
            };
        }
        if matches!(route, publication::ReconcileRoute::Issue)
            && let Some(crate::steering::Task::FileRequest {
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
        if matches!(route, publication::ReconcileRoute::Restart) {
            return Ok(Reconciliation::Absent);
        }
        if matches!(route, publication::ReconcileRoute::ConsoleNote) {
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
        if matches!(route, publication::ReconcileRoute::Records) {
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
                    let issue = match self.runtime.block_on(client.issue(&repo, delivery.issue)) {
                        Ok(issue) if issue.state == "closed" => publication::IssueRead::Closed,
                        Ok(_) => publication::IssueRead::Open,
                        Err(_) => publication::IssueRead::Failed,
                    };
                    match publication::close_after_reconcile(issue) {
                        publication::CloseAction::Complete => {}
                        publication::CloseAction::Close => {
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
                        publication::CloseAction::Pending => return Ok(Reconciliation::Pending),
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
    fn repair_approval(&mut self, delivery: &Delivery) -> Result<(Reconciliation, Option<usize>)> {
        // The current owner authorizes repair of evidence from an older delivery.
        self.records.fence(self.epoch)?;
        self.repair_approval_copies(delivery)
    }
    fn reconcile_cancellation(&mut self, delivery: &Delivery) -> Result<Reconciliation> {
        if crate::approval_delivery::is_approval(delivery) {
            self.records.fence(self.epoch)?;
            // Stop retires the local operation, never sends another approval.
            // An outstanding remote POST may still land; webhook repair remains keyed.
            return Ok(Reconciliation::Absent);
        }
        self.reconcile(delivery)
    }
    fn publication_comment(delivery: &Delivery) -> publication::Comment {
        match delivery.output {
            Out::Comment {
                kind: CommentKind::Clarify,
                ..
            } => publication::Comment::Clarify,
            Out::Comment {
                kind: CommentKind::ReplacementDropped,
                ..
            } => publication::Comment::ReplacementDropped,
            _ => publication::Comment::Other,
        }
    }
    fn publication_route(delivery: &Delivery) -> publication::ReconcileRoute {
        let output = if crate::approval_delivery::is_approval(delivery) {
            publication::Output::Approval
        } else if publishing::handles(delivery) {
            publication::Output::Publication
        } else if publication::message_delivery(
            Self::request_notice(delivery).is_some(),
            Self::publication_comment(delivery),
        ) {
            publication::Output::Message
        } else {
            publication::Output::Other
        };
        let steering = match delivery.steering {
            Some(crate::steering::Task::FileRequest {
                correction: true, ..
            }) => publication::Steering::Correction,
            Some(crate::steering::Task::Restart { .. }) => publication::Steering::Restart,
            Some(crate::steering::Task::ConsoleNote { .. }) => publication::Steering::ConsoleNote,
            _ => publication::Steering::Other,
        };
        publication::reconcile_route(output, steering)
    }
}
