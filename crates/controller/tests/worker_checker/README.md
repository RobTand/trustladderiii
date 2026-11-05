# Recorded worker decisions

Worker unit targets use a per-test script captured from the real Rust checker.
The script checks command and budget-operation order and requires every recorded
step to be consumed. It deliberately does not recompute decisions from new facts.
Unknown or additional calls fail. The shim's failure and malformed-output tests
still install their own executables or subprocess doubles.

`checker_stub.py --record <tl3> <pytest selections>` captures scenarios, including
child-worker calls. `checker_stub.py --generate <checker-vectors example>` derives
the responses from the real Rust functions. The example calls `budget::decide`,
`classifier::validate`, `reviewer::validate` and `postmortem_reply::validate`.
The compressed JSON stores the original facts as well as each response, with a
fixed gzip timestamp. No Python implementation of the decisions exists here.
Run generation only inside a BuildBuddy or admitted PrismaBuild action.

`//crates/controller:test_test_worker_checker` checks every vector against Rust,
runs the generator's `--check`, and proves that a corrupted verdict is rejected.
The existing `budget_parity` target still runs old Python and new Rust through
the real recorded I/O interface, including durable-stage crashes and clipping.
`test_test_workers` still exercises real end-to-end worker processes.

The assertions below remain in their Python source files. Their execution
against the real checker now belongs to
`//crates/controller:test_test_worker_budget_orchestration`. The worker unit
versions replay their scenarios instead. No assertion was deleted or weakened.

| Python module | Assertions exercised against the real checker |
| --- | --- |
| test_exploration | Turn/input exhaustion, final-only reply, retained tool pairs and clipping |
| test_maker_budget | Reserve, hidden tools, progress warnings, restore, exhaustion and diagnostics |
| test_maker_compaction | Repeated compaction, ineffective compaction and invalid-checkpoint recovery |
| test_maker_context | Exact cutoff before dispatch, fallback context and preserved edits |
| test_maker_forecast | Measured file fit, tokenizer fallback, planner/maker consistency |
| test_maker_reasoning | Reasoning exclusion and long reasoning-only turns |
| test_maker_turn_budget | Long transcript bounds and honest unfinished work |
| test_maker_sessions | Session decisions, checkpoint reuse and summary limits |
| test_planner_forecast | Forecast arithmetic, limits and byte-identical warning presentation |
| test_progress_budget | Backstop, stalled progress, reserve and restored progress identities |
| test_prompt_cache | Cache-weight accounting and provider usage observations |
| test_turns | Drop/retry/repair boundaries and the budget checker within turn orchestration |

Every hand-back includes graph-impact against the integration base and the PR
base. Worker reach must stay at or below 65 with no allowlist additions.
