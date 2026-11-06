use super::*;
pub use tl3_controller::evidence::{publication, with_publication};
const BODY_BYTES: usize = 60_000;
fn publication_goals(delivery: &Delivery) -> impl Iterator<Item = &Value> {
    delivery
        .evidence
        .as_ref()
        .into_iter()
        .flat_map(|e| &e.goals)
        .filter(|goal| goal["stage"] == "publishing")
}
pub(crate) fn required_publication_lines(delivery: &Delivery) -> Vec<&str> {
    publication_goals(delivery)
        .flat_map(|goal| {
            goal["publication"]["body_lines"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
        })
        .collect()
}
pub(crate) fn publication_lines(delivery: &Delivery, body: &str) -> String {
    let mut body = body.to_owned();
    for line in required_publication_lines(delivery) {
        if !body.lines().any(|existing| existing == line) {
            body.push('\n');
            body.push_str(line);
            body.push('\n');
        }
    }
    body
}
pub(crate) fn validate_publication(
    delivery: &Delivery,
    branch: &str,
    draft: bool,
    body: &str,
) -> Result<()> {
    for goal in publication_goals(delivery) {
        let requirements = &goal["publication"];
        let branch_ok = requirements["branch"].as_str().is_none_or(|required| {
            crate::goals::branch_satisfies(
                required,
                branch,
                Some(delivery.issue),
                Some(delivery.node().0),
                false,
            )
        }) && requirements["branch_prefix"].as_str().is_none_or(|prefix| {
            crate::goals::branch_satisfies(
                prefix,
                branch,
                Some(delivery.issue),
                Some(delivery.node().0),
                true,
            )
        });
        let lines_ok = requirements["body_lines"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .all(|line| body.lines().filter(|existing| *existing == line).count() == 1);
        if !branch_ok || draft || !lines_ok {
            use crate::publication_failure::{LocalRefusal, Reason};
            let reason = if !branch_ok {
                Reason::BranchMismatch {
                    required: requirements["branch"].as_str().map(str::to_owned),
                    prefix: requirements["branch_prefix"].as_str().map(str::to_owned),
                    generated: branch.into(),
                }
            } else if draft {
                Reason::NonDraftObligation
            } else {
                Reason::BodyObligation
            };
            return Err(LocalRefusal::error("pr_obligation", reason));
        }
    }
    Ok(())
}
pub(crate) fn validate_body(body: &str) -> Result<()> {
    if body.len() > BODY_BYTES {
        return Err(crate::Error::Permanent {
            class: "pr_evidence",
            source: Box::new(crate::publication_failure::LocalRefusal(
                crate::publication_failure::Reason::BodySize,
            )),
        });
    }
    Ok(())
}
