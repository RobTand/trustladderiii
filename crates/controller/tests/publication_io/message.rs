pub(crate) fn question_draft(
    writer: &mut dyn Writer,
    delivery: &Delivery,
    input: WriteRequest,
    policy: &Policy,
) -> Result<String> {
    let fallback = (input.kind == MessageKind::Question).then(|| input.clone());
    match (
        checked_draft(writer, &delivery.key, input, policy),
        fallback,
    ) {
        (Err(Error::MessageRejected), Some(input)) => fixed_question(
            &input,
            delivery
                .question
                .and_then(|question| crate::question_findings::fixed_decision(question.kind)),
            policy,
        ),
        (result, _) => result,
    }
}
fn fixed_notice_text(delivery: &Delivery) -> Result<Option<String>> {
    match delivery.output {
        Out::Notice {
            notice: Notice::Incident { .. },
            ..
        } => {
            let id = delivery
                .question_context
                .as_ref()
                .and_then(|context| context["incident_id"].as_str())
                .or_else(|| {
                    delivery
                        .note
                        .as_deref()
                        .and_then(|note| note.split_once("clear incident ").map(|(_, tail)| tail))
                        .and_then(|tail| {
                            tail.split(|c: char| !(c.is_ascii_digit() || c == '.'))
                                .next()
                        })
                })
                .ok_or(Error::Invalid("incident id missing"))?;
            let destination = format!(
                "https://github.com/{}/{}/issues/{}",
                delivery.repository.owner, delivery.repository.name, delivery.issue
            );
            let request = delivery
                .question_context
                .as_ref()
                .and_then(|context| context["request"].as_str())
                .and_then(|request| request.rsplit_once('('))
                .and_then(|(_, url)| url.strip_suffix(')'))
                .unwrap_or(&destination);
            let diagnostic = delivery
                .question_context
                .as_ref()
                .and_then(|context| context.get("publication_diagnostic"))
                .filter(|value| !value.is_null())
                .map(|value| {
                    crate::decode_stored::<crate::publication_failure::Diagnostic>(value.clone())
                })
                .transpose()?;
            let recurrence = delivery
                .question_context
                .as_ref()
                .and_then(|context| context["occurrences"].as_u64())
                .map(|count| format!("Recorded occurrence {count}."))
                .unwrap_or_else(|| "Occurrence count unavailable.".into());
            let input_failure = delivery
                .question_context
                .as_ref()
                .and_then(|context| context.get("worker_input_failure"))
                .filter(|value| !value.is_null())
                .map(|value| crate::decode_stored::<crate::worker_inputs::Context>(value.clone()))
                .transpose()?;
            let mut details = diagnostic
                .as_ref()
                .map(|diagnostic| format!("{recurrence} {}\n\n", diagnostic.render()))
                .unwrap_or_default();
            if let Some(failure) = input_failure {
                details.push_str(&format!("{failure}\n\n"));
            }
            Ok(Some(format!(
                "{details}Reply with \"clear incident {id}\" on {destination} or on the request's issue at {request} after fixing the cause."
            )))
        }
        Out::Notice {
            notice: Notice::Summary(outcome),
            ..
        } => {
            let restart = (outcome == Outcome::Stopped)
                .then_some("Comment `restart` on this issue to start again.");
            Ok(match (restart, delivery.note.as_deref()) {
                (Some(restart), Some(note)) => Some(format!("{restart}\n\n{note}")),
                (restart, note) => restart.or(note).map(str::to_owned),
            })
        }
        _ => Ok(None),
    }
}
