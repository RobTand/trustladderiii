# Sandbox gate trials, 2026-10-30

## Phase B queue trials

Sandbox PR #29 merged as `1179d3fccfdc9fccc9b08efd11615f373ae3d5fc`.
The live settings checker passes with `merge_bot_account: RobTand` and
`branch_protection_injection_mode: none`. The trials used a clean detached
worktree of `t17-sandbox-merge-account` at `8bf5e74229edecf105e1a4a529e0f42183b6b706`.
Every agent trial command and verifier ran through PrismaBuild. Sandbox CI
ran directly on its fleet runners.

The App created both Git trees, commits, branches and PRs. Each PR changed only
one `trials/` text file. No gate file changed.

- **Success:** [sandbox #30](https://github.com/RobTand/tl3-sandbox/pull/30)
  entered the queue automatically after exact-head App approval and the
  `tl3-queue` label. Mergify tested separate draft
  [#32](https://github.com/RobTand/tl3-sandbox/pull/32), then merged #30 at
  `668fd489e9798d64fb1c0a96295d2fd5c9ef6205`. GitHub records merger `RobTand`.
  Mergify records `Merged via merge queue` and identifies draft #32 as passed.
  The original head stayed `1ee8d4c881c7350e9c75551bd03ea9dcd5d6bab1`.
- **Draft CI:** `Sandbox tests`, job `124326120949`, succeeded on
  `tl3-sandbox-dl380g10-slot-1-1793396093-18022`. Its labels were `self-hosted`,
  `linux`, `x64`, and `tl3-sandbox-fleet`. Original CI also succeeded on a sandbox
  fleet runner. TL3 never approved the draft.
- **Refusal:** [sandbox #31](https://github.com/RobTand/tl3-sandbox/pull/31)
  had green CI and the queue label, with no App approval. Mergify check
  `124324781836` reported `Waiting for queue conditions to match`. The label
  and base matched; App approval was the only unmet queue-rule condition.
  Its recorded payload says `waiting`, with null `queued_at` and
  `speculative_check_pr`. There was no Mergify queue comment or draft.
  The App removed the label, closed the PR and deleted its branch.

Recordings are under [`evidence/queue-trials/`](evidence/queue-trials/).
The standard recorder retains its existing field allowlist. Supplementary REST
selections retain only changed-file metadata, merger identity, workflow/run/job
identity, runner labels/name and completion timestamps. No headers or
credentials are evidence. `refusal/condition-report.json` preserves the complete
queue condition output and its decoded Mergify state.

The positive production-adapter replay expects `checks_passed: true` from #30's
recorded pull, head checks and merge checks. The separate bootstrap replay still
expects `false`. The host results and every trial action, exit code, log hash and
verified CAS receipt/payload hash are in `evidence/queue-trials/prismabuild.json`.
The supplementary jobs recorder failed once because shell quoting removed a
JSON key's quotes. The corrected recorder passed; both actions remain recorded.
This was a recording error, not a sandbox CI failure.

This phase covers the requested success and refusal trials. The separate
main-advancement trial remains unrun, so #114 remains **Part of #17**. The App's
delivery history still contains only the old ping with HTTP 502. Actual
in-progress check-run and merged closed webhook deliveries remain unobserved;
REST snapshots do not replace them. The twelve seed issues were preserved.

## Earlier sandbox trials

The approved copy now includes sandbox PR #23, merged by the coordinator as
`f9a76b6a9fc62518540d80cd989ad3d0bd35591e` after Rob's approval. The live
GET-only settings checker passes at this follow-up. The no-approval merge trial
passed in the original run. Successful queue merges and main advancement are
blocked on sandbox CI: no self-hosted runners were registered at inspection,
and the workflow still targets GitHub-hosted `ubuntu-24.04`.

The sandbox's files were deployed through PR #15. Its CI passed before the
coordinator-authorized bootstrap merge
`8901bccfca30f1297c1c6312e6a239ec3b669d76`. Mergify reads configuration from the
default branch. No trustladderiii PR was merged.

| Trial | Observed result | Recording in `evidence/` |
|---|---|---|
| Mergify installation | Its App posted a queue check on #14 | `activation/ready/` |
| Missing approval | #20 had green CI and the queue label. REST merge returned 405, naming only the missing TL3 approval check | `unapproved/`, `ci-green-approval-missing.json` |
| TL3 branch ownership | Human creation of `tl3/issue-17-node-999` returned 422. The original record omitted the actor; a follow-up PrismaBuild probe records actor `RobTand` and the exact request. Creation by integration 5542171 succeeded in the original trial | `human-branch-denied.json`, `post-recovery/human-branch-denied.json`, issue #17's trial comment |
| Draft checks | #16 kept its approved head while Mergify tested draft #18 on a separate branch | `queued/`, `draft/`, `draft-cleanup.json` |
| Label removal | Removing the label dequeued #16. REST event 34696807438 confirms `mergify[bot]` closed draft #18 | `dequeued/`, `post-recovery/draft-18-events.json` |
| Check placement and head matching | Queue checks appeared on the original head. Drafts carried Actions CI and Mergify protections, with no queue check | `queued/`, `blocked-original/`, `blocked-successful-draft/` |
| Queued title | `In merge queue`, followed by `Running merge queue checks` | `queued/`, `approval-injected-on-draft/` |
| Failed draft CI | Original #21 passed CI. Draft #24 failed. The queue reported `Dequeued — checks failed` | `failed/`, `failed-draft/` |
| Workflow timeout | Draft #26 exceeded its 10-minute job limit. GitHub reported `cancelled`; Mergify reported `Dequeued — checks interrupted` | `timeout/`, `timeout-draft/`, including annotations |
| Outside-queue merge evidence | Bootstrap merge commit had no checks. The production adapter replay reports `checks_passed: false` | `bootstrap/`, controller test `sandbox_gate` |
| Successful queue merge evidence | Blocked on sandbox CI. No merged queue payload or `checks_passed: true` claim | Not observed |
| Original head while main advances | Blocked on sandbox CI. Recovery #23 is merged, but no main-advancement trial ran | Not observed |
| In-progress webhook | REST queue status was `in_progress`. Delivery of that webhook was not observed | `queued/`, `webhook-deliveries/` |
| Closed-event actor | Throwaway issue #27 was closed by the App. REST recorded a non-null actor. This does not rule out null actors in other events | `closed-issue-actors.json` |

The App's delivery history contained only a failed ping, with status 502. No
check-run or pull-request delivery payload was available. REST PR snapshots and
issue events are labelled as REST evidence; no closed webhook payload was
reconstructed from them.

## Configuration deadlock and recovery

The first main ruleset requires both original-head approval and CI. Mergify's
default branch-protection injection copies the approval requirement into draft
merge conditions. TL3 intentionally never approves the draft. Both original
and draft CI pass, but the queue cannot merge. The required setting from the
client review, approval in `queue_conditions`, does not by itself prevent this
extra requirement from being injected.

PR #23 adds `branch_protection_injection_mode: none`. Explicit queue conditions
still require approval on the original head and the label. Explicit merge
conditions still require draft CI. GitHub still enforces approval and CI on the
original PR. The configuration is valid, CI passed, and TL3's exact-head check
passed. Rob approved the exact head. The coordinator recorded his approval and merged it with normal GitHub enforcement, as issue comment 6531998384 records.

A temporary split-ruleset experiment did not avoid the injection. It was reverted.
The final live main ruleset has no bypass actors. The only bypass actor on TL3
branches is its own App. The committed baseline matches these final settings.

The recovery merge is complete. `files/.mergify.yml` and its approved hash now
match the file at that merge commit. The follow-up uses no ruleset bypass and
submits no review with Rob's account. `evidence/post-recovery/` retains the live
file response, runner inventory, workflow response, and draft #18's REST events.
[Run the remaining trials](RUN_TRIALS.md) describes the prepared recorder,
verifier, ordering, and the remaining positive adapter replay.

The live timeout result also exposed a client follow-up: its current title-only
fallback does not recognize `Dequeued — checks interrupted` or
`Dequeued — pull request dequeued`. Those selected checks remain `Unknown` when
no versioned payload is present. The recordings preserve the actual titles and
summaries for the client owner; this change does not invent a failure policy.
The adapter's separate issue-event path still records person removals.

## Verification and cleanup

`evidence/prismabuild.json` records action keys, exit codes, log hashes and verified
receipt hashes. All tests ran through PrismaBuild. The live GitHub trials ran
from the session as authorized.

Formatting, Clippy, 59 client tests, 38 controller scenarios and 17 checker tests
passed on dl380g10, sparky and sparklina. A second action on each host passed the
recorded bootstrap adapter replay and all 17 checker tests. There were no skips.
The nullable-title regression failed before the fix. PrismaBuild reported exit 1, wrapping cargo's exit 101; it then passed.
That client change was required to post approval through TL3's own client:
Actions returns `output.title: null`, including on a successful check.

Throwaway original PRs #14, #16, #17, #20, #21 and #25 are closed and their branches
are deleted. Mergify drafts #18, #19, #22, #24 and #26 are closed and their branches
are deleted. Throwaway issue #27 is closed. The bootstrap setup branch is deleted.
Recovery PR #23 subsequently merged after Rob's review. The twelve seed issues
were not changed.

## Follow-up verification

The red commit adds a failing check against the real post-recovery Mergify file,
a stub-driven trial-description test, and offline trial-verifier cases. The
controller replay's redundant `expected` argument was removed without weakening
its `checks_passed: false` assertion. The checker implementation is unchanged.
The trial verifier's fixtures are synthetic unit inputs, not live trial evidence.

Follow-up action keys, terminal exit codes, log hashes, and receipt hashes are in
`evidence/prismabuild-followup.json`. No successful-merge or main-advancement
result is claimed until sandbox CI runs. Webhook observations remain unobserved.

All three follow-up host suites passed formatting, Clippy, 59 client tests, the
caller-description example test, 38 Mergify scenarios, the bootstrap adapter
replay, and all 28 Python tests. No tests were skipped. The live GET-only checker
and repeated human-branch probe also exited 0. Their terminal records, logs,
receipts and CAS payload hashes were verified. The expected red action exited 1.
