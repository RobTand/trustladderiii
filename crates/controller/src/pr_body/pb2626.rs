//! PrismaBuild #2626's real issue body, fetched 2026-11-02 (#447 review).
//! [P2] Defer new GPU claims while a gate GPU lease is live on the host
pub const PB2626_ISSUE_BODY: &str = r#"## Problem

Rob's approved design (2026-11-02): on each DGX Spark (sparky, sparklina), gate GPU tests take priority over PrismaBuild GPU work. Those tests are Bazel actions that a BuildBuddy self-hosted executor runs. An outside process has no way to tell a host's PB worker "admit no new GPU work for a while, and know at once if I die." Operator holds (`holds/<host>.json`, `tools/fleet/pbhold.py`) are the wrong tool:
- they stop every claim, not only GPU claims;
- an operator manages them by hand, for up to 12 h;
- they have no liveness, so a dead holder leaves the box locked until expiry.

This issue makes the worker honour host GPU leases for GPU claims. #2627 adds the lease's CPU and memory headroom, #2628 attributes the gate's GPU processes, #2629 records each lease, and #2630 is the client that gate tests call.

## Protocol `prismabuild.gpu_lease.v1` (shared by #2626, #2627, #2628, #2629, #2630)

Gate GPU tests are Bazel test actions that a BuildBuddy self-hosted executor runs on sparky and sparklina, one action per Spark at a time.
- The executor container is named `bb-gpu-executor` and labelled `bb-gpu-executor=1`.
- It uses Docker isolation (`executor.docker_gpus: all`). It creates each action container through the host dockerd with no cgroup parent, so the action's processes live in `/system.slice/docker-<64-hex id>.scope`.
- Action containers are named `buildbuddy-exec-<20 random chars>` and run as root (uid 0, no user namespace).

Before such a test touches the GPU, it takes a per-host lease. While any lease on a host is live (waiting or granted), the PrismaBuild worker on that host admits no new GPU work and keeps the CPU and memory the lease asked for. PB never kills running PB jobs; preemption is out of scope.

### Directory

The directory is `/run/prismabuild-gpu-lease` on the host.
- It lives on tmpfs, so a reboot clears it.
- A tmpfiles.d entry creates it at boot: `d /run/prismabuild-gpu-lease 0777 root root -`. There is no sticky bit, so the PB worker's user can remove files that root in a container left behind.
- It is bind-mounted read-write into each action container at the same path (`executor.docker_volumes: /run/prismabuild-gpu-lease:/run/prismabuild-gpu-lease`). A bind mount shares the inode, so a `flock` taken in a container is visible to the PB worker on the host. This mount and the directory are already live on both Sparks.
- When the directory is absent, the feature is off and PB behaves exactly as it does today.
- The holder and PB share the host clock.

### Names PB reads

PB considers only `<lease_id>.lock` and `<lease_id>.json`, where `lease_id` is 32 lowercase hex characters, plus its own `pb-state.json` and the temp files of its own atomic writes. Every other name in the directory is ignored: PB never reads it as a lease, never counts it, and never removes it. That includes `bb-interim-reservation`, which the GPU lane's interim wrapper owns until the permanent install deletes it.

### Files a holder writes

`lease_id` is 32 lowercase hex characters.

- **`<lease_id>.lock`** is created first. The holder takes `flock(LOCK_EX)` on it and keeps the descriptor open for its whole life. The kernel drops the lock on exit, including `kill -9`.
- **`<lease_id>.json`** is written only after the lock is held, and always atomically (a temp file in the same directory, then `rename`). Fields:
  - `schema`: `"prismabuild.gpu_lease.v1"`.
  - `lease_id`, `host`, and `holder` (a string of at most 200 characters, for example the Bazel `TEST_TARGET`).
  - `mem_gb` and `cpus`: non-negative integers that the gate test needs kept free (client defaults 16 and 4). When absent, they read as 0. Values above the box's declared `mem_gb` and `cpu` capacity are clamped to it.
  - `requested_unix`, `granted_unix` (null while waiting), and `renewed_unix` (rewritten every 5 s).
  - `pb_state_at_grant`: `"ready"`, `"absent"` or `"stale"`; null while waiting.
  - `released_unix` and `release_reason`: null until a clean release.
- **On a clean release** the holder writes `released_unix` and `release_reason: "released"` (or `"wait_exceeded"`), then closes the lock descriptor. It does not delete either file; PB records the lease and removes them.

### File PB writes

PB writes `pb-state.json` atomically. Fields:
- `schema`: `"prismabuild.gpu_lease_state.v1"`.
- `host` and `updated_unix`.
- `fencing`: the sorted lease ids PB currently treats as live.
- `ready`: the sorted subset of `fencing` that may start now. A lease is ready when PB holds no GPU tokens on this host and, once CPU and memory headroom is implemented, PB's held `mem_gb` and `cpu` fit within the declared capacity minus the summed `mem_gb` and `cpus` of all live leases.
- `pb_gpu_tokens_held`, `pb_mem_gb_held` and `pb_cpus_held`: integers read from this host's resource ledger.

### Liveness (PB side)

The constants are `TTL_S = 30`, `RENEW_S = 5` and `MAX_LIFETIME_S = 3900` (Bazel's `eternal` 3600 s test timeout plus slack). A lease is live only while all three conditions hold:
1. **Its `.lock` is held.** PB probes with `flock(LOCK_SH | LOCK_NB)` on its own descriptor and unlocks at once. If that succeeds, nobody holds the lock and the lease is dead immediately. PB never blocks on the lock and never keeps it.
2. **`now - renewed_unix <= TTL_S`.**
3. **`now - requested_unix <= MAX_LIFETIME_S`.** This is the hard limit. PB stops honouring the lease past it, even while the holder still renews.

A malformed lease is judged this way:
- If the `.json` is missing, unreadable or malformed while its `.lock` is held, the lease is live. Condition 2 then uses the `.json` file's mtime (or the `.lock` file's mtime when there is no `.json`), and condition 3 uses the `.lock` file's mtime. Such a lease counts as `mem_gb = cpus = 0`.
- A malformed lease whose `.lock` is not held is dead.

### Grant (client side)

The holder grants itself in exactly these cases:
- `pb-state.json` is at most 30 s old and lists its `lease_id` in `ready` (`pb_state_at_grant = "ready"`).
- `pb-state.json` is absent (`"absent"`).
- `pb-state.json` is older than 30 s, which means no live PB worker (`"stale"`).


## Behaviour wanted

1. **Module.** Add a new `src/prismabuild/gpu_lease.py` holding the protocol constants (directory, schemas, `TTL_S`, `RENEW_S`, `MAX_LIFETIME_S`) and `live_leases(directory, *, now) -> list[Lease]`, which applies the liveness rules above. A missing directory returns `[]`. No other OSError reading the directory may raise into the claim path: if the directory exists but cannot be listed, GPU claims are denied (fail closed for GPU only, like the #1895 runtime gate's `state_unknown`) and CPU claims proceed.
2. **Claim fence.** While `live_leases` is non-empty, an item whose demand has `gpu > 0` is denied with reason `deferred_for_gate_gpu_lease`. The evidence holds `lease_id`, `holder`, `state` (`waiting` or `granted`) and `requested_unix` for each live lease, at most 4.
   - Apply the fence at both GPU fence points, beside `gpu_runtime_gate.runtime_claim_fence`, in `src/prismabuild/pool.py`: the candidate preparation in `_claim` (about line 18104) and the recheck in `_reserve_claim_host` (about line 18603).
   - Items with no GPU demand are unaffected. Running actions are never touched.
   - The denial is a deferral that ends by itself. It must not withdraw, refuse or reroute the item permanently.
3. **Offer.** While a lease is live, the host's published offer sets `observed_capacity.gpu = 0` and adds a `gate_gpu_lease` detail, exactly as `gpu_runtime_gate.runtime_fenced_offer` does for the #1895 fence. Declared `capacity` is unchanged, so submissions stay queueable and placement prefers the other Spark.
4. **State publisher.** Add a daemon thread in `tools/fleet/worker_loop.py`. Do not publish from the poll loop: it sleeps `--poll-s` (10 s by default) and blocks while an action executes.
   - It publishes `pb-state.json` every 1 s while the directory holds any `*.json` lease, and every 10 s otherwise.
   - It does nothing when the directory is absent.
   - Several loops on one host may each publish; their contents are equivalent.
   - In this issue, `ready` is the leases in `fencing` when `pb_gpu_tokens_held == 0`. #2627 adds the CPU and memory terms. Publish `pb_mem_gb_held` and `pb_cpus_held` now, read from the ledger.
5. **No-overtake invariant.** Once a published `pb-state.json` lists a lease in `fencing`, no GPU claim may commit on that host while that lease stays live. Concretely:
   - the publisher reads the live leases and the held tokens inside the same host admission critical section (`PoolQueue._admission_lock`) in which a claim's lease recheck and its token reservation happen;
   - therefore a lease in `ready` means the GPU is free of PB work and stays free.
6. **Provisioning.** Add `tools/fleet/prismabuild-gpu-lease.conf` (tmpfiles.d: `d /run/prismabuild-gpu-lease 0777 root root -`) and a short install step in `docs/operating_prismabuild.md`. The mode carries no sticky bit, so the PB worker's user can remove files that a container's root left behind.
7. **Docs.** Add a "Gate GPU leases" section to `docs/design.md` next to "Operator holds on new claims". It states the protocol above, the fence, the invariant and what is out of scope.

## Acceptance tests (red on current main, green after)

Put these in a new `tests/test_gpu_lease_fence.py`. A held lock comes from a helper child process that keeps `flock(LOCK_EX)`.
- A live lease (lock held, fresh renewal) denies a GPU item with `deferred_for_gate_gpu_lease`, while a CPU-only item in the same pass is claimed. Red: main claims the GPU item.
- Killing the lock-holding child with SIGKILL lets the next claim pass admit the GPU item, while `renewed_unix` is still fresh (no TTL wait).
- Lock held, but `renewed_unix` older than `TTL_S`: the GPU item is admitted.
- Lock held and renewing, but `requested_unix` older than `MAX_LIFETIME_S`: the GPU item is admitted.
- A malformed `.json` with a held lock denies; a malformed `.json` with no lock admits.
- Directory absent: GPU claims behave exactly as on main.
- A directory holding only `bb-interim-reservation` and other non-lease names fences nothing, and every one of those files is left in place.
- Race: a lease that appears after `_claim`'s candidate preparation but before `_reserve_claim_host` is denied at the recheck. Inject it by making the reader return nothing on the first call and the live lease on the second. Red until the recheck exists.
- Publisher: one live lease plus one held PB GPU token gives `fencing == [lease_id]`, `ready == []` and `pb_gpu_tokens_held >= 1`. After the token is released, a state with `ready == [lease_id]` and `pb_gpu_tokens_held == 0` appears within 2 s.
- Offer: while a lease is live, `observed_capacity.gpu == 0` and declared `capacity` is unchanged.

## Expected footprint

- `src/prismabuild/gpu_lease.py` (new)
- `src/prismabuild/pool.py` (two fence points, offer)
- `tools/fleet/worker_loop.py` (publisher thread)
- `tools/fleet/prismabuild-gpu-lease.conf` (new)
- `docs/design.md`, `docs/operating_prismabuild.md`
- `tests/test_gpu_lease_fence.py` (new)

The new wire schema `prismabuild.gpu_lease_state.v1` and the new denial reason may need entries in the registries that repo-wide guards pin: `src/prismabuild/record_kinds.py`, the record-compat goldens and any denial-reason closed sets. Run those guards as well.

## Out of scope

- Preempting or killing PB jobs.
- vLLM, which is external load and unchanged.
- CPU and memory headroom (#2627), GPU process attribution (#2628), the client (#2630), and lease records and file cleanup (#2629).
- Installing the tmpfiles entry on hosts. The GPU-lane worker does that.

## PR rules

- Branch `prismabuild-<this issue>` (or `prismabuild-<this issue>-<word>`).
- The PR body says `Refs #<this issue>` and has exactly one `Change type: feature` line.
- Give red/green evidence. Run tests through PrismaBuild as `AGENTS.md` requires.
"#;
