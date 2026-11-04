use super::decision::*;

/// Q32 / #403: the count cap and all operation/text combinations, in repair order.
#[kani::proof]
// Exactly three optional slots plus the terminating loop check. Larger replies
// reduce to TooMany before parsing, so this covers every recommendation count.
#[kani::unwind(4)]
fn postmortem_total_and_sound() {
    let reply: Reply = kani::any();
    let mut expected = Ok(());
    let mut present = false;
    let mut add = false;
    let mut remove = false;
    let mut replace = false;
    match reply {
        Reply::TooMany => expected = Err(Violation::TooMany),
        Reply::Recommendations(items) => {
            for item in items {
                let Some(item) = item else { continue };
                present = true;
                add |= item.operation == Operation::Add;
                remove |= item.operation == Operation::Remove;
                replace |= item.operation == Operation::Replace;
                // Independent truth table, including empty and nonempty text.
                let valid = matches!(
                    (item.operation, item.before_nonempty, item.after_nonempty),
                    (Operation::Add, false, true)
                        | (Operation::Remove, true, false)
                        | (Operation::Replace, true, true)
                );
                if !valid {
                    expected = Err(Violation::OperationText);
                }
            }
        }
    }
    let actual = decide(reply);
    assert_eq!(
        actual, expected,
        "postmortem must satisfy count and operation rules"
    );
    kani::cover!(actual == Err(Violation::TooMany), "count cap");
    kani::cover!(actual.is_ok() && !present, "no recommendations");
    kani::cover!(actual.is_ok() && add, "valid add");
    kani::cover!(actual.is_ok() && remove, "valid remove");
    kani::cover!(actual.is_ok() && replace, "valid replace");
    kani::cover!(
        actual == Err(Violation::OperationText) && add && !remove && !replace,
        "invalid add"
    );
    kani::cover!(
        actual == Err(Violation::OperationText) && remove && !add && !replace,
        "invalid remove"
    );
    kani::cover!(
        actual == Err(Violation::OperationText) && replace && !add && !remove,
        "invalid replace"
    );
}
