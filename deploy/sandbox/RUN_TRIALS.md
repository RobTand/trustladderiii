# Run the sandbox queue trials

Phase A prepares these commands; it does not run them. Run phase B only after
the coordinator merges the sandbox configuration PR adding
`merge_bot_account: RobTand`. Keep `branch_protection_injection_mode: none`.
Sandbox PR #28 has installed CI on fleet runners. No successful queue merge
under the new configuration is claimed here.

Rob told the coordinator on 2026-10-30, “Everything that you want approved is
approved.” The coordinator explicitly applies that consent to Mergify merging
`RobTand/tl3-sandbox` PRs as `RobTand`. The scope is the sandbox only. If Mergify
reports that the account must sign in to its dashboard, retain that failure and
hand it to the coordinator to ask Rob. Do not change rulesets or bypass checks.

## Set up admitted commands

Run from this repository’s clean, reviewed checkout on dl380g10. This host is
required for its existing App and `gh` runtime; never read, print, or copy their
credentials. Every agent trial command, recorder, and verifier below goes
through PrismaBuild. CI runs directly on its fleet runners. Do not submit CI
to PrismaBuild or submit recursively inside an admitted action.

Use Bash for the commands below. The fresh directory holds only non-secret
request bodies, SHAs, recordings, and build output. Keep it for the handoff.

```bash
set -euo pipefail
trial_root=$(mktemp -d /home/rob/tmp/tl3-queue-trials-XXXXXXXX)
trial_id=${trial_root##*/}
pb() {
  python3 /mnt/shared/prismabuild-fleet/repo/tools/pbrun.py \
    --cwd "$PWD" --tag dl380g10 --priority -10 --cpus 1 --demand mem_gb=1 \
    --env PATH=/home/rob/.cargo/bin:/usr/local/bin:/usr/bin:/bin \
    --env TRIAL_ROOT="$trial_root" --env TRIAL_ID="$trial_id" \
    --env SANDBOX_GATE="$trial_root/target/debug/examples/sandbox_gate" \
    --env OMP_NUM_THREADS=1 --env MKL_NUM_THREADS=1 --env OPENBLAS_NUM_THREADS=1 \
    -- "$@"
}
python3 /mnt/shared/prismabuild-fleet/repo/tools/pbrun.py \
  --cwd "$PWD" --tag dl380g10 --priority -10 --cpus 2 --demand mem_gb=6 \
  --env PATH=/home/rob/.cargo/bin:/usr/local/bin:/usr/bin:/bin \
  --env RUSTUP_HOME=/home/rob/.rustup --env CARGO_HOME=/home/rob/.cargo \
  --env CARGO_TARGET_DIR="$trial_root/target" --env CARGO_BUILD_JOBS=2 \
  --env RUST_TEST_THREADS=2 --env OMP_NUM_THREADS=1 \
  --env MKL_NUM_THREADS=1 --env OPENBLAS_NUM_THREADS=1 \
  -- cargo build --locked -p tl3-mergify --example sandbox_gate
pb python3 deploy/sandbox/check_settings.py
```

Retain every action key. After each submission, check its terminal status,
logs, and CAS receipt before proceeding. A submission acknowledgement is not a
pass. Use `pbwait.py ACTION_KEY --json` to recover a terminal result. Reserve
aggregate resources and preserve PB’s CPU affinity. Record skips and missing
tools. If PB is unavailable, diagnose and repair it; do not execute locally.

## Open two throwaway App PRs

These commands create both benign commits and PRs as TL3’s App. `gh api` only
reads metadata. In particular, **tree and commit POSTs use the App installation
token**, through `sandbox_gate tree REQUEST.json` and
`sandbox_gate commit REQUEST.json`. Never substitute `gh api --method POST`
for those writes: that would attribute the objects to Rob’s login.

```bash
open_trial() {
  pb bash -euo pipefail -c '
  kind=$1
  case_dir="$TRIAL_ROOT/$kind"
  mkdir "$case_dir"
  gh api --method GET repos/RobTand/tl3-sandbox/commits/main > "$case_dir/main.json"
  python3 - "$case_dir" "$TRIAL_ID" "$kind" <<"PYBODY"
import json, sys
from pathlib import Path
root, trial_id, kind = Path(sys.argv[1]), sys.argv[2], sys.argv[3]
main = json.loads((root / "main.json").read_text())
(root / "parent").write_text(main["sha"])
(root / "tree-request.json").write_text(json.dumps({
    "base_tree": main["commit"]["tree"]["sha"],
    "tree": [{"path": f"trials/{trial_id}-{kind}.txt", "mode": "100644",
              "type": "blob", "content": f"Sandbox queue trial: {trial_id} {kind}\n"}],
}))
PYBODY
  "$SANDBOX_GATE" tree "$case_dir/tree-request.json" > "$case_dir/tree"
  python3 - "$case_dir" "$kind" <<"PYBODY"
import json, sys
from pathlib import Path
root, kind = Path(sys.argv[1]), sys.argv[2]
(root / "commit-request.json").write_text(json.dumps({
    "message": f"Sandbox queue trial: {kind}",
    "tree": (root / "tree").read_text().strip(),
    "parents": [(root / "parent").read_text().strip()],
}))
PYBODY
  "$SANDBOX_GATE" commit "$case_dir/commit-request.json" > "$case_dir/head"
  branch="tl3/trial-$TRIAL_ID-$kind"
  printf "%s\n" "$branch" > "$case_dir/branch"
  "$SANDBOX_GATE" branch "$branch" "$(cat "$case_dir/head")"
  "$SANDBOX_GATE" open "$branch" "Sandbox queue trial: $kind" \
    "Throwaway phase B $kind trial. Changes only a trials/ text file. No gate files change."
  gh api --method GET repos/RobTand/tl3-sandbox/pulls \
    -f state=open -f "head=RobTand:$branch" \
    --jq ".[] | .number" > "$case_dir/number"
  test "$(wc -l < "$case_dir/number")" -eq 1
  gh api --method GET "repos/RobTand/tl3-sandbox/pulls/$(cat "$case_dir/number")" \
    --jq "{number,user:.user.login,head:.head.sha,branch:.head.ref,merged}"
' create-trial "$1"
}
open_trial approved
```

Link the returned PR with `link_pull_request` as the next tool call. Then run:

```bash
open_trial refusal
```

Link that returned PR as the next tool call too. Both authors must be `tl3-robtand[bot]`, and both branches must begin
`tl3/trial-`. Read each created
PR’s files through `gh api` to confirm that only its `trials/` file changes.

## Queue the approved PR

First observe successful `Sandbox tests` on the original head. Then run:

```bash
pb bash -euo pipefail -c '
case_dir="$TRIAL_ROOT/approved"
number=$(cat "$case_dir/number")
head=$(cat "$case_dir/head")
"$SANDBOX_GATE" approve "$head"
python3 deploy/sandbox/trial.py record "$number" "$case_dir/before"
"$SANDBOX_GATE" queue "$number" "$head"
'
```

Never approve Mergify’s temporary draft. On the queue event, inspect the
original’s Mergify comment and identify its separate draft number:

```bash
pb bash -euo pipefail -c '
number=$(cat "$TRIAL_ROOT/approved/number")
gh api --method GET "repos/RobTand/tl3-sandbox/issues/$number/comments" \
  --paginate --jq ".[] | select(.user.login == \"mergify[bot]\") | .body"
'
```

Set `draft_number` to the number linked by that comment. Once the draft’s
`Sandbox tests` completes successfully, record it; after the merge event,
record the original. These are separate commands at observed events, without
a polling or sleep loop.

```bash
draft_number=REPLACE_WITH_LINKED_DRAFT_NUMBER
pb python3 deploy/sandbox/trial.py record "$draft_number" "$trial_root/approved/draft"
pb bash -euo pipefail -c '
python3 deploy/sandbox/trial.py record \
  "$(cat "$TRIAL_ROOT/approved/number")" "$TRIAL_ROOT/approved/after"
python3 deploy/sandbox/trial.py verify \
  "$TRIAL_ROOT/approved/before" "$TRIAL_ROOT/approved/draft" "$TRIAL_ROOT/approved/after"
'
pb gh api --method GET "repos/RobTand/tl3-sandbox/actions/runs?event=pull_request&per_page=100" \
  --jq ".workflow_runs[] | {id,head_sha,status,conclusion,check_suite_id}"
```

Match the recorded original and draft check suites to their workflow runs.
Set each observed run ID below and retain the jobs response:

```bash
original_run=REPLACE_WITH_ORIGINAL_RUN_ID
draft_run=REPLACE_WITH_DRAFT_RUN_ID
pb bash -euo pipefail -c '
for run in "$@"; do
  gh api --method GET "repos/RobTand/tl3-sandbox/actions/runs/$run/jobs" \
    --paginate > "$TRIAL_ROOT/approved/jobs-$run.json"
done
' capture-jobs "$original_run" "$draft_run"
```

Require successful `Sandbox tests` jobs with `self-hosted`, `linux`, `x64`,
`tl3-sandbox-fleet` labels and a `tl3-sandbox-` runner name. A hosted runner or
an unstarted job does not pass. The verifier must report an unchanged approved
head, separate successful draft CI, and `Merged via merge queue` from Mergify.
If it reports dashboard authorization is missing, stop this trial and hand off
the Mergify check output; do not retry with another merge account.

## Label the refusal PR without approval

Do not call `approve` for this head. `queue` below only applies `tl3-queue`
after checking PR identity and head; Mergify must refuse enqueueing it.

```bash
pb bash -euo pipefail -c '
case_dir="$TRIAL_ROOT/refusal"
number=$(cat "$case_dir/number")
head=$(cat "$case_dir/head")
python3 deploy/sandbox/trial.py record "$number" "$case_dir/before"
"$SANDBOX_GATE" queue "$number" "$head"
'
```

After its original CI completes and Mergify evaluates the label, record the
refusal and inspect the complete checks and comments:

```bash
pb bash -euo pipefail -c '
case_dir="$TRIAL_ROOT/refusal"
python3 deploy/sandbox/trial.py record "$(cat "$case_dir/number")" "$case_dir/after"
python3 - "$case_dir/after" <<"PYBODY"
import json, sys
from pathlib import Path
root = Path(sys.argv[1])
pull = json.loads((root / "pull.json").read_text())
checks = json.loads((root / "head-checks.json").read_text())
events = json.loads((root / "events.json").read_text())
comments = json.loads((root / "comments.json").read_text())
assert pull["state"] == "open" and not pull["merged"], pull
assert any(e["event"] == "labeled" and (e.get("label") or {}).get("name") == "tl3-queue" for e in events)
assert not any(c["name"] == "TL3 approval" and (c.get("app") or {}).get("id") == 5542171 for c in checks)
ci = [c for c in checks if c["name"] == "Sandbox tests" and (c.get("app") or {}).get("id") == 15368]
assert ci and max(ci, key=lambda c: c["id"])["conclusion"] == "success", ci
queue = [c for c in checks if c["name"] == "Mergify Merge Queue" and (c.get("app") or {}).get("id") == 10562]
assert not queue, queue
print(json.dumps({"checks": checks, "mergify_comments": comments}, indent=2))
print("Snapshot: labelled, green CI, no App approval, no queue check, still open.")
PYBODY
'
```

The assertions are a snapshot, not proof from elapsed time. Require Mergify’s
rule evaluation to show the missing App approval and no enqueue/draft for this
PR. If Mergify has not evaluated the label, mark the result incomplete. Retain
the rule check output and comments with that evidence. Do not attempt a direct
merge or mark the trial passed merely because the PR is still open.

After recording the refusal, remove its queue label with the App:

```bash
pb bash -euo pipefail -c '
"$SANDBOX_GATE" dequeue "$(cat "$TRIAL_ROOT/refusal/number")"
'
```

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
