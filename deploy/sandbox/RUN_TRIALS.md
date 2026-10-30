# Run the remaining sandbox trials

The successful queue merge and main-advancement trials are blocked on sandbox CI.
At the follow-up inspection, the sandbox had no registered self-hosted runners,
and `.github/workflows/tests.yml` still selected `ubuntu-24.04`. The account's
hosted Actions billing limit prevents new `Sandbox tests` runs.

## Prepare CI

The coordinator extends ticket #8's fleet runners to `RobTand/tl3-sandbox`.
If a workflow change needs code-owner review, report its PR number and full head
SHA to the coordinator. The coordinator records Rob's approval with his account.
Do not submit that review yourself or use an admin bypass. Update the approved
workflow copy and hash in a reviewed follow-up after its sandbox PR merges.
Run the settings checker again through PrismaBuild.

## Submit trial operations

All agent trial execution, including the recorder and verifier below, runs in
PrismaBuild admitted actions. Use a clean commit, 1 CPU and 1 GiB for Python or
REST-only commands. Use 2 CPUs and 6 GiB for Rust builds, with
`CARGO_BUILD_JOBS=2`, `RUST_TEST_THREADS=2`, and native thread counts bounded to 1.
Use `--anywhere --priority -10`; pin a host only if authenticated runtime access
is available only there. Retain terminal records, logs and CAS receipts. CI runs
directly on its fleet runners, outside these development actions.

Build `sandbox_gate` through PrismaBuild before using its executable. Its `open`
operation now takes `BRANCH TITLE BODY`, so every PR describes the actual trial.
The helper loads the sandbox App's credentials at runtime. Never open, print or
copy those credentials. It posts TL3's check, not a human code-owner review.

Use `gh api --method GET` to inspect repository trees and commits. For a benign
trial change, create a new tree and commit through REST, retaining the original
main parent. Use the App helper's `branch tl3/NAME SHA` to publish that commit.
Use its `open BRANCH TITLE BODY`, `approve SHA`, `queue NUMBER SHA`, and
`dequeue NUMBER` operations. Never post TL3 approval on Mergify's draft.

## Record a successful queue merge

1. Create a benign PR that changes no gate files. Record its approved head.
2. Let `Sandbox tests` pass on that head. Post TL3 approval for that exact SHA.
3. Record the original before queueing with `trial.py record ORIGINAL before`.
4. Queue the original by label. Use REST checks and Mergify's comment to identify
   its temporary draft. Record that draft once its `Sandbox tests` passes.
5. After Mergify merges the original, record it again in `after`.
6. Run `trial.py verify before draft after`. Retain its output and all recordings
   under `deploy/sandbox/evidence/successful-queue/`.

The command paths from the repository root are:

```sh
python3 deploy/sandbox/trial.py record PR_NUMBER NEW_RECORDING_DIRECTORY
python3 deploy/sandbox/trial.py verify BEFORE_DIRECTORY DRAFT_DIRECTORY AFTER_DIRECTORY
```

`record` uses the existing GET-only Mergify recorder and adds `main.json` before
reading the PR. Each destination must be new. Invoke it at each observed state;
there is no timed polling loop. Inspect checks on both the original and draft,
and retain Mergify's comments linking the original to that draft. The verifier
checks the selected recordings; it does not establish that association itself.

## Record main advancing before the original merges

1. Create two independent benign PRs, A and B, from the same main SHA.
2. Pass CI and approve both exact heads. Leave A out of the queue and record A
   as `before`. This keeps the ordering deterministic without timers.
3. Queue B, record its draft and successful merge, and confirm that main moved.
4. Record A as `advanced` while A remains open. Its head must still equal the
   approved head. This snapshot must precede A's merge.
5. Queue A. Record its successful temporary draft and merged original as above.
6. Run `trial.py verify before draft after --advanced advanced`. Retain A's
   snapshots and B's merge evidence under `evidence/main-advancement/`.

The verifier requires successful CI and exact-head approval on the original,
successful CI on a separate Mergify draft, a completed successful Mergify queue
check titled `Merged via merge queue`, and a merged original with unchanged head.
For advancement, it also requires main to change while that original is still
unmerged. The newest check from the required App on that head wins.

## Finish the evidence

Replay the successful original's `pull.json`, `head-checks.json`, and
`merge-checks.json` through the production controller adapter. Add a separate
positive replay test expecting `checks_passed: true`. Keep the bootstrap negative
replay. Run the adapter tests on all three fleet hosts through PrismaBuild.

Capture actual webhook deliveries if available. REST snapshots cannot substitute
for an `in_progress` check-run delivery or a merged `closed` delivery. Label any
missing delivery evidence explicitly. Close throwaway PRs and remove their
branches through the App after recording results. Preserve the seed issues.
