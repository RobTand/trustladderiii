use crate::bounded::{Text as BoundedText, *};
use crate::render::Layout;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
#[schemars(transform = reply_constant_schema)]
pub enum AuditReplyAgreeVerdict { #[serde(rename = "agree")] Value }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline, transform = reply_constant_schema)]
pub enum AuditReplyDisagreeVerdict { #[serde(rename = "disagree")] Value }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IngestExternalEvidenceContract(pub super::ingest_reply::EvidenceContract);
impl JsonSchema for IngestExternalEvidenceContract {
    fn inline_schema() -> bool { true }
    fn schema_name() -> std::borrow::Cow<'static,str> { "IngestExternalEvidenceContract".into() }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema { schemars::json_schema!({"$ref": "ingest-reply.json#/$defs/evidence_contract"}) }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct AuditReplyAgree {
    pub verdict: AuditReplyAgreeVerdict,
    pub objections: List<BoundedText<1, UNBOUNDED>, 0, 0, false>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "List<BoundedText<1, UNBOUNDED>, 1, UNBOUNDED, true>", default)]
    #[schemars(description = "Concrete repository-relative paths that MUST change to meet the request's goals. Report only when the whole request cannot land usefully without these edits; never for inspected files, excluded edits, or an optional part. The controller verifies the effective protected-path list.")]
    pub protected_paths: Option<List<BoundedText<1, UNBOUNDED>, 1, UNBOUNDED, true>>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "IngestExternalEvidenceContract", default)]
    pub evidence_contract: Option<IngestExternalEvidenceContract>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "List<BoundedText<1, 8192>, 0, 1024, true>", default)]
    #[schemars(description = "Code-owned unsupported evidence requirements.")]
    pub evidence_errors: Option<List<BoundedText<1, 8192>, 0, 1024, true>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct AuditReplyDisagree {
    pub verdict: AuditReplyDisagreeVerdict,
    pub objections: List<BoundedText<1, UNBOUNDED>, 1, UNBOUNDED, false>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "List<BoundedText<1, UNBOUNDED>, 1, UNBOUNDED, true>", default)]
    #[schemars(description = "Concrete repository-relative paths that MUST change to meet the request's goals. Report only when the whole request cannot land usefully without these edits; never for inspected files, excluded edits, or an optional part. The controller verifies the effective protected-path list.")]
    pub protected_paths: Option<List<BoundedText<1, UNBOUNDED>, 1, UNBOUNDED, true>>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "IngestExternalEvidenceContract", default)]
    pub evidence_contract: Option<IngestExternalEvidenceContract>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "List<BoundedText<1, 8192>, 0, 1024, true>", default)]
    #[schemars(description = "Code-owned unsupported evidence requirements.")]
    pub evidence_errors: Option<List<BoundedText<1, 8192>, 0, 1024, true>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
#[schemars(inline)]
#[schemars(transform = reply_one_of)]
pub enum AuditReply {
    Agree(AuditReplyAgree),
    Disagree(AuditReplyDisagree),
}

pub fn schema_value() -> serde_json::Value {
    reply_schema_value::<AuditReply>("audit", |_| {})
}

pub fn schema_text() -> String {
    let shared = [

    ];
    crate::render::render("audit-reply.json", &schema_value(), &Layout { inline: ROOT_ORDER, shared: &shared }).expect("published reply schema order is exhaustive")
}

pub const ROOT_ORDER: &[crate::render::ObjectOrder<'static>] = &[
    ("", &["$schema", "$id", "oneOf"]),
    ("/oneOf/0", &["type", "properties", "required", "additionalProperties"]),
    ("/oneOf/0/properties", &["verdict", "objections", "protected_paths", "evidence_contract", "evidence_errors"]),
    ("/oneOf/0/properties/verdict", &["const"]),
    ("/oneOf/0/properties/objections", &["type", "items", "maxItems"]),
    ("/oneOf/0/properties/objections/items", &["type", "minLength"]),
    ("/oneOf/0/properties/protected_paths", &["type", "items", "minItems", "uniqueItems", "description"]),
    ("/oneOf/0/properties/protected_paths/items", &["type", "minLength"]),
    ("/oneOf/0/properties/evidence_contract", &["$ref"]),
    ("/oneOf/0/properties/evidence_errors", &["type", "items", "uniqueItems", "maxItems", "description"]),
    ("/oneOf/0/properties/evidence_errors/items", &["type", "minLength", "maxLength"]),
    ("/oneOf/1", &["type", "properties", "required", "additionalProperties"]),
    ("/oneOf/1/properties", &["verdict", "objections", "protected_paths", "evidence_contract", "evidence_errors"]),
    ("/oneOf/1/properties/verdict", &["const"]),
    ("/oneOf/1/properties/objections", &["type", "items", "minItems"]),
    ("/oneOf/1/properties/objections/items", &["type", "minLength"]),
    ("/oneOf/1/properties/protected_paths", &["type", "items", "minItems", "uniqueItems", "description"]),
    ("/oneOf/1/properties/protected_paths/items", &["type", "minLength"]),
    ("/oneOf/1/properties/evidence_contract", &["$ref"]),
    ("/oneOf/1/properties/evidence_errors", &["type", "items", "uniqueItems", "maxItems", "description"]),
    ("/oneOf/1/properties/evidence_errors/items", &["type", "minLength", "maxLength"]),
];
