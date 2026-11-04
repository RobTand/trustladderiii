use std::{
    io::{self, Read, Write},
    path::Path,
};

pub fn run(command: &str, args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    match (command, args) {
        ("redact", []) => {
            let mut input = String::new();
            io::stdin()
                .take(1024 * 1024 + 1)
                .read_to_string(&mut input)?;
            io::stdout().write_all(tl3_controller::redaction::text(&input)?.as_bytes())?;
        }
        ("redaction-policy", []) => {
            io::stdout().write_all(tl3_controller::redaction::generated_policy().as_bytes())?;
        }
        ("redaction-policy", [flag, file]) if flag == "--check" || flag == "--output" => {
            let expected = tl3_controller::redaction::generated_policy();
            if flag == "--check" {
                if std::fs::read_to_string(Path::new(file))? != expected {
                    return Err("redaction policy drift; regenerate with tl3 redaction-policy --output FILE".into());
                }
            } else {
                std::fs::write(Path::new(file), expected)?;
            }
        }
        _ => {
            return Err(
                "usage: tl3 redact | redaction-policy [--check FILE | --output FILE]".into(),
            );
        }
    }
    Ok(())
}
