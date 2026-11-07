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
artifacts through PrismaBuild when needed. It never changes model ladders,
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
   hygiene before stopping. Size the exact file inventory against the bounded
   action budget. Stop tl3d, prove `inactive`, explicitly stop and prove
   MLflow/status/console inactive, then stop Postgres and prove it inactive.
3. Copy the complete cluster including WAL and declared tablespaces, controller
   journals, attempts and private config. Hash every file and record ownership,
   modes, directories and scope. Verify the copied inventory against the original pre-copy hashes,
   then flush the copy before any reset. Undeclared symlinks and special files refuse.
4. Empty only the manifest's supported adapter directories. Keep the cold copy
   as diagnostic evidence. Install each selected release binary from one hash-checked buffer, then verify
   the installed checksums before executing any selected binary.
5. Start Postgres and check readiness. The SQL helper revalidates the cold copy and unchanged config without network
   calls. A one-use, expiring capability crosses only private orchestrator pipes;
   the saved context contains only its digest. Archived context alone cannot
   authorize a reset. Atomic consumption precedes SQL mutation, and Postgres
   system identity must match the cold copy. It refuses live TL3 database
   sessions, drops only the constant identifier `tl3`, recreates it, initializes
   this revision's complete schema on the empty database, and verifies the reader.
6. Start selected tl3d, verify its schema and webhook HTTP response, then explicitly
   start MLflow, status and console. Require active units and 90 seconds of stable
   readiness of all four client units with zero restarts.

The canonical PB actions have a 7200-second deadline, with the exact-source child
bounded at 7100 seconds. Preflight estimates inventory transfer cost conservatively
at 20 MiB/s with eight I/O passes plus 1800 seconds overhead; it refuses a predicted
budget above 6900 seconds before any stop. This is a sizing bound, not a measured
live cold-copy timing claim. The data root is read for its initial hash, copy
verification and SQL-boundary copy verification, without rehashing it at file reset.
Actual fleet/cold-copy timing still requires deployment acceptance.

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

The canonical caller runs each admitted action through `exact-source.py`. PrismaBuild seals a child snapshot with a tracked closure stamp; the launcher makes a private temporary clone on the PB workspace volume and checks out the selected
qualified commit. It strips inherited GIT_* repository overrides and verifies
complete history, the selected tree, and parent ancestry before executing.
Cargo output is explicitly on the configured workspace volume, not TMPDIR.
Release output is staged privately and atomically published only when complete;
a failed build/copy does not publish a retry-wedging directory. Build and deployment retain their strict clean-tree checks. The snapshot itself remains unchanged.

The current matching-nightly policy remains mandatory while Kani exists. Its
authoritative gate and nightly definition must change together in the Kani
retirement unit. This tooling does not anticipate or bypass that policy change.
