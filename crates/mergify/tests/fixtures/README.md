# Recorded Mergify responses

Recorded 2026-10-30 with `tools/record.py`, using GitHub REST GET requests only.
`source.json` records each repository, PR number, and head. The recorder selects
fields used by `tl3-github` and keeps the bot's full check output and comment body.
PR authors were added with GET requests from the same sources on 2026-10-30.
It records all Mergify check names, including unrelated checks, so filtering runs
against actual responses. It does not record tokens or authentication material.

| Directory | Source | Reports |
| --- | --- | --- |
| `merged` | RobTand/prismabuild PR 2408 | Merged, in-place checks |
| `waiting` | RobTand/prismabuild PR 2425 | Ready for queue entry |
| `running` | matrixorigin/matrixone PR 31723 | Running in-place checks |
| `failed` | frappe/erpnext PR 62987 | CI failure and earlier manual dequeue |
| `conflict` | altendky/carl PR 84 | Base branch conflict |

No live `In merge queue` title was found in the open public PRs inspected in
`matrixorigin/matrixone` and `frappe/erpnext` on 2026-10-30. The queued-title test
therefore remains synthetic: it changes the recorded running title to the
lifecycle documentation's `In merge queue`. Metadata edge cases are synthetic variants of
recorded schemas. They cover future versions, unknown states, timeout, interruption,
and deferred checks. These variants are not claimed as live recordings.

To record another PR, choose a fresh destination:

```bash
python3 crates/mergify/tools/record.py OWNER/REPO NUMBER /tmp/new-mergify-recording
```

Queue state and comments may change while recording. Inspect the result before
adding it as a fixture. Tests never invoke the recorder or access GitHub.

`throwaway-key.pem` is a fixed, disposable RSA test key generated for this fake
server. It has never been installed in a GitHub App. Do not use it for credentials.
