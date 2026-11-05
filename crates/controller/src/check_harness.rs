//! #823: bounded strict transport for the worker's shared harness decisions.

use std::io::{Read, Write};

pub fn decide(_kind: &str, _document: serde_json::Value) -> crate::Result<serde_json::Value> {
    Ok(serde_json::json!({"not_implemented": true}))
}

pub fn command(kind: Option<&str>) -> crate::Result<()> {
    let Some(kind @ ("summary" | "identity")) = kind else {
        return Err(crate::Error::Invalid(
            "expected check-harness summary or identity",
        ));
    };
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(32 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 32 * 1024 * 1024 {
        return Err(crate::Error::Invalid("harness request is too large"));
    }
    let document = crate::adapters::workers::json::parse(&bytes)
        .map_err(|_| crate::Error::Invalid("invalid check-harness JSON"))?;
    let result = decide(kind, document)?;
    serde_json::to_writer(std::io::stdout().lock(), &result)?;
    std::io::stdout().write_all(b"\n")?;
    Ok(())
}
