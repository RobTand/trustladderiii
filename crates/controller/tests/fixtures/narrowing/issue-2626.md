## Problem

On each Spark, gate GPU tests take priority over PrismaBuild GPU work. An outside process cannot tell a host's PB worker to admit no new GPU work for a while and know at once if it dies. Operator holds stop every claim, are set by hand and have no liveness.

## Protocol `prismabuild.gpu_lease.v1` (shared by #2626, #2627, #2628, #2629, #2630)

Before a gate GPU test touches the GPU, it takes a per-host lease. While any lease on a host is live, the PrismaBuild worker on that host admits no new GPU work.

### Directory

The directory is `/run/prismabuild-gpu-lease` on the host. When it is absent, the feature is off and PB behaves exactly as it does today.

### Names PB reads

PB considers only `<lease_id>.lock` and `<lease_id>.json`, where `lease_id` is 32 lowercase hex characters, plus its own `pb-state.json`. Every other name in the directory is ignored and never removed, including `bb-interim-reservation`.

### Liveness (PB side)

The constants are `TTL_S = 30`, `RENEW_S = 5` and `MAX_LIFETIME_S = 3900`. A lease is live only while all three conditions hold:
1. **Its `.lock` is held.** PB probes with `flock(LOCK_SH | LOCK_NB)` and unlocks at once. If that succeeds, the lease is dead immediately.
2. **`now - renewed_unix <= TTL_S`.**
3. **`now - requested_unix <= MAX_LIFETIME_S`.**

A malformed `.json` with a held `.lock` is live, counted as `mem_gb = cpus = 0`. A malformed lease whose `.lock` is not held is dead.

## Behaviour wanted

1. **Module.** Add a new `src/prismabuild/gpu_lease.py` holding the protocol constants and `live_leases(directory, *, now) -> list[Lease]`, which applies the liveness rules above. A missing directory returns `[]`. If the directory exists but cannot be listed, GPU claims are denied and CPU claims proceed.
2. **Claim fence.** While `live_leases` is non-empty, an item whose demand has `gpu > 0` is denied with reason `deferred_for_gate_gpu_lease`, at both GPU fence points in `src/prismabuild/pool.py`.
3. **Offer.** While a lease is live, the host's published offer sets `observed_capacity.gpu = 0` and adds a `gate_gpu_lease` detail.
4. **State publisher.** Add a daemon thread in `tools/fleet/worker_loop.py` that publishes `pb-state.json`.
5. **No-overtake invariant.** Once a published `pb-state.json` lists a lease in `fencing`, no GPU claim may commit on that host while that lease stays live.
6. **Provisioning.** Add `tools/fleet/prismabuild-gpu-lease.conf` and an install step.
7. **Docs.** Add a "Gate GPU leases" section to `docs/design.md`.

## Acceptance tests (red on current main, green after)

- A live lease denies a GPU item with `deferred_for_gate_gpu_lease`, while a CPU-only item is claimed.
- Killing the lock-holder lets the next claim pass admit the GPU item.
- Publisher: one live lease plus one held PB GPU token gives `fencing == [lease_id]` and `ready == []`.
