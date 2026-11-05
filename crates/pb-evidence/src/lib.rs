//! #821: deterministic PB evidence decisions. Files and subprocesses remain in the worker.
pub mod consistency;
pub mod decision;
pub mod ending;
pub mod error;
pub mod generator;
pub mod identity;
pub mod identity_fields;
pub mod python_json;
pub mod transport;
pub mod wire;

use base64::{Engine, engine::general_purpose::STANDARD};
use ending::PbEnding;
use error::{Refusal, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Rust owns the transport contract. Python supplies facts, never a parallel model.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "operation", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Request {
    SummaryStart {
        ending: PbEnding,
        key: String,
    },
    SummaryFinish {
        ending: PbEnding,
        key: String,
        raw: String,
    },
    GeneratorStart {
        ending: PbEnding,
        key: String,
    },
    GeneratorFinish {
        ending: PbEnding,
        key: String,
        raw: String,
    },
    FailureRead {
        ending: PbEnding,
    },
    FailureReceipt {
        ending: PbEnding,
        record: Option<ending::FailedRecord>,
    },
    Capability {
        code: i64,
        raw: String,
    },
    MachineRecord {
        private: bool,
        code: i64,
        raw: String,
        detach: bool,
        #[serde(default)]
        io_error: bool,
    },
    Consistency {
        result: Value,
        expected: consistency::ExpectedRun,
    },
    Selectors {
        value: Value,
        #[serde(default = "default_true")]
        is_list: bool,
    },
    Priority {
        value: Value,
    },
    Settings {
        value: Value,
    },
    Identity {
        base_sha: Value,
        selectors: Value,
        forcing: tl3_contracts::checks::Forcing,
        declared_symptom: Value,
    },
    Canonical {
        value: Value,
    },
    PythonJson {
        value: Value,
        ascii: bool,
        compact: bool,
    },
    BoundIdentity {
        candidate: Value,
    },
    ExecutionIdentity {
        candidate: Value,
    },
    Unusable {
        result: Value,
        selectors: Vec<String>,
        outcome: String,
    },
    Violations {
        failure: Value,
        receipts: Vec<Value>,
        base_sha: Value,
        checks: Vec<Value>,
    },
    GeneratorIdentity {
        artifact: Value,
        command: String,
        image: String,
        implementation: String,
        budget: Value,
    },
    GeneratorValidate {
        artifact: Value,
        before: Value,
        contract: generator::GeneratorContract,
        #[serde(default)]
        check_identity: bool,
    },
}
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Response {
    Return(Value),
    Raise(Refusal),
}

fn default_true() -> bool {
    true
}

fn bytes(raw: &str) -> Result<Vec<u8>> {
    STANDARD
        .decode(raw)
        .map_err(|_| Refusal::value("invalid byte transport"))
}

pub fn evaluate(request: Request) -> Result<Value> {
    match request {
        Request::SummaryStart { ending, key } => {
            Ok(serde_json::to_value(ending::start(&ending, &key, false)?)
                .expect("log receipt serializes"))
        }
        Request::SummaryFinish { ending, key, raw } => {
            ending::finish(&ending, &key, &bytes(&raw)?, false)
        }
        Request::GeneratorStart { ending, key } => {
            Ok(serde_json::to_value(ending::start(&ending, &key, true)?)
                .expect("log receipt serializes"))
        }
        Request::GeneratorFinish { ending, key, raw } => {
            ending::finish(&ending, &key, &bytes(&raw)?, true)
        }
        Request::FailureRead { ending } => {
            ending::failure(&ending, None)?;
            Ok(json!(ending::failure_read(&ending)))
        }
        Request::FailureReceipt { ending, record } => ending::failure(&ending, record.as_ref()),
        Request::Capability { code, raw } => Ok(json!(transport::capability(code, &bytes(&raw)?))),
        Request::MachineRecord {
            private,
            code,
            raw,
            detach,
            io_error,
        } => transport::record(private, code, &bytes(&raw)?, detach, io_error),
        Request::Consistency { result, expected } => consistency::check(result, &expected),
        Request::Selectors { value, is_list } => {
            consistency::selectors(if is_list { &value } else { &Value::Null })
        }
        Request::Priority { value } => consistency::priority(&value),
        Request::Settings { value } => consistency::settings(&value),
        Request::Identity {
            base_sha,
            selectors,
            forcing,
            declared_symptom,
        } => Ok(identity::identity(
            &base_sha,
            &selectors,
            &serde_json::to_value(forcing).expect("forcing serializes"),
            &declared_symptom,
        )),
        Request::Canonical { value } => Ok(json!(python_json::dumps(&value, true, true))),
        Request::PythonJson {
            value,
            ascii,
            compact,
        } => Ok(json!(python_json::dumps(&value, ascii, compact))),
        Request::BoundIdentity { candidate } => Ok(json!(identity::bound(&candidate)?)),
        Request::ExecutionIdentity { candidate } => Ok(json!(identity::execution(&candidate)?)),
        Request::Unusable {
            result,
            selectors,
            outcome,
        } => identity::unusable(&result, &selectors, &outcome),
        Request::Violations {
            failure,
            receipts,
            base_sha,
            checks,
        } => identity::violations(&failure, &receipts, &base_sha, &checks),
        Request::GeneratorIdentity {
            artifact,
            command,
            image,
            implementation,
            budget,
        } => {
            generator::identity(&artifact, &command, &image, &implementation, &budget)?;
            Ok(Value::Null)
        }
        Request::GeneratorValidate {
            artifact,
            before,
            contract,
            check_identity,
        } => generator::validate(&artifact, &before, &contract, check_identity),
    }
}
pub fn decide(operation: &str, mut document: Value) -> Value {
    document["operation"] = json!(operation);
    let decision = serde_json::from_value(document)
        .map_err(|_| Refusal::value("invalid PB evidence request"))
        .and_then(evaluate);
    serde_json::to_value(match decision {
        Ok(value) => Response::Return(value),
        Err(error) => Response::Raise(error),
    })
    .expect("PB response serializes")
}
