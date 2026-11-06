# EP hand-back

## STATUS

Implementation complete on the preserved EP branch history. Final-head qualification
is recorded separately in `/home/rob/tmp/tl3-build/rust-fv/planner/contracts/EP/handback.md`
and its logs. Do not treat preparation success or historical red evidence as final
qualification. The external handback names the exact qualified head and verdicts.

found by: live probe

The adapter uses field-level constant edits. It keeps route text verbatim in the
ingest brief, removes route-only goals/contracts and audit objections, and guards
before preflight mutates the reply. Audit inheritance cannot restore route-only
contracts. Unknown/mixed obligations and structured verification remain mandatory.
No core row API, schema, kernel, state table or controller-routing change.

Preparation: https://app.buildbuddy.io/invocation/d45db31d-f1d0-424d-808d-bdb2777669db
completed successfully. Existing generators refreshed only ingest/audit identity
vectors, current P4 corpus rows and four real-Rust prompt recordings. Other six
roles compare byte-identical. Frozen reference sources are unchanged. BUILD
regeneration and Python lint/requirements checks passed without generated BUILD edits.
The remote driver wraps existing generators; it adds no application target or seam.

Clippy is NOT APPLICABLE: EP changes no Rust source or Cargo file, per the
program-lead lint ruling. PB remains held for actual PB work. The obsolete
`crates/controller/tools/redaction_policy.py` is absent and was not restored.
Its current replacement is `tl3 redaction-policy --check/--identity-check`.
The final controller consumer tests exercise the current redaction contracts.

Branch: `t-ep-route-prose-context`. Last supplied checkpoint
`8d2fc6951c711a7dd8c6484296f32849fdf1a5ee` remains an ancestor.
Red commit: `4f112e70fb5e3c80c1bb16fd0b7a15f6e3364b2d` remains an ancestor.
No PR, deployment, live configuration/store/daemon change, model call or restart.

## Fixtures and red/green

| Original fixture | Red result | Green |
| --- | --- | --- |
| PQ #2648 | Ingest, audit, and goal assertions fail because no route repair row exists | Not implemented |
| PQ #2629 | Ingest, audit, and goal assertions fail because no route repair row exists | Not implemented |
| PQ #3299 | Ingest, audit, and goal assertions fail because no route repair row exists | Not implemented |
| PB #2783 | Ingest, audit, and goal assertions fail because no route repair row exists | Not implemented |
| PB #2823 | Ingest, audit, and goal assertions fail because no route repair row exists | Not implemented |
| PQ #2798 negative | Ingest and audit controls pass; GPU/served validation remains a requirement | No production change yet |

Excerpts came from the original `.md` fixtures under `/home/rob/tmp/tl3-build/ops/fixtures/exec-prose/`, not the cleaned copies. Tests keep route and verification excerpts separately. No traces or credentials were copied.

## Runs

No runs are in flight.

Completed RED parent invocation: https://app.buildbuddy.io/invocation/c43f0ab8-b432-42aa-9d24-500c797453d5

Child test invocation: https://app.buildbuddy.io/invocation/6b4a0b78-8fcb-48f3-97a9-52aef4eb9c3b

The parent reached COMPLETE with success=false; the child reached COMPLETE with TESTS_FAILED. All 15 positive assertions failed for the intended missing-row behavior. The sharded target also ran surrounding tests, with 149 passed and one existing xfailed. The requested Bazel `--test_filter=exec_prose` did not narrow the Python runner as expected. No new skip or xfail was added. Evidence was read through `bb_logs.py` and the hardened client's terminal verdict. Raw logs are in external `EP/red.log`.

## Historical checkpoint notes (superseded by STATUS)

Read `ticket-EP.md` and `row-form.md` again before continuing. The existing entry point is `workers/src/tl3_workers/evidence.py::contract_violations`; it currently registers only CONTRACT_MISSING. Role checks call `preflight` from `roles/ingest.py` and `roles/audit.py`. The tool-free repair guard consumes `rule_violations` and `rule_role` on safe exceptions.

Next: implement the route-context row and wire its safe repair exception before preflight mutates the model reply. Keep the phrase fallback in one documented row; use structured verification methods and tool_capabilities conservatively, and retain unknown or mixed verification statements. Tests already assert the row names `/evidence_contract/0`, preserves verbatim route text in the ingest brief, removes route-only goals and audit objections, and rejects unrelated reply edits.

Proposed, not implemented: a const complete-reply Edit at the empty root JSON pointer atomically moves prose and removes requirements without changing rules.py. The core already supports this. Reassess composition with independent schema repairs before committing to it. Audit cannot edit its ingest input or add a context field; source context remains in the original issue and ingest brief. Handle inherited evidence contracts carefully: preflight currently unions them back into audit replies.

Decisions are in `EP/decisions.md`. The Python placement was proposed as the ticket's explicitly requested pre-Rust row adapter, but has not been implemented or validated. Confirm it against the Rust-first brief before adding deterministic matching logic.

## Historical checkpoint verification gaps

No green run, BUILD regeneration, prompt-identity generation, ingest/audit suite, full workers suite, controller worker sibling target, controller prompt_render, graph impact, lock check, lint, format, or clippy evidence exists for this WIP. Prompt diff is empty; all prompt and identity files are unchanged. Before final evidence, fetch and check ancestry against the current origin/rust-fv, obey BB-HOLD, and run the ticket's final checks through the authorized remote/admission paths. Do not open a PR.
