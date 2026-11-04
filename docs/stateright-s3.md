# S3: effects under lost responses

`crates/controller/tests/stateright_s3.rs` checks the delivery contracts in
[the effect machine](state-table.md#the-effect-machine). Issue #747 originally
asked for universal at-most-once mutation. Rob amended that requirement to
match the table, including temporary duplicate messages and epoch fencing.
No state-table rule or production behavior changes.

## Properties follow the delivery contract

| Contract | Table rows | Checked property |
| --- | --- | --- |
| Once | Submit a PrismaBuild job, approval check, start or stop an agent run | At most one upstream mutation per key. Healthy, fair delivery eventually confirms exactly one mutation for every eligible key. |
| Converges | Withdraw a job, push a branch, open or reopen a PR, enqueue or dequeue, close a PR, worktree or branch | Repeated delivery reaches the target state and preserves it. |
| At least once, then deduplicated | Comment or message to Rob | A temporary second copy is reachable. After reconciliation exactly one marked copy remains, and reconciliation never deletes the last copy. Fair healthy reconciliation eventually leaves one copy. |
| Epoch fence | Paragraph following the table | An application whose epoch is older than the current upstream fence cannot mutate upstream. |

An eligible key has not been cancelled by an epoch advance. A stale unsent
effect can terminate without an upstream mutation, as the table requires.
An effect applied before the fence advances is allowed to remain applied.
Its recorded acknowledgement is still usable after a response is lost.

The model represents each key by its fixed vector index. The keys never change
on retry. Per-key upstream records are separate from visible mutations and
message copies. The store's credit ledger is not an upstream deduplication
record and is outside S3.

## Transitions come from the implementation

References below name the inspected source at
`e3292d911a6e42c89f1f197b90648419a39a6b3f`.

| Model action | Implementation |
| --- | --- |
| `CommitSent`, `Dispatch` | `crates/controller/src/lib.rs:1190`, `drive_once_with` commits Sent before calling `execute`. |
| `LoseResponse`, `MarkUncertain` | `crates/controller/src/lib.rs:1255`, transport error preserves Sent; line 1271 advances it to Uncertain. |
| `ReadRecord` | `crates/controller/src/lib.rs:1298`, reconciliation returns Found, Absent, or Pending. Found completes the original key. |
| `RetrySameKey` | `crates/controller/src/lib.rs:1332`, authoritative absence returns the existing effect to Intended. |
| `CommitConfirmed` | `crates/controller/src/lib.rs:1555`, `complete` commits the acknowledgement, transition, and resulting observations. |
| `Deliver`, Once | `crates/controller/src/adapters/record.rs:33`, per-key reconciliation; `crates/prismabuild/src/submit.rs`, action-key submission; `crates/controller/src/adapters/github/publishing.rs`, approval external-id reconciliation. |
| `Deliver`, Converges | `crates/controller/src/adapters/github/publishing.rs`, reconciliation against branch head and PR state; `crates/controller/src/adapters/mergify.rs`, queue-state reconciliation. |
| `DuplicateDelivery`, Message | `crates/github/src/client.rs:677`, marker read before POST is not an atomic upstream deduplication operation. `crates/controller/tests/crash/upstreams.rs:246` explicitly persists two copies. |
| `ReadRecord`, `ReconcileCopies` | `crates/github/src/client.rs:697`, `reconcile_comments` keeps the oldest app-owned marked comment and deletes extras. The same adapter operation can be called again after confirmation. |
| `AdvanceFence`, `CancelStale`, fenced `Deliver` | `crates/controller/src/adapters/record.rs:44`, monotonic fence before writes; `crates/controller/src/lib.rs:1148`, stale delivery cancellation. |

`LoseRequest` represents a failed attempt that made no upstream write. Only a
subsequent authoritative Absent read permits retry. A lost response alone
never establishes absence. Store transactions and upstream-record operations
are atomic in this abstraction. S1 owns their interior crash boundaries.

## Bounds and liveness assumptions

Each run exhausts one or two keys, at most two network losses in total, and at
most one duplicate delivery. It checks all three single-contract cases and
all nine ordered two-contract combinations. Each key has its own resource or
marker. Conflicting target writes to one resource and outside writers are
outside this model.

Epochs are 1 and 2. The upstream fence starts at 1 and can advance explicitly
or when it accepts an epoch-2 delivery. Both early cancellation and rejection
of an already dispatched stale delivery are explored. A third epoch adds no
new ordering relation for this bounded model.

Liveness assumes healthy upstreams, fair dispatch and reconciliation, and
eventual completion of in-flight requests. It does not assert that a healthy
network alone schedules a controller look. For messages, the premise includes
the next reconciliation call, including when a successful response was received.
Requests drain before the attempt's response or authoritative reconciliation.
Indefinite outages, infinite Pending replies, and arbitrary scheduling stutters
are excluded from the healthy executions.

Stateright 0.31's `eventually` checker requires acyclic paths. The model has no
unbounded stuttering actions: retry loops consume a loss, duplicates consume
their budget, the fence only increases, and message cleanup reduces two copies
to one. With these budgets fixed, each remaining phase progresses to a terminal
state. No depth cutoff or state-count cutoff is used. Reachability properties
require two losses, duplication, absence and retry, stale rejection, successful
completion, and temporary duplicate messages. They prevent vacuous success.

## Conformance and retained scenarios

`lost_response_keeps_the_key_real_trace_conforms` runs the real controller,
Postgres store, and persistent worker fake. It compares outbox state, epoch,
pending membership, and upstream delivery count after each drive. A Pending
read must leave the real snapshot unchanged. Healthy reconciliation must
confirm the original key without another execute. This replays the existing
simulation scenario `lost_response_keeps_the_key`.

`real_comment_reconciliation_conforms_and_keeps_the_last_copy` replays the
two-copy state through the actual GitHub client's reconciliation against a
stateful local HTTP fixture. A second read must retain the same oldest copy.
`real_adapter_epoch_fence_conforms` compares a newer write followed by an old
write against the real persistent adapter fence.

No scenarios are removed. S3 can inform later review of simulation
`lost_response_keeps_the_key`, `sent_but_absent`, and
`retry_uses_the_kernels_new_key`; Mergify lost-response and refusal-bound
scenarios; GitHub notice retries; Hindsight lost-ack retain scenarios; and
publishing lost-response recovery. Their adapter-specific evidence and
concrete long traces remain necessary until #727 or a later pruning review.
