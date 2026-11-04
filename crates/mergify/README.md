# TL3 Mergify client

`tl3-mergify` uses an authenticated `tl3-github` installation client. It queues
PRs with a label and reads the current head's Mergify check.

## Served repository configuration

Served repos must use temporary draft PRs for queue checks. In-place checks can
merge main into the original branch, rewriting the head TL3 approved. Set
`max_parallel_checks: 2` to disable in-place checks. Put TL3's approval in
`queue_conditions`: check conditions in `merge_conditions` run on the draft PR,
where TL3 never posts its approval.

Use this configuration as the starting point for a served repo. Replace `tl3`
with the installed TL3 App's slug, `TL3 approval` with its check name, and the CI
check names with the repo's required checks. Create the `tl3-queue` label first.

```yaml
merge_queue:
  mode: serial
  max_parallel_checks: 2

queue_rules:
  - name: default
    queue_conditions:
      - base = main
      - label = tl3-queue
      - check-success = @tl3/TL3 approval
    merge_conditions:
      - check-success = Rust
      - check-success = Python
    merge_method: merge
    batch_size: 1
    max_checks_retries: 0

pull_request_rules:
  - name: Queue TL3-approved PRs
    conditions:
      - base = main
      - label = tl3-queue
      - check-success = @tl3/TL3 approval
    actions:
      queue:
        name: default

commands_restrictions:
  queue:
    conditions:
      - or:
          - sender-permission>=write
          - sender=tl3-robtand[bot]
  requeue:
    conditions:
      - or:
          - sender-permission>=write
          - sender=tl3-robtand[bot]
```

Allow TL3's App login in `.mergify.yml` `commands_restrictions` for both `queue`
and `requeue` before serving the repository. The deployed App is
`tl3-robtand[bot]`; replace that login if using another App. Installing the App
does not give its commands a human collaborator's write permission. Without an
explicit allowance, Mergify can accept the comment and then refuse the command.
See [command restrictions](https://docs.mergify.com/commands/restrictions/).

Keep the label in `queue_conditions` so removing it dequeues the PR. TL3 posts
its successful approval check on the reviewed head before adding the label.
Require approval from TL3's App, restrict pushes to TL3's branches, and check the
repo's protections before serving it. In the sandbox trial, confirm that **the
original branch is never touched in draft mode**, including when main advances.
This crate does not configure repos or certify that guarantee.

See Mergify's [batch checks](https://docs.mergify.com/merge-queue/batches/#in-place-checks-no-batch-prs),
[queue conditions](https://docs.mergify.com/merge-queue/rules/), and
[queue action](https://docs.mergify.com/workflow/actions/queue/).

## Client operations

- `Mergify::new(github)` takes an installation client with issue write permission.
- `queue(repo, number, approved_head)` requires an open PR on that head, then
  adds `QUEUE_LABEL`, `tl3-queue`. Adding an existing label is harmless.
- `requeue(repo, number, approved_head, effect_key)` checks the open PR's head,
  then posts the queue command as the App, with its effect marker below it.
- `dequeue(repo, number)` removes that label. An absent label is harmless.
- `read(repo, number, approved_head)` fetches the PR and its head's checks, then
  rereads the PR. A concurrent push or merge returns `ChangedDuringRead`.
- `is_merge_queue_pr(&pull_request)` recognizes branches under
  `mergify/merge-queue/` authored by `mergify[bot]`. TL3 must ignore their events.

Retrying a label operation converges without an effect key. A successful write
confirms the label change, not queue membership. Reconcile through `read`.
The controller must serialize opposing queue and dequeue effects and handle a
push between queue's preflight read and label write. Before requeueing, it waits
for a fresh `dequeued` label or the served repo's `Rule: Queue TL3-approved PRs (queue)` check
to conclude. `Observation.dequeue_rule_concluded` rejects an older rule result
and requires both check completion times. The adapter uses GitHub label events
to reject a `dequeued` label left over from before the latest queue entry.

Mergify's [queue command](https://docs.mergify.com/commands/queue/) resets a dequeued
PR; the older requeue command is deprecated. Its [command syntax](https://docs.mergify.com/commands/)
requires the command to start the comment. An HTML marker before it would prevent
execution. The adapter journals dispatch before posting, and a command receipt
alone does not confirm queue acceptance.

`observe(pull_request, checks, approved_head)` is the pure observation function.
It selects the newest `Mergify Merge Queue` check on this head from Mergify's
App, ID 10562 and slug `mergify`. It recognizes queue, running, waiting, CI failure,
conflict, and dequeue reports. Unknown titles or malformed metadata stay `Unknown`.
The versioned metadata is observed output, not a stable API. Keep the returned
check as evidence when a format changes.

Only GitHub's `merged_at` confirms `Merged`. `Observation.unapproved_merge` is
true when that merged PR's head differs from `approved_head`, even without a
Mergify check. The controller must record that merge truthfully, raise an incident,
and tell Rob. `merged_via_queue` recognizes the selected successful Mergify check
reporting a queue merge. The controller uses that evidence for queue CI and reads
merge-commit checks for outside merges under the [state table](../../docs/state-table.md).

## Verification

Tests replay [recorded responses](tests/fixtures/README.md) against a fake GitHub
server and exercise label retries, head checks, and merge approval. From a clean
commit, run formatting, Clippy with warnings denied, and tests through PrismaBuild:

```bash
python3 /mnt/shared/prismabuild-fleet/repo/tools/pbrun.py \
  --cwd "$PWD" --anywhere --priority -10 --cpus 2 --demand mem_gb=4 \
  --env PATH=/home/rob/.cargo/bin:/usr/local/bin:/usr/bin:/bin \
  --env RUSTUP_HOME=/home/rob/.rustup --env CARGO_HOME=/home/rob/.cargo \
  --env CARGO_BUILD_JOBS=2 --env RUST_TEST_THREADS=2 \
  --env OMP_NUM_THREADS=1 --env MKL_NUM_THREADS=1 --env OPENBLAS_NUM_THREADS=1 \
  -- bash crates/mergify/tools/verify.sh
```

## Queue decisions and their proofs

`src/decide` holds the queue adapter's decisions as pure functions over small
enums (#735). The client here and the adapter in `crates/controller` read
GitHub, parse its strings into these types, and act on the result.

- `check`: a check run as enums: status, conclusion, app, name and head. Check
  names and app IDs are interned per read, so grouping compares integers.
- `observe`: which queue check describes a PR, and the queue state.
- `facts`: which facts a queue read yields. That includes the Q52 missing-check
  rule, the #447 blocked queue, merge evidence and a person's dequeue or close.
- `translate`: how a queue fact reaches the kernel. Setup evidence (Q52, #447,
  unreadable checks) only opens or clears incidents. It never becomes a kernel
  fact, so it never counts a failure or charges credit.

`tl3-github`'s `status` module maps REST status codes to failure classes.

The decisions do no I/O and have no `unsafe`, no floats and no target-specific
code, so their proofs hold on amd64 and arm64.

`src/decide/proofs.rs` and `crates/github/src/status.rs` hold one Kani harness
per property. Lists hold at most three checks or events. Three entries give
every shape the grouping rules tell apart: a newer, an older or an equal ID in
the same group, and another group. Every harness has cover checks and a seeded
mutant in `tools/kani.py`. The tool fails a harness whose covers are not all
satisfied, and a mutant the harness does not reject.

Until #726 gives each harness a Bazel target, run the proofs on PrismaBuild
from a clean checkout:

```bash
python3 crates/mergify/tools/kani.py manifest > /home/rob/tmp/tl3-mergify-kani.json
python3 /mnt/shared/prismabuild-fleet/repo/tools/pbcampaign.py --transport pool --max-inflight 2 /home/rob/tmp/tl3-mergify-kani.json
```

Inside an admitted action, `kani.py prove [NAME...]` proves harnesses and
`kani.py mutants [NAME...]` checks each seeded mutant. The string parsers have
proptest round-trips in `tests/parse.rs`.

### Enqueue lifecycle (#751)

`decide::enqueue::decide(state, event)` returns the next `EnqueueState` and an
ordered effect list. Ordinary builds use a `Vec`; Kani uses a three-slot
outbox. `Read` effects request only the next necessary input. `SaveIntent`,
`SaveRecovery` and `SaveRerun` precede their upstream writes. The adapter stops
if a save fails. Label dispatch follows `SaveIntent`, the shell fence, then a
fresh `LabelRead` validation. Recovery samples time after eligibility and
before command-history reads; dispatch reuses that tick.

The state is a projection, not a new storage format. The controller still
reads and writes the same `.intent`, `.applied`, `.reruns`, head recovery and
acknowledgement files. It retains the same command keys and retirement rules.
No state moves into the store in this step.

Seven harnesses in `enqueue_proofs.rs` cover exact reconciliation outcomes,
stale heads, sticky refusals, write-ahead ordering, recovery grace and budget,
dequeue freshness, and required-check classification. Their scalar inputs
range over their full domains. Unwind 4 covers the three effect slots and
loop exit. The recovery harness checks timer expiry and two consecutive
command dispatches. It does not prove crash/restart interleavings, file I/O,
command-key adoption, annotation parsing, or check/run selection. Keep all
185 controller scenarios until those assertions have their own coverage.

`tools/kani.py verify NAME` rejects that harness's seeded mutants and then
proves the restored source. Run it only inside an admitted PrismaBuild action
until #726 provides the required per-harness BuildBuddy targets. These proofs
are development evidence; the current Gate does not run them.
