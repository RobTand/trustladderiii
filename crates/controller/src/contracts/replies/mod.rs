//! Role replies at the worker boundary (#730 C3). The declarations generate both
//! serde types and ordered schemars schemas; presentation order preserves #648.
#[macro_use]
mod wire;
pub mod audit;
pub mod classifier;
pub(crate) mod decision;
pub mod ingest;
pub mod maker;
pub mod postmortem;
pub mod reviewer;
pub mod writer;

use serde::Serialize;
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum RoleReply {
    Audit(audit::AuditReply),
    Classifier(classifier::ClassifierReply),
    Ingest(ingest::IngestReply),
    Maker(maker::MakerReply),
    Reviewer(reviewer::ReviewerReply),
    Writer(writer::WriterReply),
    Postmortem(Box<postmortem::PostmortemReply>),
    /// WR1b replaces this with PlannerReply after WR1's planner extraction lands.
    Planner(Value),
}

impl RoleReply {
    /// Parse after the same schema check that guarded the old Value reader. Keeping
    /// it here also enforces JSON Schema's cross-field conditions at the boundary.
    pub fn parse(role: &str, value: &Value) -> crate::Result<Self> {
        crate::adapters::workers::protocol::validate_reply_schema(role, value)?;
        Self::decode(role, value)
    }

    /// Decode the Rust contract without consulting a JSON Schema validator.
    pub fn decode(role: &str, value: &Value) -> crate::Result<Self> {
        let parsed = match role {
            "audit" => serde_json::from_value(value.clone()).map(Self::Audit),
            "classifier" => serde_json::from_value(value.clone()).map(Self::Classifier),
            "ingest" => serde_json::from_value(value.clone()).map(Self::Ingest),
            "maker" => serde_json::from_value(value.clone()).map(Self::Maker),
            "reviewer" => serde_json::from_value(value.clone()).map(Self::Reviewer),
            "writer" => serde_json::from_value(value.clone()).map(Self::Writer),
            "postmortem" => {
                serde_json::from_value(value.clone()).map(|reply| Self::Postmortem(Box::new(reply)))
            }
            "planner" => return Ok(Self::Planner(value.clone())),
            _ => return Err(crate::Error::Invalid("unsupported worker role")),
        };
        parsed.map_err(|_| crate::Error::Invalid("worker reply breaks role schema"))
    }
}

pub fn generated() -> Vec<super::Generated> {
    [
        ("audit", audit::document()),
        ("classifier", classifier::document()),
        ("ingest", ingest::document()),
        ("maker", maker::document()),
        ("reviewer", reviewer::document()),
        ("writer", writer::document()),
        ("postmortem", postmortem::document()),
    ]
    .into_iter()
    .map(|(role, schema)| {
        let mut text = serde_json::to_string_pretty(&schema).expect("a schema serializes");
        text.push('\n');
        super::Generated {
            path: format!("{}/{role}-reply.json", super::SCHEMA_DIRECTORY),
            text,
        }
    })
    .collect()
}
