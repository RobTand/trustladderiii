# Flux deployment owner limits

`flux-deploy.py` binds the first deployment owner brief at
`/mnt/shared/flux-migration/briefs/TL3-FLUX-FIRST-DEPLOY-AUTHORITY.md`, SHA-256
`afe66f03ac0e6bd772a7032c4f30103a8092c98d7af05a1ed9d373f9dd9b1442`.

The shared lock directory is `/mnt/shared/flux-vanilla/deploy`, owned by `rob`
with mode `0755`. Its only permitted entry is the regular, private, single-link
`deploy.lock`. Existing invalid directories, foreign entries, symlinks, and
unowned files cause refusal. The coordinator changes permissions only when it
creates the directory. It never writes temporary files or dispatch data there.

All dispatches and their intents, bindings, closure records, observations, and
receipts live under `/home/rob/.local/share/tl3/deploy-admission`, mode `0700`.
This fixed host-local directory belongs to TL3's state root. It is outside
Postgres data, job scratch, and all six fresh-init reset journals under the
worker runs directory's `.controller`. Evidence survives fresh-init and
coordinator exit. Neither the source checkout path nor an invocation option
selects another evidence root or another lock.

The global flock covers the complete sibling scan, both stages, and the
`accepted.json` write. Only the dispatch being reattached is exempt from the
scan. Build reuse grants no exemption for unresolved fresh-init or other jobs.
The existing `tl3.deploy.closed.v1` format and all intent, generation, jobspec,
job ID, and observed-file hash bindings remain unchanged. Closure requires
terminal, cgroup, and scratch proof and does not grant PASS.

Every new dry-run jobspec and every persisted jobspec used for reattach or build
reuse must select exactly `constraints.hostlist = ["dl380g10"]`, one slot,
at most 8 cores, at most 32 GiB of memory leaves, and an explicit positive
duration of at most 5400 seconds. Node requests, exclusive resources,
ambiguous constraints, missing values, and malformed counts cause refusal
before submission. New intents and dry runs use the single 5400-second
constant. Build stays at 8 cores and 8 GiB; fresh-init stays at 1 core and 2 GiB.
The payload reserves 50 seconds for cleanup before the job deadline. The
collector allows 180 seconds beyond that deadline for native clean observation.

There is no migration or legacy evidence reader. Old UUID directories in the
shared lock directory cause refusal and require an explicit operator recovery
decision. These synthetic fixture checks confer no native acceptance, PB-off,
or deployment authority.

Before either new stage is submitted, `preflight-fresh-budget` checks the exact
manifest and config binding and inventories metadata for Postgres data, config,
the entire worker runs root, and declared tablespaces. The backup estimate keeps
the existing 1800-second overhead and eight I/O passes at 20 MiB/s. Estimates
above 4800 seconds refuse admission, leaving 90 seconds for the existing
stability check and 510 seconds of explicit margin within the job. The full
in-job scope and preflight checks still run before any stop or reset. A submitted
fresh job reattaches without requiring its backup destination to be new again.
These are planning estimates, not measured upper bounds.

All Python commands in the admission payload and its fresh-init call chain use
`-I`, including release provenance in `build-release.sh` and the existing Rust
fresh-database authorization subprocess. The helper's separate `check` command
is outside this admission call chain. No package installation or site writes
are added.

The separately reported Tessera 86400-second probe defaults and possible
three-job startup overlap remain external operations and lead acceptance issues.
This change does not qualify that whole window or alter those paths.
