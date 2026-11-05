//! #780/#807: unit stubs are recordings of the same authoritative Rust functions.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tl3_controller::{Error, Result, budget, classifier, postmortem_reply, reviewer};

#[derive(Deserialize, Serialize)]
pub struct Vector {
    command: Vec<String>,
    document: Value,
    #[serde(default)]
    response: Option<Box<serde_json::value::RawValue>>,
}

pub fn generate(mut vectors: Vec<Vector>) -> Result<Vec<Vector>> {
    for vector in &mut vectors {
        let response = match vector
            .command
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .as_slice()
        {
            ["check-budget"] => {
                let result = budget::decide(serde_json::from_value(vector.document.clone())?)
                    .map_err(Error::Invalid)?;
                #[derive(Serialize)]
                struct Response<T> {
                    result: T,
                }
                if vector.document["kind"] == "execution" {
                    // Use the CLI's typed diagnostic order, not Value's sorted map.
                    serde_json::value::to_raw_value(&Response {
                        result: serde_json::from_value::<budget::ExecutionEvidence>(result)?,
                    })?
                } else {
                    serde_json::value::to_raw_value(&Response { result })?
                }
            }
            ["check-pb", operation] => serde_json::value::to_raw_value(&tl3_pb_evidence::decide(
                operation,
                vector.document.clone(),
            ))?,
            ["check-reply", role] => {
                let inputs = &vector.document["inputs"];
                let reply = &vector.document["reply"];
                let verdict = match *role {
                    "classifier" => classifier::validate(inputs, reply),
                    "reviewer" => reviewer::validate(inputs, reply),
                    "postmortem" => postmortem_reply::validate(reply),
                    _ => return Err(Error::Invalid("unsupported recorded checker role")),
                };
                let violations = match verdict {
                    Ok(()) => vec![],
                    Err(Error::Invalid(message)) => vec![message],
                    Err(error) => return Err(error),
                };
                serde_json::value::to_raw_value(&json!({"violations": violations}))?
            }
            _ => return Err(Error::Invalid("unsupported recorded checker command")),
        };
        vector.response = Some(response);
    }
    Ok(vectors)
}
