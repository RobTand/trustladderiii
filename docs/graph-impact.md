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

`tools/ci/graph-impact-allowlist.json` maps exception keys to nonempty
reasons or bounded crate entries. Keys are `reach:<test-label>`, `edge:<group>:<consumer> -> <dependency>`, `figure:u_pct`, `figure:loc_u_pct`,
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

A `crate:<Cargo-package-name>` entry credits only the exact Bazel package in
`head['crate_packages'][name]`, the same ownership mapping used for its source
count. Subpackages and other packages receive no credit. For example:

```json
{
  "crate:example-core": {
    "reason": "Extract verified decisions for #730.",
    "sources": 10,
    "max_fanout": 66
  }
}
```

`sources` is N, the exact number of production source labels in that package at
head, including unused sources. It excuses gained reach to those sources for
per-test budgets and structural reach. A stale N fails and asks for an update in
the same change. N can change only when the source count changes. Reasons remain
immutable.

`max_fanout` is the exact absolute maximum at head. For each production source in
the package, count every runnable head test whose transitive reach includes it,
then take the largest count. Unused sources count as zero; an empty source set
has maximum zero. Compile and runtime data consumers count, each test once.
This is `max(head['dependents'][source])` over the package's source labels.
The existing structural fanout check counts only tests present at both base and
head. Restricting the absolute bound to that population would change its meaning
when a later PR advances the base. The bound therefore includes new head tests,
so an unchanged head graph has the same absolute value against any base.

Every crate entry needs a nonnegative integer `max_fanout`; booleans, fractions,
strings, and missing values fail. Lower and higher stale values both fail with
the measured value and "update max_fanout in the same change", even with zero
fanout growth. Update it in every slice that changes the measured maximum.
An exact entry excuses existing-source fanout growth only for sources in its
own package with a validated N. Other existing sources still fail under the
same consumer-count and added-edge rules. Source move identities retain their
original labels for structural comparison. A rename within the same package
keeps its validated reach and fanout credits at that original identity. A move
across packages cannot transfer those credits. SCC, edge-group, and forbidden-edge
checks use the full graph.

The TL3 adapter matches source moves using the typed edge diff and SHA-256 of
the file bytes read at each revision. A match needs a removed rule-to-source
edge, an added rule-to-source edge, and exactly one production source with that
content hash in each revision's full source universe. Surviving and unused
duplicates make the identity ambiguous before move candidates are selected.
The origin must disappear from the head source universe. Copies, edited files,
ambiguous matches, and files without source-edge evidence receive no move match.
Exact content identity avoids filename guesses and Git rename similarity
thresholds.

For comparison only, the adapter maps each matched destination's reach and
fan-out back to its origin label. Base package ownership remains unchanged, so
a move credits only that file. Unrelated additions in the destination package
cannot inherit a fabricated base peer. Unmatched new sources still count as
growth when other sources disappear, unless they inherit an original base
package's fan-out or have a reach exception. Additional consumers of a moved
file still count as fan-out growth unless an exact crate entry credits a rename
within the same package. Comparison-only identity paths connect the
original labels to actual added edges for diagnostics. The exported graphs,
SCCs, edge diffs, and U figures remain unchanged.
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
