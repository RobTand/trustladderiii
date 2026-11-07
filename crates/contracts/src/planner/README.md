# Planner slice 1 preparation

Status: design preparation, blocked before Rust implementation and registration.
Found by: spec-writing.

This directory reserves planner-owned proposal and evaluation work. It contains
no Rust module, replacement schema, alternate refusal representation, or runner.
The harness owns all existing shared contracts files until its slice 1 lands and
the parent explicitly releases the lease. A settled API does not release that
lease.

## Existing flow and the required ownership change

`workers/src/tl3_workers/runtime.py::_run` constructs `PlannerContract`, invokes
the Python model loop, and emits its `plan_check` provenance beside the reply.
`PlannerContract.check` validates `planner_schema_reply(reply)` and then calls
`check_reply`. The schema helper fills incomplete proof declarations with a
validation-only message. `check_reply` gathers observations, computes judgments,
and rejects model-supplied code evidence after schema validation.

`crates/controller/src/adapters/workers/protocol.rs::map_reply_with_plan_check`
accepts optional worker provenance. Its ordinary planner mapping reads forecast
and violation fields from the reply. None of these call sites changes in this
preparation. Their eventual replacement is one Rust session, with Python
returning model responses and Python API observations to that session.

Contracts defines the boundary without depending on controller, kernel,
harness-core, the filesystem, or an executor. The controller binds request and
base identity, gathers authoritative observations, stores session state, and
produces the final decision. A model response cannot supply that decision.

## Two candidate boundaries

One candidate keeps the current augmented planner reply as the model type and
checks its code-owned fields after parsing. This retains the ownership ambiguity
in the current reply. Reject it.

The selected design gives the model one proposal root with `leaf`, `split`, and
`cant_meet` variants. Rust stores observations and decisions separately. Model
JSON rejects unknown fields, including fields belonging to observations or
decisions. This design still needs agreement on raw check declarations before a
complete proposal can be implemented. No partial proposal type or generic check
placeholder stands in for that agreement.

## Proposal fields and the unresolved check binding

| Variant | Model declarations |
| --- | --- |
| `leaf` | Files, check-to-goal associations, `no_design_questions`, `lands_on_its_own`, required `estimated_exploration_turns`, optional `estimated_tool_turns`, and optional `edits_tests` |
| `split` | Reason, pieces with brief/files/goal indexes, required exploration estimates on the root and each piece, and optional total tool estimates |
| `cant_meet` | Report, files, optional protected-path claims, and optional estimates |
| All variants | Optional supplied-memory IDs, memory flags, and convention claims with source quotes |

Split reasons remain `does_not_fit`, `execution_budget`, `multiple_repos`, and
`out_of_room`. Explicit zero is a valid exploration declaration. An absent
required exploration declaration has no old-worker bypass. Schema parsing alone
does not establish coverage, fit, or acceptance.

Reuse `bounded`, `footprint::Footprint`, existing input contracts, and ingest's
memory-flag and source-quote types. Source quotes in a proposal are claims for
Rust to check against source, not accepted observations. Paths used as an
accepted footprint must pass the existing canonical footprint boundary.

The final program-lead ruling is one type per trust level. Harness publishes
`CheckDeclaration` for model output in slice 1. It carries goal association,
exact selectors, planner-only kinds, and model-safe proof subject and support
selection. It has no `Reproducing`, `red_message`, or captured evidence fields.
The owner has the required `goal_ids` semantics, but the final canonical API
is not published at this preparation's base.

Existing `Check` is trusted and admitted. Only Rust validation or
controller-owned sources produce it. Never parse it from model output.
Proof declarations have the same trust split. Captured `red_message` and
evidence come only from actual runs. This ruling supersedes the earlier direct
raw-model `Check` proposal; that earlier proposal is not an implementation path.

The remaining check binding is a dependency STOP:

- Bind the raw proposal only after the owner publishes canonical
  `CheckDeclaration` and the model-safe proof declaration API. Do not copy
  either type, introduce a nested Check or wrapper, or add selector aliases.
- Preserve planner subject-selection ability and exact required goal
  association semantics through the published types. Keep planner kinds to
  `Demonstrating`, `Regression`, and `BehaviorPreserving`.
- Represent missing declarations explicitly so the Rust session can offer the
  existing bounded correction. Never create a valid-looking proof placeholder.
- Rust admits checks using authoritative validation and actual observations.
  Model declarations never establish captured RED output, execution, a verified
  proof, or a receipt.

The trust boundary is final, not another design exploration. The raw-model
binding remains held until its canonical API is published. This unit neither
implements the superseded direct-Check shape nor changes shared types.

## Session facts preserve inherited evidence obligations

The controller supplies existing `PlannerInput`, request/base identity, and the
inherited evidence contract as authoritative facts. In particular,
`PlannerInput.audit_evidence_contract` and its requirement provenance, source
paragraphs, and verification methods survive the removal of the model's
`evidence_contract` field. The session cannot reconstruct them from a refusal
string or accept a model-authored replacement.

Availability of evidence and satisfaction of a requirement are separate facts.
If required observations are absent, the session retains the obligation and
refuses acceptance. Source context does not become verification evidence merely
because the model cited it. Method and source preservation remain EP/EP2
acceptance cases. An absent input field is never evidence that all inherited
requirements were met; the controller must establish the complete obligation
set for the bound request before it can decide.

The following values belong only to Rust facts or decisions, outside the model
proposal: forecast judgments, execution estimates derived by code, validation
outcomes, resolved violations, inherited decision context and evidence contract,
evidence errors, captured RED text, admitted rehearsal receipts, and provenance.
An empty model `violations` array or a model `fits: true` cannot authorize a plan.

## Planner-owned evaluation against canonical rows

The parent settled `crates/contracts/src/rules.rs` as the sole canonical API:
`Role`, `RenderContext`, `FieldPointer`, `NameKind`, `Name`, `Allowed`,
`SchemaKeyword`, `EditOp`, `Repair`, `Evidence`, `AttributionMethod`, `Violation`,
`TerminalForm`, `SchemaMembership`, `Policy`, `render_all`, `permits`, `schema`,
and `generated`. Harness owns these data types, validation, rendering, and
permissions. Planner consumes them as-is.

Planner owns `Callback`, `Witness`, `Registration`, `Rule`, ordered evaluation,
and planner role-table integration. These algorithms can be extracted from
`dc59ebfb78d223ed30a8f3d1628f71b1d39b95f1:crates/contracts/src/rules.rs`
after the landed canonical API is available. No row data or canonical renderer
is copied into planner. The corrected source is reference material, not a
reviewed current-main candidate.

The proposed callable surface follows the existing C0 algorithms:

```text
Rule<F>::register(self, fixture: &Registration<F>, policy: &Policy)
    -> Result<Rule<F>, String>
Rule<F>::evaluate(&self, facts: &F, role: Role, policy: &Policy)
    -> Result<Vec<Violation>, String>
evaluate<F>(rules: &[Rule<F>], facts: &F, role: Role, policy: &Policy)
    -> Result<Vec<Violation>, String>
```

The following calls already belong to canonical data, not planner:

```text
render_all(rows, role, render_context, policy) -> Result<String, String>
permits(rows, before, after, role, policy) -> Result<bool, String>
```

Each failed fact resolves its allowed and repair callbacks once into a
`Violation`. Guard and rendering consume those same resolved rows. Both
callbacks receive an explicit role. Ordered evaluation stops after a gating
rule emits rows. No Python membership evaluator or renderer remains after the
eventual caller switch.

Registration requires violating examples and explicit shared or per-role
witnesses. It checks every advertised finite or constant alternative. A
source-only witness is null only when all repairs for that role are source
edits. Mixed field edits and source edits use a real edited reply witness;
rechecks ignore only unresolved source edits. This preserves the C0 correction
instead of the reviewed ancestor's null-witness defect.

`Allowed::alternatives` and the registration document-edit helper were private
in the combined C0 source. Their planner-owned algorithm portions can move
with registration; private canonical helpers must not become accidental
dependencies. `Violation::validate(role, policy)` is already public in C0 and
must remain callable on the canonical type. The landed extraction still needs
an exact source comparison before implementation binds those imports.

Use canonical subject validation, terminal policy, repair order, UTF-8 limits,
and evidence omission markers. Source instructions are at most 1024 UTF-8
bytes; excerpts are at most 4096 bytes including `\n[truncated]`; evidence
records cap at 16 and outside-selection targets at 64. Planner does not
implement another cap, silently erase omission counts, or clip the whole
refusal message. Canonical membership errors propagate as errors.

## Qualification and adoption remain pending

There is no compilable planner boundary in this preparation. No shared module,
BUILD target, Cargo entry, schema inventory, or temporary runner registers this
directory. No tests, generation, formatting, proofs, models, or probes run.
No acceptance, RED/GREEN, own-child verdict, or CAS receipt is claimed.

After harness landing and explicit lease release, qualification needs:

- Model-root and schema agreement on required declarations, unknown fields,
  numeric goal identities, kind restrictions, missing proof, and source claims.
- Single callback resolution, role witnesses, real mixed Edit/SourceEdit
  registration, advertised-alternative rechecks, and ordered gating.
- Guard/render agreement using the canonical resolved rows, terminal-role
  restrictions, exact instruction and multibyte excerpt boundaries, and
  persisted omission markers.
- #592/Q49 actual-run evidence, #809 bounded coverage repair, and EP/EP2
  context-versus-verification and exact source/method preservation cases at
  their owning semantic slices. No accepted proof may use a declared message
  as its actual run result.
- Approved remote execution with source binding, the run's own verdict/logs,
  counts, and SHA/length CAS receipts. Existing proved coverage is retained or
  replaced and qualified in the same change, without new out-of-scope proofs.

At the caller switch, delete the replaced planner Python logic and its tests
in the same final slice. Until then the active Python path stays intact. This
preparation removes zero Python code lines and zero Python test lines. It is
neither a completed slice nor permission to land independently.

The parent owns `crates/plan-check` disposition in slice 2: useful rules and
proofs move into the production engine, and the old crate and oracle generators
are deleted then. This schema preparation leaves that crate and its generators
untouched.
