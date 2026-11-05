use crate::ending::RECORD_LIMIT;
use crate::error::{Refusal, Result, parse};
use crate::python_json::integer;
use serde_json::{Value, json};

pub fn capability(code: i64, raw: &[u8]) -> bool {
    if code != 0
        || raw.len() > 4096
        || !raw.ends_with(b"\n")
        || raw.iter().filter(|b| **b == b'\n').count() != 1
    {
        return false;
    }
    std::str::from_utf8(raw)
        .ok()
        .and_then(|raw| parse(raw).ok())
        .is_some_and(|value| {
            value.as_object().is_some_and(|map| map.len() == 2)
                && value["schema"] == "prismabuild.machine_record_capability.v1"
                && integer(&value["machine_record_fd"]).is_some_and(|version| version >= 1)
        })
}

pub fn record(private: bool, code: i64, raw: &[u8], detach: bool, io_error: bool) -> Result<Value> {
    let phase = if private {
        "prismabuild_channel"
    } else {
        "prismabuild_stdout"
    };
    let prefix = if private {
        "machine_record"
    } else {
        "stdout_record"
    };
    let failure =
        |suffix: &str, cause| Refusal::transport(phase, &format!("{prefix}_{suffix}"), cause);
    if io_error {
        return Err(failure("io_failed", None));
    }
    if code < 0 {
        return Err(failure("command_killed", None));
    }
    if raw.is_empty() && code != 0 && detach {
        return Err(Refusal::transport(
            "prismabuild_submission",
            &format!("submission_refused_exit_{code}"),
            None,
        ));
    }
    if private && raw.is_empty() {
        return Err(failure("empty", None));
    }
    if raw.len() > RECORD_LIMIT {
        return Err(failure("too_large", None));
    }
    if private && !raw.ends_with(b"\n") {
        return Err(failure("unterminated", None));
    }
    let text = std::str::from_utf8(raw)
        .map_err(|e| failure("invalid_json", Some(Refusal::utf8(raw, e))))?;
    let value = parse(text).map_err(|error| failure("invalid_json", Some(error)))?;
    if !value.is_object() {
        return Err(failure("not_object", None));
    }
    Ok(json!([code, value]))
}
