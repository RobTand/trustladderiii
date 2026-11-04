# Transient Mergify dequeue retries: Q27 decision needed

Status: proposed, not approved or implemented. This is the stop-and-report
outcome requested in [#786](https://github.com/RobTand/trustladderiii/issues/786).

## Why this needs a rule decision

Increasing the adapter's one-command budget does not itself require a kernel
change. The requirement that a transient dequeue **never raises an incident
before its retry bound**, however, needs an exception to the existing stall
rule. Implementing that exception in the controller would still change a rule.

The current sources say:

- [State table, Looks and stalls](../state-table.md#looks-and-stalls):
  "Mergify and GitHub: the stall raises an incident."
  Progress means "Checks finishing, or the queue moving."
- [Queued](../state-table.md#queued): a GitHub or Mergify stall, with no incident
  already open, raises one. A probe or Rob clears the first; only Rob clears
  later incidents.
- [Failures, Q7](../state-table.md#failures): "Every second failure raises an
  incident, and the operation waits for \"incident cleared\" before it retries."
  Delivery failures follow the same rule, counted per effect.
- [Q27](../state-machine.md): "A rule change needs Rob's approval, including a
  rule that comes from a bug Kani or a test finds. A clarification doesn't."

There is a concrete conflict even without deciding whether every transient
dequeue is a counted Q7 infrastructure failure. Consider a confirmed transient
dequeue followed by a 60-minute backoff, with no later upstream progress. The
default Mergify no-progress allowance is 30 minutes. Before the backoff ends,
the existing rule requires a stall incident. #786 requires no incident before
the bounded retry total is exhausted. Config permits any positive allowance,
so choosing shorter backoffs does not establish that guarantee either.

Changing the diagnostic's attempt count or advancing the local timer cannot
stand in for upstream progress. Extending the allowance or suppressing stall
handling until the retry bound changes when an incident is required.

## Existing implementation and timer

These locations are on the stacked branch after merging `origin/rust-fv` at
`d6df228c4b0eec7decbc0c6510da75e4cfb9cdd9`:

- `crates/config/src/lib.rs:638`: the default GitHub/Mergify no-progress
  allowance is 1,800 seconds. Validation at line 1132 permits positive values.
- `crates/controller/src/looks.rs:817`: `Look::Alive` compares the progress
  checkpoint and preserves failed-look counts after the allowance expires.
- `crates/controller/src/looks.rs:982`: `look_alive` emits
  `Fact::UpstreamStalled` for GitHub/Mergify after no progress. For a pending
  enqueue effect it also enters failed-look incident handling.
- `crates/kernel/src/lib.rs:2548`: a stall in Queued raises the existing
  incident, with later incidents requiring Rob. In Publishing this fact does
  nothing; the pending-effect failed-look path still applies.
- `crates/controller/src/looks.rs:461`: `next_look_in` selects the earliest
  durable allowance `due_ms`. `crates/controller/src/ingress.rs` waits on that
  timer and calls `look_due`; a retry must reuse this mechanism.
- `crates/controller/src/adapters/mergify/waiting_requeue.rs:371`: recovery
  grace uses a persisted timestamp and subsequent reads through the existing
  look mechanism. It does not exempt Mergify from stall incidents.
- `crates/controller/src/looks.rs:882`: `Look::Quota` persists a reset time
  and schedules the next look. #759's short model-unavailable probe uses this
  model-quota path. Q53 explicitly exempts model quota from failure counting;
  it grants no equivalent exception for Mergify dequeues.

No new sleep, poll loop, kernel fact, or kernel transition is needed to
implement the proposed policy. Approval is needed for its behavior, regardless
of which module implements it.

## Proposed narrow exception for approval

Treat an authoritative, typed transient Mergify dequeue as an internal retry
of the same enqueue operation, in Publishing or Queued. During its bounded
backoff and command reconciliation, do not count that dequeue as a Q7 failure
or raise the Mergify no-progress incident solely because it is waiting on this
retry. Preserve incidents caused by independent failures to read upstream.

Limit the exception to verified machine codes for `BranchIndexingDelay` and
`DraftPrCreationFailed`. Preserve today's behavior for checks failures,
conflicts, person-dequeues, and `Unknown`. In particular, prose that resembles
a transient reason must remain `Unknown`.

Proposed retry policy, to make the decision concrete:

- Allow at most six automatic requeue commands per repository, PR, and head,
  separate from the existing durable command budget. Persist the attempt
  reservation and unique command key before dispatch. A send with an unknown
  outcome counts as reserved and must be reconciled before another command.
- Use backoffs of 5, 15, 30, 60, 60, and 60 minutes. Persist the deadline and
  schedule it through allowance `due_ms` and the existing ingress timer.
  Restart or an early webhook must not shorten or restart the backoff.
- Count a new attempt only after a distinct, authoritative transient dequeue
  of the preceding queue attempt. Repeated reads of one dequeue consume no
  additional attempts. Active, merged, or unresolved attempts do not establish
  another failure.
- If the sixth requeue is authoritatively dequeued for a transient reason,
  raise one exhaustion incident through the existing incident mechanism.
  Repeated looks and restarts must deduplicate it. Do not reset the exhausted
  ledger on incident clear unless that reset is explicitly approved.
- Retain the typed reason, attempts reserved, deadline, and upstream attempt
  identity in diagnostics. Head changes get a separate ledger and must still
  satisfy the existing approval, pause, human-hold, and epoch fences.

The decision needed is whether to approve this bounded Mergify exception to
Q7 and the no-progress rule. Without that exception, the implementation must
retain earlier stall incidents and cannot satisfy the requested invariant.

## Classification research, 2026-11-04

Mergify documents structured dequeue codes in its REST activity log. Typed-only
classification is possible through that API; it is not possible from PB #2794's
observed GitHub dequeue check alone. Whether the historical PB event actually
carries a non-null code remains unverified: the unauthenticated activity-log GET
returned HTTP 403 with `{"detail":"Forbidden"}`. No credentials were read or
forwarded to Mergify, and no authenticated Mergify request was attempted.

### Exact machine fields and values

The primary sources are the [activity-log reference](https://docs.mergify.com/api/activity-log/)
and its [OpenAPI 3.1 document](https://docs.mergify.com/openapi.json).
The REST base is `https://api.mergify.com/v1`.

| Event discriminator | Exact JSON path | Schema |
| --- | --- | --- |
| `type = "action.queue.leave"` | `events[].metadata.dequeue_code` | `EventQueueLeaveMetadata.properties.dequeue_code` |
| `type = "action.queue.checks_end"` | `events[].metadata.abort_code` | `EventQueueChecksEndMetadata.properties.abort_code` |

Both fields explicitly enumerate these exact uppercase wire values:

| Failure | Exact code | Documented rule-context spelling |
| --- | --- | --- |
| GitHub has not indexed the new branch | `DRAFT_PULL_REQUEST_CREATION_BRANCH_NOT_INDEXED` | `draft-pull-request-creation-branch-not-indexed` |
| Draft PR creation failed | `DRAFT_PULL_REQUEST_CREATION_FAILED` | `draft-pull-request-creation-failed` |

The [data-types reference](https://docs.mergify.com/configuration/data-types/#queue-dequeue-reason)
describes the first as "Mergify could not create the merge queue draft pull request
because GitHub had not yet indexed the new branch." It describes the second as
"Mergify could not create the merge queue draft pull request."
The [conditions reference](https://docs.mergify.com/configuration/conditions/#pull-request-attributes-queue-dequeue-reason)
exposes these lowercase spellings through `queue-dequeue-reason`, "The code
identifying why a pull request's merge queue checks ended". These are documented
codes, not names inferred from display text. The documentation does not promise
that every generic draft-PR creation failure is transient; that policy remains
part of the pending decision.

`dequeue_code` and `abort_code` are nullable. The leave event also carries
`pull_request_head_sha`, nullable `session_id`, nullable `batch_id`, `merged`,
`queue_name`, `branch`, and `queued_at`. The checks-end event carries `aborted`,
`abort_status` with values `DEFINITIVE` or `REEMBARKED`, and session/batch/run
identity. Its `abort_reason` and the leave event's `reason` are documented as
Markdown display text, including legacy plain-text fallback. They are not the
code fields. A checks-end event alone need not mean the PR left the queue;
correlate the leave record to the exact PR/head/session before retrying.
Missing, null, stale, or unrecognized codes cannot establish a transient class.

### REST retrieval and access limit

Use `GET /repos/{owner}/{repository}/logs`, with `pull_request`, repeated
`event_type`, `received_from`, `received_to`, and `per_page` filters. Follow the
response's RFC 5988 `Link` pagination links. The documented defaults cover only
a one-day window, so use explicit timestamps when recovering old history.
The following unauthenticated GET was made once and returned the 403 above:

```text
https://api.mergify.com/v1/repos/RobTand/prismabuild/logs?pull_request=2794&event_type=action.queue.leave&event_type=action.queue.checks_end&received_from=2026-11-04T10%3A00%3A00Z&received_to=2026-11-04T10%3A30%3A00Z&per_page=100
```

The [authentication reference](https://docs.mergify.com/api/usage/#authentication)
and endpoint schema allow an admin-scoped application key or Mergify user token.
GitHub user tokens remain accepted but are deprecated as Mergify credentials;
a GitHub App installation token is not listed as that user-token scheme.
This research neither provisions credentials nor tests TL3's current credential
compatibility. An authorized authenticated GET is still needed to capture the
actual PB #2794 leave/checks-end events and confirm historical code availability.

The old `/repos/{owner}/{repository}/events` and
`/repos/{owner}/{repository}/pulls/{pull}/events` endpoints were
[deprecated in 2023](https://docs.mergify.com/changelog/2023-10-31-events-api-deprecation/)
in favor of `/logs`. Do not build against those old paths.

### Contract fixture and live check fixture

This fixture is extracted verbatim from the two `QueueChecksOutcome.properties`
definitions in the downloaded OpenAPI document. It pins the documented codes
and meanings. It is a schema excerpt, not a captured PB activity-log event:

```json
{
  "DRAFT_PULL_REQUEST_CREATION_BRANCH_NOT_INDEXED": {
    "type": "integer",
    "title": "Draft Pull Request Creation Branch Not Indexed",
    "description": "The draft pull request cannot be created because GitHub has not indexed the branch yet",
    "default": 0
  },
  "DRAFT_PULL_REQUEST_CREATION_FAILED": {
    "type": "integer",
    "title": "Draft Pull Request Creation Failed",
    "description": "The draft pull request cannot be created",
    "default": 0
  }
}
```

For either code above, the documented classification input is the projection
`{"type":"action.queue.leave","metadata":{"dequeue_code":CODE}}`, or
`{"type":"action.queue.checks_end","metadata":{"abort_code":CODE}}`.
These are schema-derived projections, not complete event fixtures. The complete
live event fixture remains unavailable because of the 403; no fabricated event
is presented as observed evidence.

This second fixture is an actual response projection from
[PB queue check 126517983421](https://api.github.com/repos/RobTand/prismabuild/check-runs/126517983421).
The title and entire summary are verbatim JSON string values, with newlines
escaped by JSON serialization:

```json
{
  "id": 126517983421,
  "name": "Mergify Merge Queue",
  "head_sha": "1dcd11d4d75771a0f249b8feca4f4e042d92eb53",
  "external_id": "queue:2794",
  "conclusion": "neutral",
  "output": {
    "title": "Dequeued — draft pull request creation failed due to GitHub branch indexing delay",
    "summary": "### 🚫 Left the queue · draft pull request creation failed due to GitHub branch indexing delay\n\n7 min in the queue, with no time running CI.\n\n**So far**\n\n- ✅ **Entered the queue** Wed 4 Nov, 10:19 · rule `default` · triggered by the [`@mergifyio queue` command](https://github.com/RobTand/prismabuild/pull/2794#issuecomment-6628278843)\n- 🚫 **Left the queue** 10:26 · `1dcd11d`\n\n**Reason**\n\nThe draft pull request cannot be created\n\n**Hint**\n\nGitHub is currently delayed in indexing new branches, preventing the pull request from being created.\nRetrying right away may not resolve the issue.\nThe problem is temporary and typically clears once GitHub catches up.\nIf you want to requeue this pull request, you can post a `@mergifyio queue` comment.\n\n<sub>Times in UTC · [dashboard](https://dashboard.mergify.com/orgs/RobTand/repos/prismabuild/queues/status?branch=main&pull=2794) · [how the merge queue works](https://docs.mergify.com/merge-queue/)</sub>",
    "text": null,
    "annotations_count": 0,
    "annotations_url": "https://api.github.com/repos/RobTand/prismabuild/check-runs/126517983421/annotations"
  }
}
```

The [annotations GET](https://api.github.com/repos/RobTand/prismabuild/check-runs/126517983421/annotations)
returned `[]`, matching `annotations_count = 0`. `output.text` is null. The
summary contains neither a `mergify.v1` span nor a JSON block or abort code.
Its queue-rule check, `126509983401`, previously returned the exact title
"The pull request has been removed from the queue `default`" and summary
"The draft pull request cannot be created.\n\nYou can take a look at `Mergify Merge Queue` check runs for more details about the failure.\n".
That generic rule report does not distinguish branch indexing from other
creation failures. No separate real generic draft-creation-failure queue-check
title/summary was captured; do not invent or approve one based on this report.

`GET /repos/RobTand/prismabuild/commits/1dcd11d4/check-runs?filter=all&per_page=100`
reported 30 runs. Earlier Mergify engine check `126507535242` on this head does
contain a `mergify.v1` JSON payload, but its decoded state is `waiting`, not a
dequeue. Its existence does not supply typed evidence for the later dequeue.
An additional page had zero runs.

### Other interfaces checked

- The [current queue APIs](https://docs.mergify.com/api/merge-queue/) expose live
  batches and queued PRs, including a pending manual `dequeue_requested` flag.
  The documented live PR/status schemas do not expose the historical
  `dequeue_code`; use the activity log for an exited attempt.
- The [statistics API](https://docs.mergify.com/api/statistics/) documents both
  draft-creation codes as outcome counters. The
  `/stats/queues_pr_exit_reasons` endpoint groups `exit_reason` counts by time.
  These aggregates cannot bind a reason to PB #2794's exact head or session.
- [GitHub's `check_run` webhook](https://docs.github.com/en/webhooks/webhook-events-and-payloads#check_run)
  contains the check-run object. No separate Mergify dequeue code is documented
  in that envelope. The actual check output and annotations inspected above
  supply none. Historical webhook deliveries were not read.
- The Mergify OpenAPI document has no top-level outbound `webhooks` definitions.
  The [published page index](https://docs.mergify.com/sitemap-0.xml) and
  [Slack integration documentation](https://docs.mergify.com/integrations/slack/)
  did not identify a generic outbound dequeue-reason webhook contract. This is
  a limit of the documentation search, not proof that none exists.

All network research used GETs, sequentially, with at least about one second
between requests. Initial bare documentation requests returned 403; the official
pages and schema were accessible with a normal browser User-Agent. The Mergify
API 403 above was not retried with credentials. No API writes, live changes,
implementation, tests, or proofs were performed.

## Verification after approval

Write and execute the three requested adapter regressions before production
changes: backoff then successful requeue without an incident, exhaustion with
one incident, and unchanged checks-failed handling. Extend the existing frozen
reference differential I/O schedules to allow divergence only for confirmed
transient classes, including crashes at each write and unresolved sends.

Add Kani step proofs for the retry bound, escalation boundary and deduplication,
unchanged command budget, and nontransient equivalence against an independent
oracle. Each needs covers and an intended-assertion mutant. The timer integration
also needs a deterministic test that spans the ordinary no-progress deadline,
an early webhook, and a restart.

No production files, state-table rows, Q-rules, tests, or runtime state changed
for this note. No tests or proofs were run: implementation stopped at the Q27
boundary. No BuildBuddy result is claimed for #786.
