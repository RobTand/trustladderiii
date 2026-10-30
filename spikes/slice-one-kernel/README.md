# Slice-one kernel spike

This spike builds TL3's slice-one kernel in Rust from `docs/state-table.md` version 4. It answers one question before anyone builds the controller: can the state table's rules be proven as written, at the stated bounds, in reasonable time?

## What is here

- `src/lib.rs` is the kernel: `step(world, event) -> (world, outputs)` plus `settle`, with no standard library in ordinary builds. It also holds:
  - `consistent` and `check`, the invariant;
  - `measure` and `settle_rank`, the ranking functions;
  - `witness`, the progress event;
  - `proof_of_event` and `proofs_of_phase`, the proof map.
- `src/proofs.rs` holds the Kani proofs.
- `tests/` holds scripted scenarios, seeded random walks and the proof-map check.
- `run.py` runs one PrismaBuild action: `run.py tests`, `run.py prove NAME...`, or `run.py prove` for all.

## How the proofs work

A step is one event applied by `apply`, followed by internal transitions taken by `advance` until settle finds none. Each proof covers one transition kind, starting from an arbitrary consistent world within the bounds. Because of that, the rules hold for every reachable world by induction, with no histories replayed:

- Applying any event, and any internal transition, keeps `consistent(world)` and the per-transition rules:
  - effect keys are fresh;
  - a paused request starts nothing;
  - TL3 enqueues only an approved head;
  - Closed is final;
  - merge credit applies once;
  - each of Rob's answers applies once.
- Settle ends for any number of requests. Every internal transition lowers its request's `settle_rank` and leaves the other requests alone.
- There are no dead states. From every settled world with an open request, the witness event lowers the progress measure, and no internal transition raises it again.

The proofs are as small as they usefully get, so they run fast and can run on every change:

- one proof per event variant;
- one per phase for settle's internal transitions;
- one per phase for progress;
- one per phase for merges, which are the heaviest event to prove.

Each proof acts on request 0. Requests are interchangeable, so request 0 stands for any of them. A concrete event or phase lets Kani skip the paths that case can't reach.

Three guards keep the proofs honest:

- `proof_of_event` and `proofs_of_phase` are exhaustive matches, so an event variant or phase without a proof doesn't compile.
- `tests/proof_map.rs` checks that every mapped proof exists.
- Every proof has a Kani cover check, and `run.py` fails any proof whose covers aren't all satisfied, so no proof passes vacuously.

## Bounds and simplifications

The bounds are:

- two requests, or three after a new one arrives;
- two repos of four files each;
- W and the agent pool at most 2;
- counters below 2^40.

These are simplifications against the state table, all deliberate:

- Each request is one root leaf, so its credit lives on its node.
- Rob owns every pause, as in slice one.
- Notices are outbox entries no node waits on. They cover questions to Rob, notes, issue summaries and filing a replacement request.
- Looks run in the controller. What they find arrives as replies, acknowledgements and facts.
- After a permanent effect failure, carrying on restarts the phase's effects from the top. Every effect converges or is deduplicated, so this ends where retrying only the failed effect would.
- Heads are abstract ids below 8, and footprints are bitmasks.
- Events name requests by index.

## What the spike found

- **Proving a whole step at once is too big for Kani.** Every harness that did so hit PrismaBuild's one-hour limit, even for the simplest event. Proving one transition at a time takes 2 to 10 minutes per proof and under 25 GB.
- **Kani found states the first invariants allowed but the kernel never reaches.** Examples include a closing request still holding an open PR, a merge recorded outside a merged outcome, and a question carrying a continuation the kernel never gives it. Each became a fact in `check`. The tests check every intermediate state of 200,000 random steps, and none of the counterexamples was a reachable kernel bug.
- **The cover checks caught a harness bug.** One harness had two cover statements and reached only one of them.

## Results

The first full run of the small proofs verified 54 of 69 with every cover satisfied, using about 200 CPU-minutes across dl380g10, sparky and sparklina. Of the 15 failures:

- 13 ran out of their memory reservation; they need about 20 GB, or splitting;
- 1 found an invariant gap, now fixed;
- 1 was the cover-check harness bug, now fixed.

A full rerun is in progress.

## How to run it

```bash
cd spikes/slice-one-kernel
ENV="--env PATH=/home/rob/.cargo/bin:/usr/local/bin:/usr/bin:/bin --env RUSTUP_HOME=/home/rob/.rustup \
  --env CARGO_HOME=/home/rob/.cargo --env KANI_HOME=/home/rob/.kani"
# The tests, on any fleet box.
python3 /mnt/shared/prismabuild-fleet/repo/tools/pbrun.py --cwd . --anywhere --cpus 2 --demand mem_gb=8 \
  --priority -10 --timeout-s 1800 $ENV -- /home/rob/venvs/pb-cpu/bin/python run.py tests
# One proof. A campaign submits one row per proof.
python3 /mnt/shared/prismabuild-fleet/repo/tools/pbrun.py --cwd . --anywhere --cpus 1 --demand mem_gb=10 \
  --priority -10 --timeout-s 3600 $ENV -- /home/rob/venvs/pb-cpu/bin/python run.py prove apply_conflict
```

Rust 1.99.1 and Kani 0.68.0 are installed at the same paths on dl380g10, sparky and sparklina, so `--anywhere` is accurate.
