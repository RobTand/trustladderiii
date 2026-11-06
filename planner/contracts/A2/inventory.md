# A2 refusal inventory, before implementation

found by: spec-writing, except #929 role loss, found by: review.
Base f0908729d13279351ef68279dc47523745714b34. EP history retained.
No core API change. No child delegation. Python rows are the explicitly approved
short-lived A2 table, ported by C0/C3/C4. No new wire contract or prompt.

| Owned site | Captured facts | Row repair / allowed membership | Existing acceptance | Retained case / coverage | Deletion |
| --- | --- | --- | --- | --- | --- |
| ingest.check_memory_ids: forecast | presence of code-owned forecast | remove /forecast | model cannot supply forecast | test_workers, new interface case | plain ValueError prose |
| ingest.check_memory_ids: sized brief files | forecast input, brief kind, missing files | add /files, ingest schema's files membership | files required before Forecaster reads | test_workers interface case | plain ValueError prose |
| maker._check_reply: self_check coverage | goal ids, raw self_check | replace /self_check, every supplied id exactly once, generated reply item constraint | exact cardinality and set | test_maker invalid replies; interface table | plain ValueError prose |
| maker._check_reply: maker status | goal stage and self_check index | replace named entry with met status and schema-required evidence, retaining goal identity | maker stage requires met | request_123 in test_maker_evidence | plain ValueError prose |
| maker._check_reply: downstream status | stage and index | replace named entry with pending status and required why, retaining identity | downstream requires pending | request_123 in test_maker_evidence | plain ValueError prose |
| maker._check_reply: finding coverage | current finding ids, reproductions | replace /reproductions, exact finding coverage, generated item constraints | exact cardinality and set | review662/test_maker_review | plain ValueError prose |
| maker._check_reply: missing reproduction proof | reproduction index, tests; no permitted assertion quote | CantMeet, empty set | proof must be supplied, never invent RED | review662/test_maker_review | handwritten missing-proof prose |
| maker._check_reply: harness validation | reproduction index, named harness rule facts | CantMeet when no bounded replacement follows from facts | original Proof.parse then Check, same order | test_maker_review selector/proof cases | wrapper refusal prose; harness validation stays owned by harness |
| maker._check_reply: requested file | requested index/path, _parts result | CantMeet without a known replacement | original _parts confinement | test_maker_forecast | plain empty-file prose |
| forecast._file_tokens: noncanonical | raw spelling, unique canonical spelling if confined | planner row selects unique canonical edit or CantMeet | _parts then PurePosixPath equality, before file reads | test_planner_forecast + all F1 footprint-paths.json vectors | named prose at raise sites |
| forecast._file_tokens: not_regular, hard_cap, binary, encoding | path, measured size, reason id, hard cap | CantMeet, no usable source replacement | original descriptor confinement and incremental reads in order; Q50 propagates | #321 unsafe/binary/UTF8/large-file cases | named prose at raise sites |
| evidence.proof_applicability | check/goal index, immutable source obligation, subjects, existing proof | A1 row's existing exact proof repair or CantMeet; RED only source quote | original applicability decisions, campaign early continue | test_proof_obligations, planner suite | named prose; deferred until A1 landing |
| evidence.branch_objections | source publication requirement, actual configured branch | CantMeet, explicit source remains frozen | original branch_satisfies | worker/planner branch cases | handwritten branch-conflict text; deferred until A1 landing |
| evidence.preflight missing contract | capabilities present, missing key | add evidence_contract from row, planner uses A1 empty inherited declaration | current check_contract before mutation; inherit after | EP live fixtures; test_workers/turns | named missing-contract prose; preserve CONTRACT_MISSING/ROUTE_CONTEXT |
| evidence.preflight removed/unsupported method | source requirement, method, capability alternatives/missing dimensions | method-only bounded replacement, source obligations CantMeet | same inherited contract union, capability comparison, write order | test_proof_obligations and planner targets | named prose; deferred until A1 landing |
| generated_scope ingestion | source conflict absence, exact conflicting brief/goal field | named field replacement accepted by existing no-freeze predicate and reply field schema | same generation_scope_conflicts detection | test_turns real repair, test_workers source/quoted cases | GeneratedOutputProhibition fixed repair prose |
| generated_scope audit | original source field/quote or ingest claim | rows name source/ingest; correction targets the ingest/source, no audit reply permission | same disagree + deduplicated objections write | test_workers audit generation cases | handwritten audit objection prose |
| contracts.schema_violations constructors (#929) | existing schema constraint/pointer and active role | unchanged Edit, preserve role | same schema checks | existing schema-row target + role regression | omission of role= |
| postmortem envelope rewrap (#929) | existing violation, translated pointer and role | unchanged Edit on inner reply | same envelope boundary check | test_postmortem real invalid-assessment repair | omission of role= |

Composition: all diagnostics touching one pointer must share a constraint that
satisfies their intersection. Independent pointers compose through permits.
No source sampling or durable write is moved. Ingest retains check_contract,
preflight, prerequisites, citations, generation scope, verbatim retention, files
check, then forecasting. Maker keeps validation before commit. Forecast keeps
canonicality, ancestor stat, confined open, fstat, bounded incremental reads,
UTF8 finalization, then generated-file classification.

Design alternatives considered inline: a second worker permissions layer would
duplicate the A0 guard and is rejected. Registered rows with captured facts use
A0 permits directly. Harness validation remains at the original maker call point.

No tests deleted. Any changed assertion must retain the original rejection or
accepted repair behavior in its existing target; no parity corpus is added.

## Placement disposition from the lead

Audit's generated_scope projection and evidence branch/capability/missing-contract
projections remain code-owned audit disagree/objections at their original call
points. They are outside audit-reply model repair. B1/C4 owns structured objections.
No fake planner role, audit terminal form or alternate repair is introduced there.
Their source-projection assertions stay in test_workers/test_turns/test_proof_obligations.
The original EP route clause vocabulary is unchanged. The new #1455 clause gap is
explicitly outside A2, per the lead's 2026-11-06 direction.

A1 landed at origin/rust-fv 9aa2480605ca2e4ce8b5d647aff48b00fd0c7129.
A2 merged that actual ref without rewriting the supplied EP history. Planner
proof/path facts now feed A1's existing rows. The same resolved violations render
planner output and guard correction. Forecaster's intake notes remain code-owned
advisory projections of measured facts, with fixed condition labels, not a second
correction mechanism. Proof-applicability campaign refusals have planner tool rows;
these render in tool context and grant no reply edits or fake audit permissions.

Coverage-array errors incorporate all stage predicates into one replacement
constraint before per-entry statuses run. This prevents two diagnostics granting
incompatible edits to an array that changes length or identities. Independent
status/finding errors retain their old ordering and are collected together.

## Candidate preparation and inherited integration

The first remote preparation at 7624d61fd23f7dcff59731670a555abb4f672264
completed BUILD generation and found one new test's missing json import in lint.
The generated dependency on tl3_harness was retrieved and applied, and the import
was corrected. No test verdict is claimed for that preparation.
One earlier submission used a mistyped SHA and failed repository setup; it is
retained separately as a setup failure, never as RED.

Sampled/merged origin/rust-fv 349b85e710 before qualification. This includes
landed EP d3b9eb126, A0c, A1, F2 and harness R1. Supplied EP history and the red
commit remain ancestors. Only the A2 diff against actual origin is owned here;
inherited Rust/Cargo work does not make A2 clippy applicable. No A2 Rust/Cargo
or controller production source edit exists. No *_io pin edits are needed.
