# Clear the unserved prerequisite incidents from #810

The 80 setup incidents in the 2026-11-05 report are irrelevant prerequisite
updates from `RobTand/trustladderiii`. That repository is the operations
destination, outside the served request repositories. The journal contains 80
distinct observation hashes, each with
`merge prerequisite fact is outside served repositories`.
Reconstructing the serialized observations matches every hash to installation
`176903725`: 36 completed issue facts and 44 merged pull request facts.
The input paths are `issues` updates with a completed closure and
`pull_request.closed` with a merge. Check-run and comment events do not produce
these facts. The journal does not distinguish an issue closure from a later
edit to an already closed issue.

No replay is needed. These facts cannot satisfy a same-repository prerequisite
for any served request. The delivery's other translations run independently.
The fix filters new unserved prerequisite updates and consumes previously
queued ones without an incident. It preserves operations comments and served
repository dependency facts. A served update without an issue or pull request
number raises `prerequisite_number`, with a message naming the missing number.

After the coordinator deploys the fix, the TL3 lead runs this preview from the
fixed checkout:

```sh
python3 tools/recover_setup_incidents.py --config "$HOME/.config/tl3/tl3.toml"
```

Inspect the list. It contains only still-open IDs from the original 80, and it
refuses an ID whose summary changed. To clear that list as Rob:

```sh
python3 tools/recover_setup_incidents.py --config "$HOME/.config/tl3/tl3.toml" --apply
```

The helper posts one `clear incident ID` comment per incident to the configured
operations issue, `RobTand/trustladderiii#81`. It requires the GitHub CLI to be
signed in as `RobTand`, waits 1.1 seconds between writes, and stops on any API
error. It reads status through the installed CLI and uses GitHub REST only.
It does not deploy, connect to Postgres, replay inputs, or edit configuration.
Rerunning skips incidents that status no longer lists. Repeated clear commands
for the same immutable incident ID do not replay the original observation.

Once the daemon handles the comments, run the preview again. It should report
zero diagnosed incidents still open. Inspect any new incident separately.
