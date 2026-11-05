//! Audit reply wire contract (#730 C3).
use super::wire::*;

wire_link!(
    AuditReplyAgreeEvidenceContract,
    super::ingest::EvidenceContract,
    "ingest-reply.json#/$defs/evidence_contract"
);

wire_object!(AuditReplyAgree,["objections"],["type","properties","required","additionalProperties"],vec![],{
objections: List<Text<1,UNBOUNDED>,0,0,false>, ["type","items","maxItems"], vec![];
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
protected_paths: Option<List<Text<1,UNBOUNDED>,1,UNBOUNDED,true>>, ["type","items","minItems","uniqueItems","description"], vec![("description",Doc::atom("Concrete repository-relative paths that MUST change to meet the request's goals. Report only when the whole request cannot land usefully without these edits; never for inspected files, excluded edits, or an optional part. The controller verifies the effective protected-path list."))];
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
evidence_contract: Option<AuditReplyAgreeEvidenceContract>, ["$ref"], vec![];
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
evidence_errors: Option<List<Text<1,8192>,0,1024,true>>, ["type","items","uniqueItems","maxItems","description"], vec![("description",Doc::atom("Code-owned unsupported evidence requirements."))];
});

wire_object!(AuditReplyDisagree,["objections"],["type","properties","required","additionalProperties"],vec![],{
objections: List<Text<1,UNBOUNDED>,1,UNBOUNDED,false>, ["type","items","minItems"], vec![];
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
protected_paths: Option<List<Text<1,UNBOUNDED>,1,UNBOUNDED,true>>, ["type","items","minItems","uniqueItems","description"], vec![("description",Doc::atom("Concrete repository-relative paths that MUST change to meet the request's goals. Report only when the whole request cannot land usefully without these edits; never for inspected files, excluded edits, or an optional part. The controller verifies the effective protected-path list."))];
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
evidence_contract: Option<AuditReplyAgreeEvidenceContract>, ["$ref"], vec![];
#[serde(default,deserialize_with="present",skip_serializing_if="Option::is_none")]
evidence_errors: Option<List<Text<1,8192>,0,1024,true>>, ["type","items","uniqueItems","maxItems","description"], vec![("description",Doc::atom("Code-owned unsupported evidence requirements."))];
});

wire_tagged!(AuditReply, "verdict", {Agree(AuditReplyAgree) => ("agree",0,0),Disagree(AuditReplyDisagree) => ("disagree",0,0)});

pub fn document() -> Doc {
    let mut s = AuditReply::wire_schema();
    s.set(
        "$schema",
        Doc::atom("https://json-schema.org/draft/2020-12/schema"),
    );
    s.set(
        "$id",
        Doc::atom("https://trustladderiii.invalid/workers/protocol-1/audit-reply.json"),
    );
    s.ordered(&["$schema", "$id", "oneOf"])
}
