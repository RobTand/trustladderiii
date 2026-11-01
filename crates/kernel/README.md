# TL3 kernel

`tl3-kernel` implements slice one of [the state table](../../docs/state-table.md), version 5.
It is an edition-2024 Rust crate in the root Cargo workspace, with `no_std` plus `alloc` in ordinary builds.
It does no I/O. The controller commits its decisions and carries out its outputs.
All production crates share the root workspace and Cargo.lock. The optional `serde` feature supplies controller snapshot encoding and is disabled for Kani.

## Controller API

- `World::new(&[slots_per_repo], pool_capacity)` creates an empty world.
- `step(world, event) -> (World, Vec<Out>)` applies one event and takes every enabled internal transition.
- `settle_world(world) -> (World, Vec<Out>)` settles reconciled gate or agent capacity without inventing an external event.
- `Event` carries new requests, operation replies, effect acknowledgements, facts and commands.
- `Out::Effect` names the node, sequence, effect kind and exact head where needed. `Out::Notice` asks for a message.
- `World::check`, `consistent` and `valid` check the invariant. `World::witness` and `measure` support progress checks.
- `Node::waiting_on()` returns a `Wait` for status readers.
- `Node::operation_pending(kind, generation)` tells the controller whether an acknowledged operation still needs a result.
- `Node::effect_cancelling(seq)` identifies a delivery whose node has moved on.

These queries live in `src/queries.rs`. They read the world without changing
`step`, `settle`, or any transition. Enqueue watches name the generation before
the acknowledgement advances Publishing to Queued.

`HeadId`, `FileId`, `RepoId` and `NodeId` wrap `u64`. The controller interns external commit hashes, paths and repository names.
Keep those mappings stable across restarts. File IDs are scoped to a repo. Head ID zero is valid.
`RepoId` indexes `World::gates`, and `NodeId` indexes `World::nodes`.
Requests append nodes, so those indices stay stable. Keep closed nodes in the snapshot.
Unknown node and repo IDs are ignored without truncation to the machine's index width.

`Node::approved` is one `Option<HeadId>`, the exact head that passed checks and review.
A later approval replaces it. A merge naming any other head raises the unapproved-merge incident and note.

`World::intern_footprint(files)` sorts and deduplicates controller-interned `FileId`s and returns a copyable `Footprint`.
Plans carry that handle, and `MakerNeedsFiles` carries a handle for the additional files.
`World::files(footprint)` returns the files in ascending order. The kernel unions extensions and derives locks from slot holders.
There is no file-count ceiling.

A footprint containing only IDs below 64 is an inline bitset. Larger sets live in the world's append-only `file_sets` table.
Their handles stay stable, and equal sets reuse a handle. Nodes keep only handles, so `Node`, `Reply` and `Event` stay `Copy`.
The controller persists `World::file_sets()` in order and restores the sets with `intern_footprint` before loading nodes and pending events.
Use handles only with their own world. Unknown table indices in replies count as schema failures.

Proof assumptions bound file IDs below 4, so the footprint table is empty in those worlds.
Large sets go through the same transitions, but their set operations take a separate path that the proofs exclude. Tests cover that path instead:

- scenarios with 300 files, high IDs, extensions and file locks across repositories;
- random walks whose file IDs cross the 63/64 boundary, with the lock rule checked again using plain sets;
- `tests/footprint_sets.rs`, which checks every union and lock decision over inline, interned and mixed sets against `BTreeSet`.

Kani asserts that the bounded fixture never enters an interned-set helper, then prunes that path to avoid expanding growable storage.
The assertion must prove before the path can count as excluded.
The fixed `Outbox` remains in place.

Owning a vector in each node proved too costly, even with small file sets held inline.
A minimized probe checked the arbitrary starting world in 2.9 GiB, but cloning it exhausted 20 GiB before taking a transition.
Interning large sets preserves `Node: Copy` and avoids that clone path. The PR records the before and after action keys and memory peaks.

The controller-facing interner normalizes incoming file lists. Transitions union already-sorted sets with a linear merge.
This keeps Rust's recursive sort out of the transition proofs. A diagnostic found Kani expanding that sort even in a reply proof with only inline footprints.
The lock invariant compares each pair of holders once, separately from counting holders per repo.
Putting that comparison inside the repo loop made a reply proof exhaust 48 GiB.
The separate comparison verified at 21.8 GiB, with the same bounds, assertions and cover.

`Notice::Note(NoteKind::Replanned)` marks the one automatic re-plan after attempt three fails.
It is emitted when the node goes back to Planning. If credit is exhausted, the note waits until Rob funds that continuation.
The message adapter supplies the plain-language text.

## What is here

- `src/lib.rs` is the kernel: `step(world, event) -> (world, outputs)` plus `settle`, with no standard library in ordinary builds. It also holds:
  - `consistent` and `check`, the invariant;
  - `measure` and `settle_rank`, the ranking functions;
  - `witness`, the progress event;
  - `proof_of_event` and `proofs_of_phase`, the proof map.
- `src/proofs.rs` holds the Kani proofs.
- `tests/` holds scripted scenarios, seeded random walks, the footprint set checks and the proof-map check.
- `run.py` runs a build and tests, runs named Kani proofs, or prints a PrismaBuild campaign manifest.

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

Each transition writes its outputs into an `Outbox`, a fixed array of at most `OUTBOX` (5) outputs, and `step` collects them into its list. An overflow is an assert, so the proofs also show that no transition emits more than that.

Three guards keep the proofs honest:

- `proof_of_event` and `proofs_of_phase` are exhaustive matches, so an event variant or phase without a proof doesn't compile.
- `tests/proof_map.rs` checks that every mapped proof exists.
- Every proof has a Kani cover check, and `run.py` fails any proof whose covers aren't all satisfied, so no proof passes vacuously.

## Bounds and simplifications

The bounds are:

- two requests, or three after a new one arrives;
- two repos of four files each;
- W at most 3 and the agent pool at most 2 in the general proofs;
- four requests in sixteen focused gate proofs at W = 2 and W = 3, covering grants, file locks, tickets, planning admission and progress;
- counters below 2^40.

These are simplifications against the state table, all deliberate:

- Each request is one root leaf, so its credit lives on its node.
- Rob owns every pause, as in slice one.
- Notices are outbox entries no node waits on. They cover questions to Rob, notes, issue summaries and filing a replacement request.
- Looks run in the controller. What they find arrives as replies, acknowledgements and facts.
- After a permanent effect failure, carrying on restarts the phase's effects from the top. Every effect converges or is deduplicated, so this ends where retrying only the failed effect would.
- Production IDs are 64-bit newtypes. Proof assumptions bound heads below 8 and each repo to four file IDs.
- Events name requests by stable `NodeId` indices.

## Findings from the spike

- **Proving a whole step at once is too big for Kani.** Every harness that did so hit PrismaBuild's one-hour limit, even for the simplest event. Proving one transition at a time takes about 2 to 12 minutes per proof and at most 25 GB.
- **Kani found states the first invariants allowed but the kernel never reaches.** Examples include a closing request still holding an open PR, a merge recorded outside a merged outcome, and a question carrying a continuation the kernel never gives it. Each became a fact in `check`. The tests check every intermediate state of 200,000 random steps, and none of the counterexamples was a reachable kernel bug.
- **The cover checks caught a harness bug.** One harness had two cover statements and reached only one of them.
- **A growable list on the heap is expensive for Kani.** Every merge proof needed more than 48 GB, even split by phase. Two bisects on the fleet found that the merge itself and the full invariant fit in 5 GB, and that the cost came from reading the merge's outputs back from their `Vec`. The likely cause is that a merge adds notices after a branch, so the `Vec`'s length is unknown at each push and CBMC also explores growing it. With the fixed-size outbox, the merge proofs need 5.2 GB.

## Historical spike results

All 81 proofs verified on commit `7a9af9b`, with every cover check satisfied, and the tests pass: the 11 scenarios, the random walks and the proof-map check. The run took 285 CPU-minutes across dl380g10, sparky and sparklina, and 16 minutes from the first start to the last finish.

| Proofs | Count | Peak memory | Minutes each, median and max |
|---|---|---|---|
| `apply_*` | 39 | Median 4.7 GB. `apply_new_request` needs 22.3 GB. | 3.4, 6.1 |
| `advance_*` | 14 | At most 5.9 GB | 1.9, 3.4 |
| `progress_from_*` | 14 | Median 13.4 GB. `progress_from_queued` needs 24.5 GB. | 9.8, 11.9 |
| `pr_merged_*` | 14 | At most 5.2 GB | 1.9, 3.3 |

Earlier runs:

- The first full run verified 54 of 69 smaller proofs. 13 ran out of memory, 1 found an invariant gap and 1 was the cover-check harness bug.
- The merge proofs then kept running out of memory, at 24 GB and at 48 GB, until the outbox.

## Run development verification

Submit from a clean commit. PrismaBuild snapshots the entire checkout for each action.
Do not edit files while a campaign submits. Keep the manifest and receipts outside the checkout.

From the repo root, generate and submit all 103 proofs and the tests:

```bash
python3 crates/kernel/run.py manifest > /tmp/tl3-kernel-campaign.json
python3 /mnt/shared/prismabuild-fleet/repo/tools/pbcampaign.py --detach /tmp/tl3-kernel-campaign.json
python3 /mnt/shared/prismabuild-fleet/repo/tools/pbwait.py --endings KEY...
```

The manifest gives each proof its own action and lets PrismaBuild choose any eligible worker.
It reserves one CPU per action, bounds Cargo and native thread counts, and sets priority -10.
Advance, merge and non-reply event proofs reserve 10 GiB. Reply proofs reserve 44 GiB, and general progress proofs reserve 24 GiB.
All four-node gate proofs reserve 10 GiB, after measured peaks of 2.7 to 5.3 GiB. Waiting-only worlds prove separately that some internal transition is enabled; mixed worlds prove that every other world has a maker, merge or resume witness. Each concrete event application proves strict progress. This preserves the original worlds and assertions without expanding `is_settled` across all four phases in one solver invocation.
`progress_from_queued`, `apply_new_request`, `apply_new_adoption` and `apply_reviewer_out_of_room` reserve 44 GiB.
The coordinator set the 44 GiB budget on 2026-11-01 after six proofs from the former 64 GiB class peaked at 27.9–35.3 GiB. The two rebase reply proofs reserve 48 GiB after a measured peak of 38.7 GiB. Other 32 GiB classes keep 32 GiB. Critical-path development tests use priority -9 when named by the coordinator; bulk proofs remain at -10.
If an ending shows that a proof needs more, raise that row's `demand.mem_gb` before retrying.
Compare each ending's cgroup memory peak with the historical spike table above.
Rust 1.99.1 and Kani 0.68.0 live at the same paths on dl380g10, sparky and sparklina.

To generate a smaller campaign, name `tests` or the proof harnesses:

```bash
python3 crates/kernel/run.py manifest tests apply_conflict > /tmp/tl3-kernel-campaign.json
```

Inside an admitted action, `run.py tests` checks formatting and Clippy, builds the kernel in ordinary `no_std` mode and runs the scenarios, random walks and proof-map check.
`run.py prove NAME...` runs the named harnesses. Omitting the names runs every proof sequentially, so use the campaign for a full fleet run.
The script fails if Kani fails or any cover is missing or unreachable. `KANI_OUTPUT=regular` shows CBMC's detailed progress.

A submission is not a pass. Check the terminal exit code, actual logs and verified CAS receipt before reporting a result.
CI runs separately on Mergify's fleet runners. It does not submit to PrismaBuild.

The proposed Q48 layer adds two per-leaf progress proofs at W=2 and W=3.
They cover four leaves: an unpaused oldest waiter, its last conflicting holder,
and two younger leaves. The holder completes through the real stop and
teardown paths. Visiting younger leaves first cannot take the older waiter's
newly available slot, even when a disjoint overtaker remains active.
This is a bounded progress proof with successful holder completion, not a
proof of unbounded temporal liveness. Q48 needs Rob's approval before queueing.
