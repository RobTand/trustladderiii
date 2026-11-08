# Test tool

`run_tests(tests=None)` runs the current worktree's pytest suite. `tests` can
name relative files, directories, or node IDs. Omit it or pass an empty list
for the whole suite. Options, absolute paths, parent traversal and Git metadata
paths are refused. There is no command or shell argument.

## Maker integration

The maker declares `run_tests` in its role's tool descriptions. Its input schema
carries test settings. Before building its tools, bind those settings:

```python
from tl3_workers.tests_tool import TestSettings

worktree.test_settings = TestSettings(**inputs["tests"])
```

`inputs["tests"]` holds the immutable image digest and `execution`, the
controller's selected Flux profile: `{"backend": "flux", "profile": <absolute
per-attempt execution.json>}`. Settings without that profile are refused.
`make_test_tool(worktree, settings)` returns the callable directly.
`TestSettings` also accepts positive `cpus`, `mem_gb`, and `quiet_seconds`, with
defaults of 1, 2, and 120. Deployment installs `tl3-harness` alongside the workers.
The trusted installed harness must be outside the role's worktree.
A missing binding returns an explicit infrastructure error without submitting.

The factory copies current regular files through the confined worktree handle,
including uncommitted and ignored files. It omits symlinks, special files, hard
links, and `.git`, matching file tool confinement. Concurrent mutation is refused
when a file becomes unsafe to open. The agent calls tools serially; the maker
must keep other writers out while freezing input.

The tool stages the trusted installed harness outside that copy. Three fixed
Git commands create a private snapshot repository without source hooks or Git
configuration. `flux_execution` then activates the profile, allocates the input
root, and prepares one run through the Rust worker-execution control. The run
declares the image, CPU and memory demand, parallel workers and walltime; the
backend derives native thread limits from them. The tool checkpoints the run's
wait reference before submitting it. A later call with the same staged input,
selection and execution contract reattaches to that run; a changed contract
fails closed instead of submitting again.

The admitted run reuses the check harness's `container_command`, progress
writer and report plugin. The checkout and harness mounts are read-only. Both
check mode and the test tool pass `--rootdir=.` to pytest, so node IDs are
rooted at the checkout, including tests in nested projects. Tests get no
network, fleet mount, host socket or inherited credentials. The summary has the
harness's existing in-process report custody: test code shares the reporter's
interpreter. This is not a tamper-proof check verdict.

## Results

Results contain `status`, `tests`, `totals`, `note`, and, after submission,
`wait_reference`. Completed results carry one native run receipt:
`backend`, `wait_reference`, `run_binding`, `receipt_sha256` and the run's
`ending`. Rust releases a payload only for a verified completed report, and the
worker checks each payload blob's size and digest before reading it.
Each retained test has its name, `outcome` and failure `message`.
Outcomes are `passed`, `failed` for assertions, `error` for setup, teardown,
collection or runtime errors, and `skipped`. Xfail and xpass count as skipped,
as in the check harness. Collection failures name the collector.
Missing selector paths are reported before pytest starts. Pytest exit 4 reports
"Selector not found" with the requested selectors. Both return completed results
with `error` outcomes, so a bad selector does not trigger an infrastructure retry.

The JSON result is at most 16,384 bytes using Python's default JSON encoding.
Messages are clipped to 1,000 characters. Entries that exceed the remaining
budget are omitted. A plain truncation note explains either cut; totals include
all reported tests. An empty suite reports zero totals and an explicit note.
Admission, an unknown observation, a run that ends without a completed report,
and unverifiable payload evidence return `status="infrastructure_error"` with
zero test totals, never a failed test.

## Verification

The worker suites run as Bazel tests (`//workers/tests:test_pb_tests` and its
neighbours). They replace only the Flux control and CLI wire with a fixture
(`pb_fixtures.FluxFake`); the worker's payload checks and receipt mapping run,
and fixture jobs execute the real staged action or runner. They never submit
recursively. Run narrow targets on BuildBuddy, as
[the Bazel guide](../docs/bazel.md#running-tests) describes.
