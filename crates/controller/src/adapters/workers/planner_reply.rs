use crate::{Error, Result, contracts::roles::planner_reply::PlannerReply};
use serde_json::Value;
use tl3_kernel::Reply;

pub(super) fn map(reply: &Value) -> Result<Reply> {
    // Preserve Q54's error precedence: forecast evidence precedes violations and
    // planner decisions. The schema boundary has already checked the wire shape.
    super::forecast(reply)?;
    let violations = reply["violations"]
        .as_array()
        .ok_or(Error::Invalid("planner violations missing"))?;
    if !violations.is_empty() {
        return Ok(Reply::PlanCantMeet);
    }
    let typed: PlannerReply = serde_json::from_value(reply.clone())
        .map_err(|_| Error::Invalid("worker reply breaks role schema"))?;
    Ok(match typed {
        PlannerReply::CantMeet(_) => Reply::PlanCantMeet,
        PlannerReply::Split(_) => Reply::PlanSplit,
        PlannerReply::Leaf(leaf) => Reply::PlanLeaf {
            footprint: tl3_kernel::Footprint::default(),
            fits: leaf.forecast.is_some_and(|forecast| forecast.fits),
        },
    })
}

#[cfg(test)]
#[path = "planner_reply_tests.rs"]
mod tests;
