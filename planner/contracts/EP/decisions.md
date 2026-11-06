# EP decisions

found by: live probe

- Work from origin/rust-fv on t-ep-route-prose-context. No PR or subagents.
- Use the ticket's existing Python refusal-row adapter. This is the explicitly requested pre-Rust rule table, not a new parallel Rust/Python implementation. No schema, kernel, state table, controller routing, or core row changes.
- Structured methods distinguish verification obligations from runner mechanics where possible. Free-text goals and audit objections have no structured discriminator. Keep the conservative fallback patterns in one documented table. Unknown or mixed verification text remains a requirement.
- Use one constant, complete-reply Edit per route violation group. This atomically preserves the verbatim text in the ingest brief and removes route-only requirements. A const replacement grants no unrelated edits. The core already supports the empty JSON pointer for the reply root.
- Audit cannot edit the ingest input. Its repair removes its own route-only contract entries or objections. The original issue remains its context; ingest is responsible for preserving source prose in its brief. Do not invent an audit context field.
- Keep verification methods, named-test outcomes, assertion RED, GPU/served validation, and performance statements. In particular PQ #2798 remains a requirement.

- Resumed builder: use field-level constant replacements for goals, evidence_contract,
  brief, objections and verdict. This supersedes the proposed root replacement,
  which would reject independent schema repairs. Apply all row edits in table tests;
  their original output assertions are unchanged. The first development run exposed
  the old single-root-edit test helper assumption; 10 contract cases and controls
  passed, while five goal cases stopped after applying only the first field edit.
- Phrase fallback is confined to evidence.route_context: every clause must match
  the documented execution-mechanics table. Unknown/mixed clauses, nonempty methods,
  protected edits, base/head runner features and unknown artifacts stay requirements.
  Advertised tool capabilities supply executor facts; unavailable verification is
  never removed to make preflight pass. No core row API change.
