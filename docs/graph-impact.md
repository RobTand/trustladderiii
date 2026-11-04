# Check graph impact

Every PR gate compares bazel-diff's `deps.json` exports at base and head, including
PRs whose test selection falls back to the full suite. The check needs no run
history. Violations fail both required language checks through the gate's input
checks. A missing export, baseline, or source label fails the gate.

`generate-hashes --depEdgesFile` supplies the graph. Its typed hash export identifies
source labels. v70.0.2 records these labels as keys but omits rule-to-source
edges. The adapter detects missing incoming source edges and runs one streamed
query over the first-party test closure per revision, adding only the missing
rule-to-source edges. Rule-to-rule edges remain bazel-diff's. The unmodified
exports still drive test selection. Other label queries identify runnable tests
and Rust libraries. Tests tagged `manual` are excluded as
roots. Their binary targets remain in the closure of the Rust test wrappers.

The TL3 source universe consists of first-party `.rs` and `.py` source-file labels.
It excludes test files, conftests, fixtures, and testdata directories. Unused
production sources count with fan-out zero. Reach includes compile and runtime
data edges, exactly as bazel-diff's static graph does. External libraries, generated
files, and non-code data carry dependency paths but are not counted as production
source files.

The log reports these figures:

- U is the mean source fan-out divided by the number of runnable tests. It estimates
  the fraction of tests selected by editing one uniformly chosen production file.
- LOC U weights each production file by its physical line count.
- Reach reports median, nearest-rank p90, and maximum production source counts.
- SCC reports cycles in the exported graph, with their members. The crate report
  separately checks direct Rust library dependencies for crate cycles.
- Crate fan-out counts every test whose closure includes that Rust library or its
  `sources` filegroup. This includes tests that read crate sources as data.

New and changed tests use the maximum base reach of sibling tests in their Bazel
package. For Rust this is the crate. An edited test may keep its own base reach.
New packages fall back to the baseline's global p90 budget. A test counts as
changed when its direct rule hash changes or a test source in its package changes.
The failure names extra sources and their dependency path.

U and LOC U are informational. The baseline preserves their minimum values without
failing ordinary population growth. Structural checks compare existing sources'
fan-out among existing tests and existing tests' reach. Added edges that grow
these counts fail and appear in the diagnostic. New tests do not inflate the
structural fan-out comparison. New sources may inherit existing package fan-out;
the summary reports them. The adapter supplies each test's package, package base
maximum reach, and typed added and removed rule-to-rule and rule-to-source edges.
SCC growth and forbidden edges use only the actual base-to-head edge diff.
The baseline stores numeric figures, tolerances, and the reach budget. It stores
no edge lists; exceptions live in the separate allowlist. For the shared core's
absolute edge check, the adapter supplies an in-memory list from base's edges.
Its component comparison detects SCC growth or mergers, including growth hidden
by a larger existing cycle. The baseline writer preserves numeric minima and
refuses an increased maximum SCC size.

New test-to-test rule dependencies and new conftest dependencies on production
code fail. This uses declared Bazel dependencies. An import missing from BUILD
declarations is outside this check's evidence; generated-input checks still apply.
Edges already present at base cannot become new because a stored baseline is old.
The gate uses Mergify's queue-aware base commit unchanged, so earlier PRs in a
batch belong to the base graph. It does not replace that commit with main or the
PR's original merge base.

The empty `tools/ci/graph-impact-allowlist.json` maps exception keys to nonempty
reasons. Keys are `reach:<test-label>`, `edge:<group>:<consumer> -> <dependency>`, `figure:u_pct`, `figure:loc_u_pct`,
or `scc:<sorted,comma-separated-members>`. New entries must name a program issue
with `#730` syntax or a full TL3 GitHub issue URL. For example:

```json
{
  "reach://crates/controller:test_contracts": "Extract contracts into a crate for #730."
}
```

The Gate summary prints every entry added relative to base, with its reason,
even when another check fails. Existing reasons cannot change. Entries can be
deleted. A `reach:` entry excuses both the test's package reach budget and its
structural reach growth. It does not excuse existing-source fan-out growth,
cycles, or forbidden edges. Those checks keep their existing rules.

The TL3 adapter matches source moves using the typed edge diff and SHA-256 of
the file bytes read at each revision. A match needs a removed rule-to-source
edge, an added rule-to-source edge, and exactly one removed and one added
production source with that content hash. The origin must disappear from the
head source universe. Copies, edited files, ambiguous matches, and files without
source-edge evidence receive no move match. Exact content identity avoids
filename guesses and Git rename similarity thresholds.

For comparison only, the adapter maps each origin's base reach and fan-out to
the destination label. A move therefore carries its previous consumers instead
of receiving a new-file exemption. Additional consumers still count as fan-out
growth. The exported graphs, SCCs, edge diffs, and U figures remain unchanged.
The summary lists each matched move, and `source_moves` in the result maps
destinations to origins.

CI runs the check on BuildBuddy through the in-repo Run API client,
[`tools/ci/remote_gate.py`](../tools/ci/remote_gate.py). For an agent run on
dl380g10, use the external Run API helper described in
[Running tests](bazel.md#running-tests), in `step` mode with this remote
command and full commit SHAs:

```sh
python3 tools/ci/graph_impact_check.py --base "$BASE_SHA" --head "$HEAD_SHA"
```

Add `--write-baseline` inside the remote command to record gains. Fetch the
`graph-impact-baseline.json` BuildBuddy artifact and commit it. The artifact
`graph-impact-result.json` includes full metrics, added edges, fan-out growth,
and up to five blame subgraphs. The gate preserves both revisions' hashes and
dependency exports. bazel-diff v70.0.2 `explain --format mermaid` produces the blame
subgraphs, which the GitHub job puts in its summary.

The recorded [30-range replay](graph-impact-replay.md) lists each failure reason.
The scripts that produced it are no longer in the tree.

The crate cut report removes one real crate or crate-source edge at a time and
counts lost reach nodes across affected tests. These are candidates for narrowing
inputs or splitting crates, not instructions to delete a required dependency.
It does not estimate runtime savings or whether a cut preserves behavior.

`tools/ci/graph_impact.py` is vendored byte-identically from PrismaQuant. Its
commit and SHA-256 are recorded in `graph-impact-core.json` and checked by a
test. TL3's adapter owns source classification, the supplemental source edges,
conftest rule dependencies, crate reports, reasoned exceptions, and move identity.
This Bazel export adapter stays in Python alongside the pinned Python core;
it does not add a second graph implementation or change the shared core.
Cross-repo imports are unnecessary.
