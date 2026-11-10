# Shared seeded scenario scope

Worker B owns mod.rs, kernel_scope.rs, kernel_original.txt, sampling.rs, api.rs,
assertion_hashes.json and sync.py. Worker A adds common/a_extra.rs only.

An integration test starts with:

```rust
extern crate alloc;
#[path = "kani_scenarios/common/mod.rs"]
mod common;
use common::*;
```

The `use common::*` is required: the real footprint/query modules refer to
crate-root World, FileId, EffectKind, Node and Phase. The scope compiles the
production modules directly from src. kernel_scope.rs copies lib.rs with only
module paths adjusted, crate-only documentation/attributes removed and unrelated
unit-test modules omitted. AdvanceContext is public within this test scope so existing nested equivalence tests can resolve their crate-root import. assert_source_identity() fails if lib.rs drifts;
sync.py refreshes that scope and the verbatim functions from proofs.rs.
Production source is not edited or made public.

The test-local `sampling as kani` binding preserves the copied function text,
including all assertion expressions and all assume filters. It is not the Kani
crate. Its any() is deterministic, assume() counts rejected candidate samples,
and cover! records witnesses. There is no nondeterministic input or dependency.

For A, `Stats::case(index, seed, Some(world), draws, || {...})` establishes a
thread-local deterministic context and catches only the private filter payload.
Assertion panics propagate and fail the test, with seed/case/world diagnostics.
Draw values are consumed in order. A mismatched draw type or unused accepted
case draw fails. Stats::finish(name, seed, start) rejects a vacuous all-skipped
run and prints attempts, passes, skips, elapsed nanoseconds and cover labels.

Public API: phases(), priorities(), statuses(), node_fixture(), world_fixture(),
Prng::next(), enumerate(radices, visitor), assert_apply(event),
assert_advance(phase, moves), assert_nodes(), assert_outputs(),
assert_fail_fast(), assert_consistent(), is_consistent(), is_settled(),
node_rank(), apply_event() and advance_node_for_test().

A may construct payloads outside checks, then run assert_apply/advance using an
explicit world. Those assertion functions retain their original text and filters.
B's branch is fv-k5-kani-tests-b-1110, from main4cd55642. The initial supplier is
source-only until the BB compilation/run recorded by B. Do not edit B's files.
