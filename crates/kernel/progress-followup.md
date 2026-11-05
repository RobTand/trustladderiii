# #805: global progress over the executable kernel

For every consistent, settled world with an open request, the real
`World::witness` selects an outside event that strictly decreases a natural
rank after the real `step` and its settle loop. The premises retain #771's
counter headroom, normalized file storage, and fresh command IDs. Q30 and Q39
permit starvation of an individual request. The theorem promises one global
progress event, with no fairness premise.

`src/progress_theorem.rs` contains `kernel_progress` and a concrete in-scope
world with a Pending prerequisite. `src/progress_specs.rs` specifies exact
selector branches. `src/witness.rs`, `src/steps.rs`, `src/settling.rs`, and
`src/scheduler.rs` contain the actual executable bodies, split into required
proof groups without changing their transitions.

## Delivery delta and frame

The rank is `node_total` plus one token for each undelivered prerequisite of
an open node. All sums use mathematical naturals over arbitrary finite storage.
The executable witness does not evaluate this sum.

1. `prepare_dependency` proves the actual lookup selects an existing Pending row.
2. `record_dependency` proves the exact replacement of that row.
3. `delivery_changed` exposes its index and the concrete table update.
4. `delivery_node_frame` preserves node distances and every owner's phase.
5. `edge_update_delta` and `edge_prefix_frame` lift the single-row delta.
6. `dependency_fact` and `on_fact` expose an exact whole-world decrement of one.
7. `apply` composes all successful handler decreases. `settle_measure` proves
   that settlement preserves rows, never reopens an owner, and cannot increase
   the rank. `step` therefore retains the strict decrease.

The checker projection includes prerequisite-key uniqueness, positive numbers,
matching repositories, and existing PR-state consistency checks. The production
checker proves each fact. These facts add no new rule or runtime validation.
Local reply contracts name the variants sent by the real dispatcher. Every
caller proves that domain, so the global property loses no case or bound.

## Required verification and remaining ports

All 28 groups are required Rust targets in `tools/bazel/verus.bzl`, with 289
positive exact obligations and the existing 45-second watchdog. Qualification
runs disable remote action and test-result caching. Six progress mutants alter
real executable source: retained prerequisite, skipped dependency dispatch,
skipped selector scan, stalled Resume, stalled maker reply, and retained effect.
Compiler failures and timeouts do not count as mutant rejection.

Kani stays frozen and unchanged. #806 must port each progress harness's complete
assertions before deleting it. Global progress supplies the composition needed
by those ports, but does not itself prove an individual leaf's Q48 grant.

## Historical #771 attempt

## Preserved draft

Commit `2668baf75a48661d49c7733c565c057c99d0988e` preserves the source-connected
progress attempt. It is an ancestor of `t771-verus-kernel-progress`; the local
reference `t771-progress-draft-20261104` also names it. Its unfinished progress
contracts are excluded from the delivered safety inventory. The proof follows
the real `World::dependency_fact`, `on_fact`, `apply`, and settle bodies, rather
than the spike's projected transition model.

## Earlier stuck obligations and measured cost

The earlier composition obligations occurred in `World::dependency_fact` and
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

## Reviewer recommendation implemented in #805

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
