# Deploy with fresh TL3 state

The canonical caller is `deploy-main.sh` in this directory. Install the standalone
`external-deploy-main.sh` as `/home/rob/tmp/tl3-build/deploy-main.sh` only after
review and the captain's tooling fast-forward. It requires exactly one full
qualified release SHA, with no branch or default input. It clones the fixed
upstream with complete history into a private staging directory beneath
`/home/rob/.local/share/tl3-deploy-sources`, checks detached HEAD and complete
porcelain including ignored files, then publishes `<SHA>` atomically. Existing
dirty, mismatched, shallow or symlinked trees refuse without repair or deletion.
Clean exact clones may be reused. Git and qualification subprocesses have
deadlines, and inherited Git overrides cannot redirect repository checks.

The external caller executes the canonical script from that exact clone. The
canonical script verifies its own source boundary and invokes the existing
authenticated qualification gate before any release build or service operation.
Clone identity alone never qualifies a release. No working file or HEAD from
the shared project checkout participates. Qualification fixtures exercise
control flow only and do not prove final bundle eligibility.
Deployment still needs the lead's outage authorization and a selected, qualified
integrated FX+S2 head. Building this tooling does not authorize a deployment.

Prepare the selected binaries, immutable runtime, existing config, exact state
inventory and external-work cleanup before the outage. The caller builds release
artifacts and admits fresh-init through the pinned native Flux caller. It never changes model ladders,
credentials, Flux bindings, images or runtime bundles. It does not install or
upgrade Python environments or the independently deployed console.

`TL3_DEPLOY_LANE=release` requires the existing authoritative gate's authenticated
full CI and kernel inventory at the selected head. `dev` requires authenticated
captain evidence at that exact head, including Verus and kernel tests. Both require
one authenticated nightly whose artifact proves the exact deployment head.
Missing, unknown or mismatched evidence refuses. The helper uses
`/home/rob/tmp/tl3-build/dev_evidence.py`; it does not accept local verdict receipts
as qualification. Qualification and its network children run under a deadline before any stop.
The captain evidence directory is fixed; no caller override is accepted.
The first caller qualification is read-only Git/evidence inspection on the coordinator,
not a test or build. The same qualification runs inside the admitted exact-source
clone before any outage, so missing history or evidence refuses before stop.
Runtime manifests must match their content addresses and private inventory
hashes; missing, changed, writable or unlisted runtime modules refuse before the outage.
The nightly lookup conservatively refuses if the authoritative helper selects a
different commit with matching proof inputs. It never dispatches a nightly.

## Prepare the private manifest

Supply a mode 0600 JSON file with these fields. Use the actual selected config
and inventory; no new TOML settings are added.

| Field | Required value |
| --- | --- |
| `schema` | `tl3.fresh-init.v1` |
| `head` | Full selected deployment commit |
| `config_sha256` | SHA-256 of the existing `~/.config/tl3/tl3.toml` |
| `flux` | Exact parsed existing `[flux]` object, kept private |
| `runtime_manifests` | Map of each configured immutable manifest locator to its SHA-256 |
| `attempts` | Complete list of exact encoded-delivery/`attempt-NNNNNNNN` directories relative to `workers.runs` |
| `archive` | `controller` mapped to `workers.runs/.controller`, plus `attempt-0`, `attempt-1`, etc. mapped in `attempts` order to their exact directories |
| `reset` | Every existing supported adapter directory, spelled `controller/direct-merge`, `controller/workers`, `controller/writer`, `controller/github`, `controller/mergify`, or `controller/prismabuild` |
| `tablespaces` | Map of each external Postgres tablespace OID to its exact existing private directory; `{}` when none exist |

The helper derives the database and file boundaries from existing configuration.
It supports only the existing deployment unit's local socket, port 5543, and
explicitly configured database/user `tl3`. A different identity refuses; it is
never silently replaced by defaults. It requires the selected Flux backend and
the existing production readiness marker.

World state is inside the TL3 database and is backed up with the complete cold
cluster. The controller journals and inventoried attempts are copied separately.
There is no standalone world-root setting. Config and credentials receive a
private diagnostic copy and stay outside reset scope.

Nonempty `flux-checks` or `worker-waits`, or an attempt containing `execution.json`
or `claim.json`,
refuses the reset. This helper has no generic cleanup-proof importer. Those
bindings need separate, exact reconciliation before this lane can proceed.
Attempt directories are archived and retained. Unknown controller state refuses
until the final integrated source contract is reconciled.

Repository Git directories, common-directory journals, refs, trim/staging receipts,
immutable runtime/staging inputs, all Flux outputs and `.run-backend` ownership
journals, native scratch, instance generations and image stores are excluded.
Neither a manifest nor a cold copy authorizes erasing outstanding execution debt.

## Execute the authorized window

Set `TL3_FRESH_STATE_MANIFEST` to the reviewed manifest and `TL3_COLD_BACKUP` to
a new absolute private backup directory. Invoke the canonical caller with the
full selected commit. `TL3_DEPLOY_DRY_RUN=1` runs qualification only.

The helper performs this sequence:

1. Authenticate qualification, validate the exact prepared release and unchanged
   config, and verify the state inventory and runtime manifest identities.
2. Validate the absent backup destination, private parent and all non-DB source
   hygiene before stopping. Refuse before any stop while `tl3-workers.slice` is
   anything other than `inactive`, or a recorded supervisor or worker is alive.
   Require the configured `workers.launcher` to equal the deployment's `LAUNCHER`,
   so newly started workers belong to that slice. Size the exact file inventory against the bounded
   action budget. Stop tl3d, prove `inactive`, explicitly stop and prove
   MLflow/status/console inactive, then stop Postgres and prove it inactive.
3. Recheck worker quiescence. Copy the complete cluster including WAL and declared tablespaces, controller
   journals, attempts and private config. Hash every file and record ownership,
   modes, directories and scope. Verify the copied inventory against the original pre-copy hashes,
   then flush the copy before any reset. Undeclared symlinks and special files refuse.
4. Recheck worker quiescence before file reset.
   Empty only the manifest's supported adapter directories. Keep the cold copy
   as diagnostic evidence. Install each selected release binary from one hash-checked buffer, then verify
   the installed checksums before executing any selected binary.
5. Start Postgres and check readiness. The SQL helper revalidates the cold copy and unchanged config without network
   calls. It rechecks inactive clients, the worker slice and recorded identities
   before consuming authorization. A one-use, expiring capability crosses only private orchestrator pipes;
   the saved context contains only its digest. Archived context alone cannot
   authorize a reset. Atomic consumption precedes SQL mutation, and Postgres
   system identity must match the cold copy. It refuses live TL3 database
   sessions, drops only the constant identifier `tl3`, recreates it, initializes
   this revision's complete schema on the empty database, and verifies the reader.
6. Start selected tl3d, verify its schema and webhook HTTP response, then explicitly
   start MLflow, status and console. Require active units and 90 seconds of stable
   readiness of all four client units with zero restarts.

The pinned Zonky 17.4.0 package contains exactly `initdb`, `pg_ctl`, and `postgres`
in `bin`. `deploy.py` declares this set as `PG_EXECUTABLES` and checks every member
before publishing an extracted installation and when reusing an installation.
The deployment never requires another PostgreSQL utility.

The hash-verified selected `tl3-deploy cold-cluster-identity DATA` reads the new
cold copy's `PG_VERSION` and `global/pg_control` before publishing reset context.
It accepts only PG17's little-endian Linux amd64 layout and complete 8192-byte
files with valid CRC-32C. The system identifier remains a decimal string.
Authorization rechecks the cold identifier against the context before consuming
the capability. The SQL boundary still compares the running cluster's identifier
before any database reset.

`tl3-deploy database-ready CONFIG` opens an authenticated connection to database
`postgres` as `tl3` over the configured Unix socket on port 5543. It issues no SQL.
The orchestrator allows three attempts, each in a process bounded at four seconds,
with half-second retry delays. A failed or stalled connection refuses before SQL
authorization. Credentials remain inside the helper.

An attempt after a partial file reset still requires a new backup destination.
Its new copy includes the current empty adapter directories and cold cluster.
Existing backups are never inputs to this retry and are never searched or changed.

The parser follows PostgreSQL `REL_17_4`'s
[`ControlFileData`](https://github.com/postgres/postgres/blob/REL_17_4/src/include/catalog/pg_control.h)
and [`get_controlfile_by_exact_path`](https://github.com/postgres/postgres/blob/REL_17_4/src/common/controldata_utils.c).
On this ABI, `sizeof(CheckPoint)` is 88, `sizeof(ControlFileData)` is 296,
and `offsetof(ControlFileData, crc)` is 288. CRC-32C covers bytes 0 through 287,
including struct padding, and excludes the stored CRC and trailing file padding.
[`pg_crc32c.h`](https://github.com/postgres/postgres/blob/REL_17_4/src/include/port/pg_crc32c.h)
defines the Castagnoli checksum convention.

The admitted fixture scripts in `fixtures/pgutil-*.fixture` retain the immutable
baseline reproduction, upstream C layout probe, and synthetic retry qualification.
They use `/mnt/shared/agent-artifacts/fresh-init-pgutil` for their own artifacts.
The Rust parser tests embed bytes from that pinned package's actual `initdb` file,
whose full SHA-256 is
`219d725db03ff4d5b63dc88d2e3064331cc2489d145ca299bb6749ff4d507c48`.
These fixtures exercise synthetic boundaries and do not qualify a live deployment.

The native Flux actions have a 5400-second walltime, with the payload child
bounded at 5350 seconds. Preflight estimates inventory transfer cost conservatively
at 20 MiB/s with eight I/O passes plus 1800 seconds overhead; it refuses a predicted
backup estimate above 4800 seconds before either new stage submits and again
before any stop. This leaves 90 seconds for stability and 510 seconds of margin
inside the job. This is a planning estimate, not a measured
live cold-copy timing claim. The data root is read for its initial hash, copy
verification and SQL-boundary copy verification, without rehashing it at file reset.
Actual fleet/cold-copy timing still requires deployment acceptance.

Worker checks inspect only `claim.json` and `process.json` in TL3 attempt
folders and the worker slice's systemd state. They read the exact recorded PID's
start time, machine identity and boot identity. An exited process, an earlier
boot or a reused PID is stale. A different machine, malformed record, symlinked
record, unreadable identity or unknown unit state refuses. Cancellation records
must contain exactly `{"cancelled": true}`. These checks do not use process
listings or scan `/proc`. The existing Flux binding refusal remains mandatory,
even when an identity record is stale.

If any start or later startup check fails, cleanup first stops tl3d and proves it
inactive to prevent new admission. It then stops `tl3-workers.slice`, stops the
dependents in reverse order, and verifies inactive clients and workers. Only
then does it stop and verify PostgreSQL. Each stop has a 150-second subprocess
deadline, and each systemd state query has a 15-second deadline. This includes
partial starts that activated a unit before returning an error. The original
exception is raised with a cleanup note. A cleanup error reports
`frozen state NOT established`, retains all evidence and leaves PostgreSQL up
when clients or workers might still need it. No previous release or state is
restored.

A verified frozen state is an observation under systemd and the recorded TL3
identities. It assumes no independent operator or external launcher starts work
after verification. It cannot cover unrecorded work outside `tl3-workers.slice`,
host loss, forced process termination during cleanup, or inaccessible ownership.
A live recorded process outside the slice prevents a frozen verdict; this helper
does not kill an unrelated scope or process to make the check pass. Recovery
requires the deployment operator. Fixtures prove control flow, not live service
or cgroup behavior.

All subprocesses have deadlines. A failure leaves the diagnostic backup intact
and does not restart an earlier binary or import old state. Stop dispatch and
diagnose before choosing another qualified build. A refusal or interruption after
stopping services leaves them down, possibly with emptied file journals. The
one-use capability cannot be replayed. Re-run the complete fresh-init flow with
a NEW backup destination and current qualified inputs; retain the original
backup and partial-attempt evidence. Never resume an isolated DROP step. Full board/stream, prompt,
executor identity and external-work acceptance remain the deployment operator's
post-deploy qualification; these fixture tests do not prove live behavior.

## Verify source changes

Run helper tests through PrismaBuild with 2 CPUs, 8 GiB, `CARGO_BUILD_JOBS=2`,
`RUST_TEST_THREADS=1`, and native threads bounded to one. Build the `tl3-deploy`
example, set `TL3_DEPLOY_HELPER` to that binary, and run Python unittest discovery
in this directory. The Rust test
`database_tests::fresh_reset_preserves_unrelated_database_and_refuses_live_sessions`
uses only its temporary disposable cluster and requires `TL3_FRESH_PG_BIN` to
identify prepared Postgres binaries. It checks a separate MLflow database's OID
and sentinel, rejects a live TL3 session and unsafe database names, and verifies
current-schema readiness after reset.

The source contract is recorded at
`/home/rob/tmp/tl3-build/rust-fv/ctrl-core/batch/fx-s2-file-state-contract.md`,
SHA-256 `420ee06aa5cb1fd257e746409e05fe101a432059a013aaee06e40b5ba7be33e1`.
Reconcile it against the chosen integrated FX head before deployment.

The canonical caller uses `flux-deploy.py` for native build and fresh-init admission.
The external entry still prepares the dedicated exact-source clone. Native jobs
read that clone, repeat HEAD, tree, full-history and complete porcelain checks,
and never read a shared project checkout or PB snapshot. The replaced PB
snapshot launcher is removed. Release output is staged privately and published
atomically only when complete. Existing release output requires reattachment to
its original dispatch, with a verified build receipt. Presence alone never skips
build evidence. After a failed fresh-init, keep the failed dispatch intact and set
`TL3_FLUX_DEPLOY_BUILD_DISPATCH` to its original successful build dispatch. Supply
a NEW backup path and current private manifest for the new cycle. The caller
rechecks the saved build's original jobspec, terminal and cleanup evidence before
admitting a new fresh-init. `TL3_FLUX_DEPLOY_REATTACH` resumes that new cycle by
its own saved IDs. These options are mutually exclusive on invocation; the new
cycle records its prior build reference for later reattachment.

## Native admission and reattachment

The caller pins generation `fv1-model/f107a7b075094ef6839e3a545b5dd711`,
rank-zero URI `local:///home/rob/flux-vanilla/0.91.0-0.56.0/runs/fv1-model/run/local-0`,
and the owner's broker starttime `1794024934.96`. Unknown generation, size or
sdexec configuration refuses. A restart requires an explicit reviewed source
rebind and native qualification, never newest-instance discovery.

Build requests one task, eight CPUs and eight one-GiB memory leaves. Cargo
parallelism remains two and native library threads remain one. Fresh-init requests
one task, one CPU and two one-GiB memory leaves. Both request5400seconds and
`host:dl380g10`, because release artifacts, user services and the Postgres socket
are local. The explicit slot resources contain memory alongside cores, with no
`-N1` or fixed node wrapper. Admitted memory differs from the standing75%/no-swap
unit cap and96G aggregate host cap. Real peak measurement remains a native
acceptance requirement.

Native jobspec generation starts with `--env=-* --rlimit=-*`. Only fixed PATH,
HOME, XDG_RUNTIME_DIR, approved scratch, thread limits and stage-specific Cargo
or nonce values enter `attributes.system.environment`, and the caller's
resource limits never enter `attributes.system.shell.options.rlimit`. The CLI
supervisor separately pins Flux URI, binary and library paths. Caller
environment and API keys never enter the original jobspec. Raw operational
output, manifest paths and all dispatch records remain private. Do not publish
raw jobspecs or output logs.

Each dispatch lives in a unique0700directory under
`/home/rob/.local/share/tl3/deploy-admission`. The shared directory
`/mnt/shared/flux-vanilla/deploy` is rob-owned0755 and contains only
`deploy.lock`. Invalid layouts refuse; no legacy evidence migration occurs.
See [Flux deployment owner limits](FLUX-OWNER-LIMITS.md). Intent and exact source/tree are durable before
submission. The native jobID is saved immediately. Set
`TL3_FLUX_DEPLOY_REATTACH` to the printed dispatch path after a disconnect. The
caller waits by savedID through `wait-event clean`, then collects status,
eventlog and private retained outputs. After an uncertain submission it queries
its unique name once and checks the exact original jobspec. Zero, multiple or
contradictory matches remain UNKNOWN. No blind resubmission occurs.

A store-wide nonblocking lock serializes coordinators through both stages.
After a coordinator dies, a new cycle refuses while any earlier submitted job
lacks hash-bound terminal and independent cgroup/scratch closure evidence.
Reattach the original dispatch to collect that evidence. A refusal or accepted
marker alone never clears this guard, and build reuse does not bypass it.
`closed.json` records only terminal completion and cleanup, including after a
failed command. It grants no successful verdict. Unknown cleanup remains held;
a failed fresh-init with verified closure can use a new backup and cycle as above.

An ended job without a payload result may receive a typed `FAILED` closure only
from bound Flux terminal records and independent exact cgroup/scratch absence.
It grants no PASS or deployment receipt. Missing identity or cleanup remains
held, and no evidence is deleted. See [failed terminal closure](DEPLOY-ADMISSION.md).
Manual evidence-preserving recovery remains until FX-L1 deploys.

A successful command alone is insufficient. The versioned wrapper receipt binds
intent, source tree, nativeID, terminal observations and output hashes. It requires
the recorded sdexec cgroup to be empty or removed and the exact owned scratch
root to be absent. Native `clean` alone is insufficient. A failed parent, missing
result, exception, malformed status or cleanup uncertainty refuses the cycle.
There is no native attestation or PB CAS claim. The payload uses only approved
btrfs scratch `/var/tmp/flux-vanilla/<f58ID>` and never changes CPU affinity.
Fresh-init executes directly in the admitted job with host user-bus access. It
does not submit a nested SSH or systemd payload to escape admission.

Fixture controls use a fake native transport, IDs, status and receipts. They prove
construction and refusal behavior only. They never qualify the native deployment.
The first FX+S2 release keeps installed caller `a089` and PB admission. The bridge
target ends2026-11-09UTC, subject to firstPBdeployment acceptance, this tooling's
review/FF/install, nextFluxdeployment acceptance, the lead's explicit
"Flux executor verified", and Rob's routing-policy sign-off. Installation and live
acceptance belong to the parent. No candidate installation is part of this unit.

The current matching-nightly policy remains mandatory while Kani exists. Its
authoritative gate and nightly definition must change together in the Kani
retirement unit. This tooling does not anticipate or bypass that policy change.
