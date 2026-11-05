#[path = "../src/postmortem_reply/decision.rs"]
mod decision;
use decision::*;
use vstd::prelude::*;
verus! {
/// Q32/#403/#743: all eight original count, text and operation covers.
fn postmortem_witness(case: u8) -> (result: bool)
    requires case < 8,
    ensures result,
{
    let operation = match case { 3 | 6 => Operation::Remove, 4 | 7 => Operation::Replace, _ => Operation::Add };
    let item = Recommendation { operation,
        before_nonempty: case == 3 || case == 4 || case == 5,
        after_nonempty: case == 2 || case == 4 || case == 5 };
    let reply = if case == 0 { Reply::TooMany }
        else { Reply::Recommendations([if case == 1 { None } else { Some(item) }, None, None]) };
    let actual = decide(reply);
    match case {
        0 => actual == Err(Violation::TooMany),
        1 => actual.is_ok(),
        2 => actual.is_ok() && operation == Operation::Add,
        3 => actual.is_ok() && operation == Operation::Remove,
        4 => actual.is_ok() && operation == Operation::Replace,
        5 => actual == Err(Violation::OperationText) && operation == Operation::Add,
        6 => actual == Err(Violation::OperationText) && operation == Operation::Remove,
        _ => actual == Err(Violation::OperationText) && operation == Operation::Replace,
    }
}
}
fn main() {}
