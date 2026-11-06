# Facts extraction

Classification: **behaviour-identical (standing rule)**.

The reference module preserves the complete pre-extraction `fact_transition`
and `ci_failed` bodies from a7e6226246324c3b1bcf4003bad4f64051265d21,
including their proof macros. Six handler-calling match arms move into private
`fact_*` helpers. Priority, Conflict and no-op arms stay inline. The admitted
CI branches move into `ci_failed_round` and `ci_failed_infrastructure`.
Admission and pre-enqueue cancellation remain in the wrapper, in their original
order. The non-infrastructure branch's terminal return moves with its body;
the wrapper dispatches the mutually exclusive branches.

Run `python3 crates/kernel/tools/facts_extraction/tokens.py` remotely to emit the
token-level diff for each move. The comparison erases only comments and ghost
macros, then checks every executable token in order, including return statements.
Each recorded diff must be empty. Public state clauses are unchanged. The opaque
`facts_transition_lemmas::post` bundles those existing state clauses only.
Report clauses remain separate, one per function.

`facts_equivalence` compares the old and new world, every outbox slot, report
prefix and suffix, and panic behavior. The deterministic sweep covers all fact
variants, phases, CI paths, cancellation, retries, fail-fast, incidents and
inactive branches. The 512-case proptest varies counters, ownership and payloads.

Qualification cases `retire_pr_closed` and `facts_ci_failed_dispatch` now select
the helpers containing their mutation. The first rejects a state update mutant
at a Verus obligation; `facts_extract_duplicate` duplicates a notice and must
fail the executable equivalence assertion. Evidence is recorded externally in
the R1 hand-back, after remote qualification.
