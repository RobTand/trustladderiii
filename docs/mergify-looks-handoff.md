# Mergify progress handoff for PR #69

PR #69 merged during the review of #80. This controller integration now supplies
the allowance mapping described below. Its upstream adapter supplies `Completion.progress: Option<ProgressToken>` and
`ProgressToken::Mergify(mergify::Progress)`. The token contains `merged_at`,
`merge_commit_sha`, and the selected queue check's `id`, `status`, `conclusion`
and `title`. Compare all fields with typed equality. No timestamp, request ID or
other controller-generated value belongs in the token.

A `Found` completion with a progress token is an authoritative live record even
when `observations` is empty and the enqueue effect is already confirmed. Map
that token to `Look::Alive` before either empty-observation early return in the
pending-effect or watch reconciliation path. A completion with terminal
observations still delivers them once through the existing inbox keys.

Use PR #69's watch-row `WatchProgress` as the sole durable previous-token store.
For each read, take the existing `current` as `previous`, put the read token in
`current`, and call `changed()`. The first token has no previous value and sets
the baseline. An identical read settles `previous == current`; it does not renew
progress time. Later identical reads write nothing. A changed token records the
new progress time and renews the allowance. Compare before overwriting the row.

The look path and the ordinary reconciliation path must call the same helper.
It returns the transient `Look::Alive` value and whether this read changed the
token. Commit the watch comparison and the allowance's progress time and due
time together. For Mergify, do not also persist a token string in
`AllowanceState.progress`, `AllowanceState.found`, or a second table. The
watch owns the token; the allowance owns only scheduling time and incident state.
Restarts load these rows without treating the old token as fresh progress.

Only new authoritative tokens renew allowances. Recording an unchanged token
never schedules an immediate look, releases a held write or counts as recovery.
Use the existing single `recv_timeout(next_look_in())` timer. At the due allowance,
read Mergify once. If the token still equals the baseline past the no-progress
allowance, send `Fact::UpstreamStalled`. The kernel already handles it in Queued,
ignores it outside Queued or while an incident is open, and requires Rob for later
incidents. No kernel change is needed.

Keep the structured token available to the incident writer. For example, say
"GitHub merged the change at X, but Mergify check Y is still in progress."
Include the request title and link. An unreadable upstream is an outage and
re-arms the allowance. It never proves the enqueue absent or the merge complete.
The default probe returns false. The Mergify adapter must implement an actual upstream health
probe before first Mergify incidents can clear automatically.

Retiring a finished watch removes its `WatchProgress` and allowance together.
No historical-token scan or timed source is needed. On a token-only read, keep
the watch active until terminal observations or kernel operation retirement.

Verify the integration with a fake clock and these cases:

- An already-confirmed enqueue returns `Found` with no observations and a token.
  It sets the baseline and eventually raises an incident if unchanged.
- A change to any token field renews the allowance once. Identical reads do not.
- Restart preserves the baseline, progress time and due time.
- First incidents continue allowance-driven probes; later ones need Rob.
- Terminal evidence retires the watch and allowance, leaving no timer.

Run those tests through PrismaBuild on dl380g10, sparky and sparklina because
queue-source integration also exercises process and timing boundaries.
