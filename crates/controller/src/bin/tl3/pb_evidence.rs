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
    let response = tl3_pb_evidence::wire::respond(
        operation,
        std::str::from_utf8(&raw)
            .map_err(|_| tl3_controller::Error::Invalid("invalid PB evidence JSON"))?,
    )?;
    let mut output = std::io::stdout().lock();
    serde_json::to_writer(&mut output, &response)?;
    output.write_all(b"\n")?;
    Ok(())
}
