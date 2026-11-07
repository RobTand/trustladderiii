# RP #930 Option B

The kernel-owned, request-bound `ReplanBudget` is persisted in the existing controller request row, outside every World-reachable type. `PlannerResult` is separate from kernel Event and Reply. World and Node remain opaque.

The kernel consumes permission only for an admitted empty-allowed planner completion. First use starts a fresh Planning generation; a second use stops for review. Wrong-key input cannot consume. Non-admitted bound completions delegate to generic reply processing so stopping release and stale settling remain intact. Ordinary planner replies retain schema and footprint admission and map accepted semantic failures to review after consumption. Both fail_fast settings retain this policy.

New request versions initialize their own unused budgets at creation. Planner inputs load their own request row, checking pending writes before the snapshot. A rewritten raw legacy node row imports consumed=true conservatively before serialization drops the old flag. Unrelated legacy request rows remain untouched. Budget, World, outputs and journal commit atomically; planner conflicts retain the input for fresh-snapshot reapplication.

The production controller, persistence and replay are wired. Development entry/helper proofs, witnesses, runtime/store checks and source mutants are recorded in the RP B-production handback. The reviewed producer is eeaa5e3d; parent review corrections follow on a distinct branch. These subsets do not qualify the integrated head. Actual-main integration, full proof/resource gates and separately captain-authorized Kani remain pending. No database migration, deployment or held-probe release is authorized.
