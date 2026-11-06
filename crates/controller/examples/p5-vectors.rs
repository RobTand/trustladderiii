//! P5 batch oracle used to drift-check worker recordings without repeated processes.
use serde_json::{Value, json};
use tl3_controller::{Error, Result, input_check, quota};

fn main() -> Result<()> {
    let mut rows: Vec<Value> = serde_json::from_reader(std::io::stdin().lock())?;
    for row in &mut rows {
        let document = &row["document"];
        let command: Vec<String> = serde_json::from_value(row["command"].clone())?;
        let response = match command
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .as_slice()
        {
            ["check-inputs", "request"] => json!({"violations": input_check::request(document)?}),
            ["check-inputs", "maker"] => json!({"violations": input_check::maker(document)?}),
            ["check-quota"] => {
                let facts: quota::Facts = serde_json::from_value(document["facts"].clone())?;
                let reset = if document["kind"] == "reset" {
                    facts
                        .links
                        .first()
                        .and_then(|link| quota::reset(link, &facts.now))
                } else {
                    quota::decide(&facts)
                };
                json!({"reset_ms": reset, "grade_judge": quota::grade_judge(&facts)})
            }
            _ => return Err(Error::Invalid("unsupported P5 checker command")),
        };
        row["response"] = response;
    }
    serde_json::to_writer(std::io::stdout().lock(), &rows)?;
    Ok(())
}
