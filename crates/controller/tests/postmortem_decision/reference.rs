// Verbatim function from 4a7fa87a, retained only in this integration test.
use super::decision::*;

/// Q32 / #403: refuse excess recommendations before checking their operations.
pub fn decide(reply: Reply) -> Result<(), Violation> {
    let Reply::Recommendations(items) = reply else {
        return Err(Violation::TooMany);
    };
    for item in items {
        let Some(item) = item else { continue };
        let valid = match item.operation {
            Operation::Add => !item.before_nonempty && item.after_nonempty,
            Operation::Remove => item.before_nonempty && !item.after_nonempty,
            Operation::Replace => item.before_nonempty && item.after_nonempty,
        };
        if !valid {
            return Err(Violation::OperationText);
        }
    }
    Ok(())
}
