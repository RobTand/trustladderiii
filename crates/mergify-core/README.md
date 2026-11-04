# TL3 Mergify core

`tl3-mergify-core` holds the Mergify queue adapter's decisions as pure functions
over small enums (#735). The adapter in `crates/controller` and the client in
`crates/mergify` read GitHub, parse its strings into these types, and act on the
result. This crate does no I/O and has no standard library in ordinary builds.

## What is here

- `check`: a check run as enums: status, conclusion, app, name and head. Check
  names and app IDs are interned per read, so grouping compares integers.
- `observe`: which queue check describes a PR, and the queue state.
- `facts`: which facts a queue read yields. That includes the Q52 missing-check
  rule, the #447 blocked queue, merge evidence and a person's dequeue or close.
- `translate`: how a queue fact reaches the kernel. Setup evidence (Q52, #447,
  unreadable checks) only opens or clears incidents. It never becomes a kernel
  fact, so it never counts a failure or charges credit.
- `http`: GitHub REST status codes to failure classes. `tl3-github` uses it.

The crate has no `unsafe`, no floats and no target-specific code, so its proofs
hold on amd64 and arm64.

## Proofs

`src/proofs.rs` holds one Kani harness per property. Each states its property
over every value of the typed inputs, with lists of at most three checks or
events. Three entries give every shape the grouping rules tell apart: a newer,
an older or an equal ID in the same group, and another group.

Every harness has cover checks, and `tools/kani.py` fails a harness whose
covers are not all satisfied. Every harness also has a seeded mutant in
`tools/kani.py`, which the harness must reject.

Until #726 gives each harness a Bazel target, run the proofs on PrismaBuild from
a clean checkout:

```bash
python3 crates/mergify-core/tools/kani.py manifest > /home/rob/tmp/tl3-mergify-core.json
python3 /mnt/shared/prismabuild-fleet/repo/tools/pbcampaign.py --transport pool --max-inflight 2 /home/rob/tmp/tl3-mergify-core.json
```

Inside an admitted action, `kani.py prove [NAME...]` proves harnesses and
`kani.py mutants [NAME...]` checks that each seeded mutant fails. The parsers
from GitHub's strings into these enums have proptest round-trips in
`tests/parse.rs`.
