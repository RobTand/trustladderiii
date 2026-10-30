# Sandbox gate trials, 2026-10-30

The settings checker passes. The no-approval merge trial passed. Successful queue
merges remain blocked on Rob's code-owner approval of sandbox PR #23, head
`09e6d50219649223b8f106060b9311d9e0666bdb`.

The sandbox's files were deployed through PR #15. Its CI passed before the
coordinator-authorized bootstrap merge
`8901bccfca30f1297c1c6312e6a239ec3b669d76`. Mergify reads configuration from the
default branch. No trustladderiii PR was merged.

| Trial | Observed result | Recording in `evidence/` |
|---|---|---|
| Mergify installation | Its App posted a queue check on #14 | `activation/ready/` |
| Missing approval | #20 had green CI and the queue label. REST merge returned 405, naming only the missing TL3 approval check | `unapproved/`, `ci-green-approval-missing.json` |
| TL3 branch ownership | Human creation returned 422. Creation of the same branch by integration 5542171 succeeded | `human-branch-denied.json`, issue #17's trial comment |
| Draft checks | #16 kept its approved head while Mergify tested draft #18 on a separate branch | `queued/`, `draft/`, `draft-cleanup.json` |
| Label removal | Removing the label dequeued #16 and closed draft #18 | `dequeued/` |
| Check placement and head matching | Queue checks appeared on the original head. Drafts carried Actions CI and Mergify protections, with no queue check | `queued/`, `blocked-original/`, `blocked-successful-draft/` |
| Queued title | `In merge queue`, followed by `Running merge queue checks` | `queued/`, `approval-injected-on-draft/` |
| Failed draft CI | Original #21 passed CI. Draft #24 failed. The queue reported `Dequeued — checks failed` | `failed/`, `failed-draft/` |
| Workflow timeout | Draft #26 exceeded its 10-minute job limit. GitHub reported `cancelled`; Mergify reported `Dequeued — checks interrupted` | `timeout/`, `timeout-draft/`, including annotations |
| Outside-queue merge evidence | Bootstrap merge commit had no checks. The production adapter replay reports `checks_passed: false` | `bootstrap/`, controller test `sandbox_gate` |
| Successful queue merge evidence | Blocked on #23. No merged queue payload or `checks_passed: true` claim | Not observed |
| Original head while main advances | Blocked on #23. No trial merged into main after bootstrap | Not observed |
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
passed. Code-owner review is the remaining requirement.

A temporary split-ruleset experiment did not avoid the injection. It was reverted.
The final live main ruleset has no bypass actors. The only bypass actor on TL3
branches is its own App. The committed baseline matches these final settings.

The coordinator permits one recovery merge of #23 after Rob personally approves
that exact head and every required GitHub check passes. The merge must use REST,
a merge commit, and normal GitHub enforcement. The implementation agent has not
submitted a review using Rob's token. Every later gate-file change must use the
gate. The approved local config deliberately remains the deployed version until
#23 merges.

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
The nullable-title regression failed before the fix with exit 101, then passed.
That client change was required to post approval through TL3's own client:
Actions returns `output.title: null`, including on a successful check.

Throwaway original PRs #14, #16, #17, #20, #21 and #25 are closed and their branches
are deleted. Mergify drafts #18, #19, #22, #24 and #26 are closed and their branches
are deleted. Throwaway issue #27 is closed. The bootstrap setup branch is deleted.
Recovery PR #23 and its branch remain for Rob's review. The twelve seed issues
were not changed.
