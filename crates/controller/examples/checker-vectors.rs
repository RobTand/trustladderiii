//! Generate test-only worker verdicts without a second decision implementation.
#[path = "../tests/worker_checker/oracle.rs"]
mod oracle;

fn main() -> tl3_controller::Result<()> {
    let vectors = serde_json::from_reader(std::io::stdin().lock())?;
    serde_json::to_writer(std::io::stdout().lock(), &oracle::generate(vectors)?)?;
    Ok(())
}
