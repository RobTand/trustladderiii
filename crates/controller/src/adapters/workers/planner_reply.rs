use crate::Result;
use serde_json::Value;
use tl3_kernel::Reply;

pub(super) fn map(_reply: &Value) -> Result<Reply> {
    Ok(Reply::PlanCantMeet)
}

#[cfg(test)]
#[path = "planner_reply_tests.rs"]
mod tests;
