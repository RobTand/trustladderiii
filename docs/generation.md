# Configured generation and the merge queue

A repository may configure a fixed generator command and generated output globs:

```toml
generated_files = ["gen/**/*.json"]
generator = "python3 tools/gen_manifest.py"
```

The leaf's full declared scope must include every output the command changes.
Generated outputs remain subject to scope authorization and protected paths.
The gate can grant slots to leaves with disjoint authored files even when their
configured outputs overlap (#845, Q27, Rob, 2026-11-06).

TL3 runs the existing admitted `run_generator` executor after a clean rebase,
commits its authorized outputs, then checks and reviews the resulting head before
publishing. Checking independently regenerates an isolated snapshot of that exact
head. Changed outputs or generator failure fail the check and reach maker repair.
Repair messages name stale outputs; makers repair the inputs and call
`run_generator`, without editing generated files by hand.

**A repository with a generator must require a CI check that regenerates and
diffs the exact tree Mergify will merge.** Checking an original PR head alone is
insufficient: two independently fresh leaves can merge cleanly into a stale
combined tree. The required check must fail for changed, deleted or untracked
generated outputs and for any generator error. For example, a check can run:

```sh
python3 tools/gen_manifest.py
git diff --exit-code -- gen/
test -z "$(git ls-files --others --exclude-standard -- gen/)"
```

Run this check on Mergify's queue checkout and make it required in the repository's
queue configuration. TL3 does not execute a separate queue-tree generator check.
Required-check settings are read from GitHub/Mergify; the local repository config
has no required-check name or queue-trigger declaration to validate this setting.

A queue CI failure follows the existing failed-round and maker-repair rules.
Before dispatching that repair in a generator repository, TL3 updates the branch
onto the latest base and regenerates through the existing executor. A conflicted
update leaves the original branch intact and preserves the existing maker route;
a later publication conflict reaches Rebasing. Repositories without a configured
generator retain their existing repair behavior. Checking verifies the maker's
result again before it can publish.
