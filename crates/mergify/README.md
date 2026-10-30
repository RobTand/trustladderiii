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
```

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
- `dequeue(repo, number)` removes that label. An absent label is harmless.
- `read(repo, number, approved_head)` fetches the PR and its head's checks, then
  rereads the PR. A concurrent push or merge returns `ChangedDuringRead`.
- `is_merge_queue_pr(&pull_request)` recognizes branches under
  `mergify/merge-queue/` authored by `mergify[bot]`. TL3 must ignore their events.

Retrying a label operation converges without an effect key. A successful write
confirms the label change, not queue membership. Reconcile through `read`.
The controller must serialize opposing queue and dequeue effects and handle a
push between queue's preflight read and label write.

`observe(pull_request, checks, approved_head)` is the pure observation function.
It selects the newest `Mergify Merge Queue` check on this head from Mergify's
App, ID 10562 and slug `mergify`. It recognizes queue, running, waiting, CI failure,
conflict, and dequeue reports. Unknown titles or malformed metadata stay `Unknown`.
The versioned metadata is observed output, not a stable API. Keep the returned
check as evidence when a format changes.

Only GitHub's `merged_at` confirms `Merged`. `Observation.unapproved_merge` is
true when that merged PR's head differs from `approved_head`, even without a
Mergify check. The controller must record that merge truthfully, raise an incident,
and tell Rob. Checking the actual merge tree and its CI remains the controller's
responsibility under the [state table](../../docs/state-table.md).

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
