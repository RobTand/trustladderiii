# TL3 store

`tl3-store` is a Rust 2024 crate in the root workspace, with one shared lockfile. It imports
no kernel types. Events, row values, effects and commit results are JSON.

The controller uses it in this order:

```rust,no_run
use serde_json::json;
use tl3_store::{Commit, RowChange, Store};

# fn example(config: &tl3_config::Config) -> tl3_store::Result<()> {
let mut store = Store::open(&config.store)?;
let world = store.load()?;
// Apply the kernel to world.rows and the incoming event here.
let mut change = Commit::new("github-delivery-key", json!({"request": 55}), &world);
change.rows.push(RowChange::Put {
    table: "nodes".into(), id: "1".into(), value: json!({"phase": "Ingesting"}),
});
let receipt = store.commit(&change)?;
// Only now may the controller execute committed effects.
# let _ = receipt;
# Ok(())
# }
```

## Design

Two designs were considered: an async SQLx pool with a migration framework,
and one synchronous `postgres` connection with numbered SQL files. The second
fits the controller's serial load/step/commit cycle and avoids exposing an
async runtime, pool policy or kernel types. Readers open separate connections.
Production dependencies are `postgres`, `serde`, `serde_json`, `thiserror`,
and the existing config crate. An async controller can put the store on its
writer thread.

`Store::open(&config.store)` reads connection settings from `tl3-config`.
This PR adds its optional `[store]` section. Credentials are never in a URL.
Only an explicitly configured password file is read, at runtime. No default
credential file or environment variable is consulted. The client supports
TCP through `localhost`, `127.0.0.1`, or `::1`, and absolute Unix socket
directories, without TLS. Other hosts are rejected. A remote deployment needs
a protected tunnel with a loopback endpoint. Connection errors omit connection parameters.
Opening refuses a server with fsync disabled and enables synchronous commits
on its connection, so a successful commit waits for local WAL durability.

Migrations run on open under a transaction-scoped advisory lock. Each numbered
SQL file and its exact text is recorded in `tl3_schema_migrations`, in the
same transaction as the schema change. A changed or newer migration refuses
to open. Add a new migration rather than editing an applied one.

## Commit and recovery

`commit` locks the singleton world revision before checking the event key.
Duplicate keys with identical JSONB payloads return the original revision,
epoch and JSON result, including after later commits or an upgrade. A different
payload returns `Error::EventKeyReused` without changes. New events require
the loaded revision and epoch; `Error::Stale` tells the caller
to reload and apply the event again. Independent writers cannot silently
overwrite a world they loaded before another commit.

The transaction inserts the append-only event, upserts or deletes only the
changed rows, inserts new Intended effects, applies delivery transitions,
and adds credit entries. Row versions are the global revision of their last
change. SQL table names and row ids are values, never SQL identifiers.
Each row also has an immutable monotonic `arrival` sequence assigned on insert.
`load` returns rows by table and arrival, so inbox consumption follows insertion
order across commits and within one commit. Sequence gaps from rollbacks are harmless.
Migration 2 backfills old rows by revision, table and key. Old versions did not
record order within one transaction, so drain their inbox before migrating.
An effect key cannot be reused, even after it becomes final. The SQL trigger
and Rust API both enforce the state table's effect transitions. A transition
also checks the expected delivery state. The controller supplies the evidence
for a transition, including reconciliation and the end of a cancelling attempt.
The store cannot establish that evidence itself.

Credits are keyed by node and reason. Repeating that pair with the same root
and amount has no effect; changing either refuses the whole commit. Include
the operation or generation in reasons for repeatable charges. Credit policy
and balances stay in the kernel. The event log and ledger reject SQL update,
delete and truncate operations. The application database role should not own
schema DDL privileges after deployment migrations.

`Reader::open` opens a separate connection with `default_transaction_read_only`
enabled and never runs migrations. It exposes only read-only, repeatable-read
transactions and snapshot loading. `load_snapshot` lets a status reader query
additional tables in the same transaction.

`load` uses a read-only repeatable-read transaction. It returns the current
rows, all Intended/Sent/Uncertain/Cancelling effects, the credit ledger and
intern dictionary from one database snapshot. Final effects remain available
through `effect(key)` or the complete `effects()` audit reader. `events_after` is a paginated audit reader. Recovery
loads the world; it never replays old events into a newer kernel.

An upgrade sets `next_epoch` to the current epoch plus one and supplies its
migrated rows in the same commit. Sent, Uncertain and Cancelling deliveries
must drain first. Intended effects keep their keys and receive the new epoch.
The store fences later writes from an old controller. Executors must still
check effect epochs before outside actions, as the state table requires.

## Interning

The get-or-create methods return typed 64-bit ids. Repos ignore ASCII case;
commit SHAs ignore hexadecimal case and are scoped by node. Paths preserve
their exact Git spelling and are scoped by repo. Issue and PR numbers are
separate namespaces scoped by repo. Concurrent inserts return the same id.
Ids can have gaps. Interning commits independently because unused dictionary
entries have no world semantics. Recovery includes the dictionary; repo
records expose `repo_id()` so callers can resume scoped interning.

## Verification

Run `cargo test --locked --manifest-path crates/store/Cargo.toml` through
PrismaBuild during development. The existing CI discovery script also finds
this crate, with no workflow changes and no ignored database tests.

The integration executable starts Postgres 17.4 through `postgresql_embedded`
on an ephemeral port, using a current-thread runtime. The downloaded binaries,
database and fixture password files live in its unique job temporary directory.
Nothing uses Docker or deployment credentials. The recovery fixture explicitly
overrides the embedded crate's `-F` flag to keep fsync enabled. Store connections
enable synchronous_commit. A separate disposable server with fsync off tests
the refusal to open. Tests run sequentially against separate databases
on that child server. Both Linux x86_64 and aarch64 archives come from Zonky;
Ubuntu hosted runners use the same fixture. Initial setup time is printed in
the job log. The first run also compiles the Rust dependencies.

The scenarios exercise rollback after partially writing a transaction,
redelivery, all 49 pairs of effect states, pending/final loading, concurrent
writers and redelivery, scoped interning, credit deduplication, schema migration checks,
append-only guards, row deletion, and epochs. The crash scenario starts a
separate store writer. Its first commit succeeds; a database trigger blocks
its second transaction after writing the event, rows and effects. The parent
checks the uncommitted data is invisible, kills the writer with SIGKILL,
reopens the store and compares the complete snapshot with the first commit.
It then reuses the killed event key successfully.

Migration 4 adds GitHub ingress. `tl3_ingress` retains verified raw webhook
bytes and complete catch-up results until intake and every registered translator
advance past them. `ingress_after(sequence)` reads deliveries pending for those
translators. Each translator advances its own `tl3_ingress_cursors` row only
after applying idempotent inputs. This does not change the kernel revision.
`Store::reconnect` replaces the ingress source's connection after a failed insert;
the controller's ownership session is never reconnected.

## Worker data

Migration 3 adds supplied worker materials and a liveness side table.
`put_worker_materials` supplies the original issue and repository text for a
request node. `worker_materials` reads it without taking controller ownership.

`claim_worker` records the attempt start before launching it. Repeating the same
claim preserves its time. A later attempt resets progress. `record_worker_progress`
accepts a run key, attempt, complete output byte offset and progress unit.
Only a newer offset updates progress time. Replaying stdout leaves it unchanged.
Old epochs cannot update the table. These writes never change the kernel revision.

`tl3_worker_progress` has one row per `run_key`, with `node_id`, `epoch`, `attempt`,
`output_offset`, `unit`, `started_ms` and nullable `last_progress_ms`. Times are
Unix milliseconds observed by Postgres. Before progress, the offset is zero,
the unit is empty and `last_progress_ms` is NULL. Ticket #15 uses `started_ms`
when a run never reports progress. Ticket #18's status hook receives a Postgres
`Transaction` and queries this table directly within its read-only transaction.
The `(node_id, last_progress_ms DESC)` index supports per-node reads.
`worker_progress(None)` reads unretired runs by start time, newest first.
`worker_progress(Some(node))` filters by request node.

The controller stores accepted ingest replies, goals and audits as world rows
in the same transaction as their kernel event. `worker_document(node, name)`
reads `briefs`, `goals` or `audits` for status and later worker inputs.


`worker_watches()` reads only watched worker deliveries. Startup uses it instead
of loading run history. `worker_reconcilable(key)` returns false for a terminal
effect with no watch. Stale ready notifications cannot reattach that run.
`retired_workers()` lists progress keys whose effects are Confirmed, Failed or
Cancelled and unwatched. Sent, Uncertain and Cancelling effects retain their evidence.
`forget_worker(key)` rechecks that predicate and deletes the progress row only
after the caller has durably removed its files. It atomically folds any last
progress into `tl3_worker_summary`. The summary keeps the newest progress time
and unit in one row per open request node, regardless of the number of retired runs.
World commits delete the summary when that node closes or is removed. Cleanup
after closure cannot recreate it. Summary insertion serializes with world commits.

Migration 5 adds this summary, a shared terminal-and-unwatched SQL view, and
`tl3_worker_progress.retirement_blocked`. `block_worker_retirement(key)` marks
a corrupt or foreign process identity after logging it once. Future drains and
restarts skip that key, retaining its files and progress for operator repair.
Other cleanup failures retain their queue row for a later drain.

The status hook reads active progress first, or the per-request summary when
no active run exists, within its existing read-only snapshot. A worker that
never reported progress still shows no recorded progress. Closed requests have
no summary. These side-table writes do not change the kernel revision.


## MVP and Hindsight upgrades

Migration 6 is `0006_worker_wait.sql`. Its deployed SQL remains unchanged.
Migration 7, `0007_hindsight.sql`, adds `tl3_memory_outbox`. Migration 8,
`0008_memory_parking.sql`, adds parked records and `tl3_memory_health`.
Migration 9, `0009_memory_recovery.sql`, adds parking timestamps and an HTTP
expiry index. Existing parked rows receive the migration time. Flushes expire
HTTP parking after 30 days and replay capacity parking after successful delivery.
`mvp_schema_6_upgrades_to_hindsight` recreates the MVP schema, upgrades it,
and checks the Hindsight tables, parking and preservation of an existing wait.
Readers require schema version 9 after this upgrade.
A successful retain deletes its outbox row and clears key-rejection health in
one transaction. Repeated rejections update the status without duplicating the
incident. Queue and replay writers share a table lock to preserve the 10,000
pending-row cap.
