# Sandbox gate trials, 2026-10-30

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
| TL3 branch ownership | Human creation of `tl3/issue-17-node-999` returned 422. The original record omitted the actor; a follow-up PrismaBuild probe records the authenticated actor and exact request. Creation by integration 5542171 succeeded in the original trial | `human-branch-denied.json`, issue #17's trial comment |
| Draft checks | #16 kept its approved head while Mergify tested draft #18 on a separate branch | `queued/`, `draft/`, `draft-cleanup.json` |
| Label removal | Removing the label dequeued #16. REST event 34696807438 confirms `mergify[bot]` closed draft #18 | `dequeued/` |
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
