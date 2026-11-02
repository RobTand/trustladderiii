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
Maximum SCC size and forbidden-edge limits still fail the check. The baseline
writer refuses those losses and preserves the global budget's minimum.

New test-to-test rule dependencies and new conftest dependencies on production
code fail. This uses declared Bazel dependencies. An import missing from BUILD
declarations is outside this check's evidence; generated-input checks still apply.

The empty `tools/ci/graph-impact-allowlist.json` maps exception keys to nonempty
reasons. Keys are `reach:<test-label>`, `edge:<group>:<consumer> -> <dependency>`, `figure:u_pct`, `figure:loc_u_pct`,
or `scc:<sorted,comma-separated-members>`. Compared with base, entries may only
be deleted. Reasons cannot change. The initial empty list permits no exceptions.

Run the check on BuildBuddy, using full commit SHAs:

```sh
bb remote --timeout=10m --jobs=16 --run_from_commit="$HEAD_SHA" \
  --container_image="docker://gcr.io/flame-public/rbe-ubuntu24-04@sha256:5f0c01e5f6d79e58d7a996330de33aa38ce599bf8431eaf27521e7602daadf50" \
  --script="python3 tools/ci/graph_impact_check.py --base $BASE_SHA --head $HEAD_SHA"
```

Add `--write-baseline` inside the remote command to record gains. Fetch the
`graph-impact-baseline.json` BuildBuddy artifact and commit it. The artifact
`graph-impact-result.json` includes full metrics, added edges, fan-out growth,
and up to five blame subgraphs. The gate preserves both revisions' hashes and
dependency exports. bazel-diff v70.0.2 `explain --format mermaid` produces the blame
subgraphs, which the GitHub job puts in its summary.

`tools/ci/graph_impact_proof.py --range PR:BASE:HEAD` compares historical ranges.
It generates independent hash pairs with v42 and v70.0.2 and fails if their impacted
rule sets differ. It plants broad reach, forbidden dependencies, existing-source
expansion, and a cycle into the exported graph and checks each failure. It also
checks that a new sibling test, three new hub sources, and a U improvement pass.
Run it with `bb remote --timeout=10m --git_fetch_depth=0 --script='...'`.

`graph_impact_replay.py --harvest graph-impact-replay-ranges.json` exports the last
30 merged ranges using queries, sharing immutable snapshots at consecutive
revisions. Its compressed BuildBuddy artifact can be replayed with `--graphs`.
The replay records old and revised rule outcomes, every failure reason, package
ceilings, and typed edge differences in `graph-impact-replay.json`.
Historical proofs reject production source labels absent from the requested
revision's Git tree. This catches untracked proof files entering Bazel data globs.
The recorded [30-range replay](graph-impact-replay.md) lists each failure reason.

The crate cut report removes one real crate or crate-source edge at a time and
counts lost reach nodes across affected tests. These are candidates for narrowing
inputs or splitting crates, not instructions to delete a required dependency.
It does not estimate runtime savings or whether a cut preserves behavior.

`tools/ci/graph_impact.py` is vendored byte-identically from PrismaQuant. Its
commit and SHA-256 are recorded in `graph-impact-core.json` and checked by a
test. TL3's adapter owns source classification, the supplemental source edges,
conftest rule dependencies, and crate reports. Cross-repo imports are unnecessary.
