# #805: prove unbounded progress over the executable kernel

## Result required

For a consistent, settled world with an open request, prove in Verus that an
actual outside event strictly decreases a well-founded global rank after the
real `step` function and its settle loop. Include executable witness selection.
Keep the enforced `headroom_ok` and fresh-command scope from #771. Q30 and Q39
accept starvation; this theorem does not promise fairness for an individual
request. Keep existing Kani progress harnesses until gated replacement proofs
cover their full assertions.

The delivered #771 safety proofs establish per-step counter bounds, safe
arithmetic, command-ID monotonicity, checked command construction, and raw
Command rejection. They do not establish outside-event progress. The current
[theorem mapping](progress-counterexample.md#proof-migration-status) lists the
remaining phase and gate obligations.

## Preserved draft

Commit `2668baf75a48661d49c7733c565c057c99d0988e` preserves the source-connected
progress attempt. It is an ancestor of `t771-verus-kernel-progress`; the local
reference `t771-progress-draft-20261104` also names it. Its unfinished progress
contracts are excluded from the delivered safety inventory. The proof follows
the real `World::dependency_fact`, `on_fact`, `apply`, and settle bodies, rather
than the spike's projected transition model.

## Stuck obligations and measured cost

The remaining composition obligations occur in `World::dependency_fact` and
`on_fact`. A dependency update must supply a strict whole-world rank decrease
while preserving the other nodes and edges. Local exact-update and supporting
frame lemmas verify, but the handler compositions exhaust the solver resource
limit. The draft is not a proof of those postconditions.

Attempts separated dependency-key lookup from guards, hid large whole-world
predicates behind opaque contracts, lifted the exact dependency update into a
separate lemma, and explicitly instantiated the strict changed summand. These
changes proved supporting facts without discharging the two compositions.

At per-function rlimit 90, the focused dependency run took 168.15 seconds and
477 MiB peak; the fact run took 176.21 seconds and 563 MiB peak. Both exhausted
the solver limit. Their PrismaBuild actions are
`b412463e130c8568a4a54e22520b3ad7e488805ab99392115b31b9f76ad2471b` and
`40608fdc2713e1e923908aa2829213ad7d7b4a5458385ee635311ea3d035b721`.
These are failure measurements, not verification evidence.

## Suggested next step

The reviewer recommends exposing a changed-edge witness and an explicit rank
delta at the real dependency update. Prove that local delta first. Then lift it
through a frame lemma that states which nodes and edges stay equal. Compose the
result with `on_fact`, then `apply`, and finally the real settle result. This
makes the strict summand and the unchanged remainder explicit at each boundary.

Acceptance requires the full outside-witness theorem over arbitrary finite
worlds within the enforced scope, source-connected contracts for every witness,
nonzero required Rust-check inventory, and seeded mutants that fail the intended
obligations. Do not replace a stuck composition with a trusted specification,
`external_body`, `assume`, or `admit`.
