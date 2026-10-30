# TL3 Mergify client

`tl3-mergify` queues and dequeues pull requests through `tl3-github` and reads
Mergify's queue reports. It is a standalone Rust 2024 crate with its own lockfile.
The workspace coordinator adds it and `crates/github` to the root workspace.

## Commands

`Mergify::new(github)` takes an authenticated installation client.
`queue(repo, number, approved_head, effect_key)` posts the documented bot queue
command. It first checks that the PR is open and its head matches `approved_head`.
`dequeue(repo, number, effect_key)` posts the bot dequeue command.
The default queue command picks the first matching queue rule. Named queues
and priority changes are outside this client.

These commands address Mergify's bot, not a person. They match Rob's manual queue
configuration and reuse the GitHub client's authenticated REST transport and
comment reconciliation. No separate Mergify credential is needed.
A successful return confirms that GitHub stored the comment. It does not confirm
that Mergify accepted or completed the command. The deployment must permit TL3's
App to run queue and dequeue commands. Mergify requires write permission by
default, and repository command restrictions can impose more conditions.

Reuse an effect key only when recovering the same operation. Queue and dequeue
use separate marker namespaces. A later intentional requeue needs a new key.
If a write fails ambiguously, retry with the same key to reconcile its comment.
Comment reconciliation prevents duplicate posts in normal recovery, but two
independent processes can race before either sees the other's post. The controller
must keep one writer per effect. Deleting a duplicate cannot undo a command the
bot has already processed.

Mergify also supports label conditions with a configured queue action or merge
protections. A label alone does not enqueue anything in Rob's config. Its current
REST merge-queue API documents dequeue, inspection, and pause operations, but no
matching enqueue operation. Bot commands provide both operations without changing
repository configuration. Dequeue also cancels a pending queue command. It can
cascade to queued successors of a stacked PR.

Sources, read 2026-10-30:

- [Queue command](https://docs.mergify.com/commands/queue/)
- [Dequeue command](https://docs.mergify.com/commands/dequeue/)
- [Command restrictions](https://docs.mergify.com/commands/restrictions/)
- [Queue lifecycle](https://docs.mergify.com/merge-queue/lifecycle/)
- [Merge queue API](https://docs.mergify.com/api/merge-queue/)

## Observations

`read(repo, number)` fetches the PR, its head's check runs, and its issue comments.
It reads the PR again and rejects the observation if that record changed during
the read. This detects a push or merge across those calls, but GitHub does not
provide an atomic snapshot of the PR, checks, and comments. Webhook recovery and
later reads must reconcile concurrent queue changes.

`Observation.state` describes the current head. Only the newest check named
`Mergify Merge Queue` from App ID 10562 with slug `mergify` qualifies.
The `Summary`, workflow rule, merge protection, and unrelated CI checks do not.
The client requires GitHub's `merged_at` before reporting current state `Merged`,
regardless of what Mergify reports. GitHub's merge record wins even when checks
are missing or delayed.

The client recognizes these reports:

| Report | Fact |
| --- | --- |
| `In merge queue`, or payload state `queued` | `Queued` |
| `Running merge queue checks`, or payload state `checking` with `checks_running` | `ChecksRunning` |
| GitHub PR has `merged_at` | `Merged` |
| Dequeued report without a CI failure or conflict reason | `Dequeued` |
| Dequeued with `CHECKS_FAILED` or `CHECKS_TIMEOUT` | `FailedCi` |
| Dequeued with `CONFLICT_WITH_BASE_BRANCH` | `Conflict` |
| Queue ready or waiting for entry conditions | `Waiting` |
| Missing, malformed, unknown, or future report | `Unknown` |

A check's GitHub status or conclusion alone does not identify queue state.
For example, a neutral check can describe a PR that never entered the queue.
Interrupted checks report a dequeue, not a proven CI failure. A held PR whose
checks are deferred remains queued. Unknown steps stay unknown.

`Observation.events` contains historical reports from bot ID 37929162 with login
`mergify[bot]`, sorted by update time and comment ID. Each event preserves the
comment and its timestamps. A comment has no reliable head binding, so historical
reports never override current-head state. The controller must deduplicate events
by comment ID and update time and use its persisted queue history when applying
them. In particular, an old failure is not a new failure after a requeue.

Mergify embeds version 1 JSON in some comments and base64 JSON in some check
summaries. These encodings are observed output, not a documented stable API.
The parser requires that version and recognizes known states. For checks without
metadata it recognizes documented or recorded titles. Older comments without a
reason object use exact first lines of their `Reason` section for CI failures and
base conflicts. Unknown reports retain their original evidence for investigation.
A format change requires new recordings and parser review, never an assumed pass.

Mergify's lifecycle documentation identifies the unified queue check and its
waiting and running titles. `status_comments: all` reports progress and outcomes;
`outcomes` reports only final outcomes. Queue labels can also expose state, but
are configurable and are not used as facts here.

- [Unified queue check](https://docs.mergify.com/changelog/2026-04-21-unified-mergify-merge-queue-check-run/)
- [Queue reasons and batch states](https://docs.mergify.com/configuration/data-types/#queue-dequeue-reason)

## Approval and in-place checks

The state table requires CI on the exact merged tree, TL3 App approval of the exact
head, and no head rewrite by Mergify. The client does not configure or certify
repository protections. Before serving a repository, the configuration checker
must verify the required approval check's name and TL3 App identity, all applicable
queue and merge conditions, branch protections or rulesets, and bypass permissions.
A same-name check from another App is not approval.

Rob's [PrismaBuild configuration](https://github.com/RobTand/prismabuild/blob/main/.mergify.yml)
uses serial mode, one parallel check, batch size one, zero retries, and no separate
two-step CI. Mergify calls this in-place checking: it tests the original PR branch
without creating a draft batch PR. With `update_method: merge`, it can first merge
the base branch into that PR branch. In-place does not mean the head stays fixed.

That update behavior conflicts with TL3's no-head-rewrite rule whenever an update
is needed. Rob's example explains his queue operation; it is not an approved TL3
configuration. TL3 needs a configuration that preserves the approved head and
checks the exact integration tree. This crate changes no state-table rule and
makes no repository configuration changes.

The controller must compare every observed head with its approved head, including
synchronize webhooks and recovery reads, and raise an incident for an unauthorized
push. `queue` only performs a preflight comparison, which cannot prevent a push
between the read and the command. Repository protections provide that guarantee.
After a merge, the controller must record the actual merged head and tree and
raise an incident if TL3 did not approve them. A Mergify success title is never
approval evidence. `Observation.pull_request` preserves the head for these checks;
retrieving and verifying the merge commit and tree belongs to the controller's
GitHub integration.

- [In-place checks](https://docs.mergify.com/merge-queue/batches/#in-place-checks-no-batch-prs)
- [TL3 state table](../../docs/state-table.md)

## Verification

Tests replay recorded GitHub REST responses against a fake GitHub server. They
also exercise command receipts, retries, App and head filtering, malformed reports,
and a push during observation. No test contacts GitHub or posts a live command.
See [fixture provenance](tests/fixtures/README.md).

Run the crate's tests through PrismaBuild, from a clean commit:

```bash
python3 /mnt/shared/prismabuild-fleet/repo/tools/pbrun.py \
  --cwd "$PWD" --anywhere --cpus 2 --demand mem_gb=4 \
  --env PATH=/home/rob/.cargo/bin:/usr/local/bin:/usr/bin:/bin \
  --env RUSTUP_HOME=/home/rob/.rustup --env CARGO_HOME=/home/rob/.cargo \
  --env CARGO_BUILD_JOBS=2 --env RUST_TEST_THREADS=2 \
  --env OMP_NUM_THREADS=1 --env MKL_NUM_THREADS=1 --env OPENBLAS_NUM_THREADS=1 \
  -- cargo test --manifest-path crates/mergify/Cargo.toml --locked
```
