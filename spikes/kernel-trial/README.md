# Kernel trial: Rust and Kani

A throwaway trial from 2026-10-29. It asks two questions: can TL3's core state machine live in a small, pure Rust kernel whose laws Kani proves, and does defining the machine functionally make it more resilient?

The machine is a toy: one item moves through implement, test, review and merge. The real state machine is still to be designed.

## What is here

- `rust/src/lib.rs` holds the kernel. `step(item, event)` returns the next item and at most one effect. `pending(item)` returns what the item needs in flight. Ordinary builds have no standard library, so the kernel cannot reach files, sockets, clocks or processes. The laws sit in the same file as Kani proofs.
- `rust/src/main.rs` stands in for the shell. It replays a noisy run, simulates a crash after every event, and times the kernel.
- `rust/run.py` runs the demo, then `cargo kani`, as one PrismaBuild action.

## Laws

1. An item merges or closes only on a commit that passed tests and that a reviewer approved, whatever events arrive, in any order, duplicated or stale. Kani proves the induction step for every item and event, and separately checks every possible run of five events.
2. While tests run, only PrismaBuild's own result for this attempt and commit moves the item. Silence, a lost lease and stale results never do, and the awaited result always does.
3. An event that moved an item is used up. Delivering it again changes nothing and starts nothing.
4. A move emits exactly what the new state needs in flight. Recovery replays the log and asserts `pending` again, so it re-issues the same effects under the same keys.

## Result

PrismaBuild action `01409f83b7c6a810fd1433a9ff684e16508993f30ca825eae32189fe1a72b0e3` ran on dl380g10 and took 7 s. Receipt `e45c081dc23895586e83f2e2fd5615a010fd0a4b4b650f7440381d7f9d19670a`, status published.

- Kani 0.68.0 with CBMC 6.11.0 verified all 5 harnesses with 0 failures, in 0.6 s to 3.5 s each and 6.0 s in total.
- The noisy run closed on commit 8 after a failed first attempt. Two quiet periods, a lost lease, two duplicates and a stale result changed nothing they shouldn't.
- Replaying the log after a crash at each of the 15 points matched the live run every time.
- The release build steps the kernel 128 million times a second.

Rust forced one decision that Dafny's unbounded integers had hidden: what happens when the attempt counter runs out. The kernel now moves the item to `Held`. Without that, Kani's overflow check would have failed the proof.

## How to run it

```bash
cd spikes/kernel-trial/rust
python3 /mnt/shared/prismabuild-fleet/repo/tools/pbrun.py --cwd . --tag x86 --cpus 2 --demand mem_gb=8 \
  --priority -10 --timeout-s 3600 \
  --env PATH=/home/rob/.cargo/bin:/usr/local/bin:/usr/bin:/bin --env RUSTUP_HOME=/home/rob/.rustup \
  --env CARGO_HOME=/home/rob/.cargo --env KANI_HOME=/home/rob/.kani \
  -- /home/rob/venvs/pb-cpu/bin/python run.py
```

`--tag x86` is there because the Rust toolchain and Kani are installed only on dl380g10.
