use std::io::{Read, Write};

pub fn run(args: &[String]) -> tl3_controller::Result<()> {
    let [operation] = args else {
        return Err(tl3_controller::Error::Invalid("usage: tl3 check-pb KIND"));
    };
    if operation == "schema" {
        serde_json::to_writer_pretty(
            std::io::stdout().lock(),
            &schemars::schema_for!(tl3_pb_evidence::Request),
        )?;
        return Ok(());
    }
    let mut raw = Vec::new();
    std::io::stdin()
        .take(80 * 1024 * 1024 + 1)
        .read_to_end(&mut raw)?;
    if raw.len() > 80 * 1024 * 1024 {
        return Err(tl3_controller::Error::Invalid(
            "PB evidence request is too large",
        ));
    }
    let mut document: serde_json::Value = serde_json::from_slice(&raw)?;
    document["operation"] = serde_json::json!(operation);
    let request: tl3_pb_evidence::Request = serde_json::from_value(document)?;
    let response = match tl3_pb_evidence::evaluate(request) {
        Ok(value) => tl3_pb_evidence::Response::Return(value),
        Err(error) => tl3_pb_evidence::Response::Raise(error),
    };
    let mut output = std::io::stdout().lock();
    serde_json::to_writer(&mut output, &response)?;
    output.write_all(b"\n")?;
    Ok(())
}
