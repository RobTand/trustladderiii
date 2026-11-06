# EP decisions

found by: live probe

- Work from origin/rust-fv on t-ep-route-prose-context. No PR or subagents.
- Use the ticket's existing Python refusal-row adapter. This is the explicitly requested pre-Rust rule table, not a new parallel Rust/Python implementation. No schema, kernel, state table, controller routing, or core row changes.
- Structured methods distinguish verification obligations from runner mechanics where possible. Free-text goals and audit objections have no structured discriminator. Keep the conservative fallback patterns in one documented table. Unknown or mixed verification text remains a requirement.
- Use one constant, complete-reply Edit per route violation group. This atomically preserves the verbatim text in the ingest brief and removes route-only requirements. A const replacement grants no unrelated edits. The core already supports the empty JSON pointer for the reply root.
- Audit cannot edit the ingest input. Its repair removes its own route-only contract entries or objections. The original issue remains its context; ingest is responsible for preserving source prose in its brief. Do not invent an audit context field.
- Keep verification methods, named-test outcomes, assertion RED, GPU/served validation, and performance statements. In particular PQ #2798 remains a requirement.
