//! One validator for controller acceptance and the worker's repair turn.

mod decision;
use decision::{Answer, Classification, Kind, decide};
use serde_json::Value;
use std::collections::BTreeSet;

/// Validate the wire reply, then decide over finite classification facts.
pub fn validate(inputs: &Value, reply: &Value) -> crate::Result<()> {
    crate::adapters::workers::protocol::classifier_schema(inputs, false)?;
    crate::adapters::workers::protocol::classifier_schema(reply, true)?;
    let classification = parse(inputs, reply)?;
    decide(classification).map_err(|violation| crate::Error::Invalid(violation.message()))
}

fn parse(inputs: &Value, reply: &Value) -> crate::Result<Classification> {
    let kind = match reply["kind"].as_str() {
        Some("change_request") => Kind::ChangeRequest,
        Some("answer") => Kind::Answer,
        Some("restart") => Kind::Restart,
        Some("complaint") => Kind::Complaint,
        Some("remark") => Kind::Remark,
        Some("unsure") => Kind::Unsure,
        Some("stop") => Kind::Stop,
        Some("pause") => Kind::Pause,
        Some("resume") => Kind::Resume,
        Some("reconcile_decisions") => Kind::Reconcile,
        _ => return Err(crate::Error::Invalid("classifier value breaks role schema")),
    };
    let answer = match reply["answer"].as_str() {
        Some("change_request") => Answer::ChangeRequest,
        Some("add_credit") => Answer::AddCredit,
        Some("unclear") => Answer::Unclear,
        _ => Answer::Other,
    };
    let decisions = inputs["decision_context"]["decisions"].as_array();
    let all: BTreeSet<_> = decisions
        .into_iter()
        .flatten()
        .filter_map(|d| d["id"].as_str())
        .collect();
    let active: BTreeSet<_> = decisions
        .into_iter()
        .flatten()
        .filter(|d| d["superseded_by"].is_null())
        .filter_map(|d| d["id"].as_str())
        .collect();
    let assessments = reply["assessments"].as_array();
    let ids: BTreeSet<_> = assessments
        .into_iter()
        .flatten()
        .filter_map(|a| a["id"].as_str())
        .collect();
    let question = &inputs["state"]["open_question"];
    Ok(Classification {
        kind,
        answer,
        reconcile_issue: inputs.get("reconcile_issue").is_some(),
        assessments_exact: decisions.is_some()
            && ids == all
            && assessments.is_some_and(|a| a.len() == ids.len()),
        selected_matches: inputs["selected_answer"]
            .as_object()
            .is_none_or(|selected| {
                selected.iter().all(|(key, value)| {
                    reply.get(key) == Some(value)
                            // Python compares 1 and 1.0 equally in a selected answer;
                            // the credit-integer rule still rejects a floating reply.
                            || (key == "amount" && reply[key].as_f64() == value.as_f64())
                })
            }),
        has_applicable: !active.is_empty(),
        supersedes_present: reply.get("supersedes").is_some(),
        supersedes_applicable: reply["supersedes"].as_array().is_none_or(|ids| {
            ids.iter()
                .all(|id| id.as_str().is_some_and(|id| active.contains(id)))
        }),
        closed: inputs["state"]["closed"] == true,
        restartable: inputs["state"]["restartable"] == true,
        question_current: !question.is_null() && reply["question_id"] == question["id"],
        answer_offered: question["answers"]
            .as_array()
            .is_some_and(|answers| answers.contains(&reply["answer"])),
        credit_integer: reply["amount"].as_u64().is_some(),
        // Python str.strip includes these four separators in addition to Unicode White_Space.
        text_nonblank: reply["new_text"].as_str().is_some_and(|text| {
            text.chars()
                .any(|c| !(c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)))
        }),
    })
}

/// The CLI boundary accepts one request and emits one JSON result, without config or DB access.
pub fn check_reply(role: Option<&str>) -> crate::Result<()> {
    use std::io::{Read, Write};
    if role != Some("classifier") {
        return Err(crate::Error::Invalid("usage: tl3 check-reply classifier"));
    }
    let mut bytes = Vec::new();
    std::io::stdin().read_to_end(&mut bytes)?;
    let request = crate::adapters::workers::json::parse(&bytes)
        .map_err(|_| crate::Error::Invalid("invalid check-reply JSON"))?;
    let violations = match validate(&request["inputs"], &request["reply"]) {
        Ok(()) => Vec::new(),
        Err(crate::Error::Invalid(message)) => vec![message],
        Err(error) => return Err(error),
    };
    serde_json::to_writer(
        std::io::stdout().lock(),
        &serde_json::json!({"violations": violations}),
    )
    .map_err(|error| crate::Error::Adapter(error.to_string().into()))?;
    std::io::stdout().write_all(b"\n")?;
    Ok(())
}
