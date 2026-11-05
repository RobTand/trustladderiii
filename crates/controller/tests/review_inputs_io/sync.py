"""Refresh the current compiled shell, never the frozen reference or base pin.

This is fixture generation, not a second implementation of any decision.
"""
from pathlib import Path

ROOT = Path(__file__).resolve().parents[4]
HERE = Path(__file__).resolve().parent
SOURCE = ROOT / 'crates/controller/src/adapters/workers/review.rs'
text = SOURCE.read_text()
start = text.index('pub(super) fn inputs(')
end = text.index('pub(crate) fn criteria(')
(HERE / 'current.rs').write_text('use super::*;\n' + text[start:end].rstrip() + '\n')

# Compile the unchanged production validator and diagnostic capture, too.
protocol = (ROOT / 'crates/controller/src/adapters/workers/protocol.rs').read_text()
parts = [
    ('fn validators()', '/// A reply checked'),
    ('pub(super) fn validate_inputs(', 'fn forecast('),
    ('fn schemas_from(', 'pub(crate) fn reply_field'),
]
(HERE / 'validation.rs').write_text('use super::*;\nuse std::sync::OnceLock;\n'
    + 'include!(concat!(env!("OUT_DIR"), "/worker_schemas.rs"));\n'
    + ''.join(protocol[protocol.index(a):protocol.index(b)] for a, b in parts).replace('pub(super) fn validate_inputs(', 'pub fn validate_real('))
failure = (ROOT / 'crates/controller/src/worker_inputs.rs').read_text()
(HERE / 'failure.rs').write_text('use serde::{Deserialize, Serialize};\n'
    + failure[failure.index('#[derive(Clone, Debug, Deserialize'):failure.index('pub(crate) fn of(')]
    + 'impl From<Failure> for tl3_controller::Error {\n'
    + '    fn from(failure: Failure) -> Self {\n'
    + '        Self::WorkerInputs(serde_json::from_value(serde_json::to_value(failure).unwrap()).unwrap())\n'
    + '    }\n}\n')
