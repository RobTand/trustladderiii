# PrismaBuild test tool

`run_tests(tests=None)` runs the current worktree's pytest suite. `tests` can
name relative files, directories, or node IDs. Omit it or pass an empty list
for the whole suite. Options, absolute paths, parent traversal and Git metadata
paths are refused. There is no command or shell argument.

## Maker integration

The maker declares `run_tests` in its role's tool descriptions. Its input schema
carries test settings. Before building its tools, bind those settings:

```python
from tl3_workers.pb_tests import TestSettings

worktree.test_settings = TestSettings(
    image=inputs["tests"]["image"],
    tools_path=inputs["tests"]["tools_path"],
)
```

Alternatively, `make_test_tool(worktree, settings)` returns the callable directly.
`TestSettings` also accepts positive `cpus`, `mem_gb`, and `quiet_seconds`, with
defaults of 1, 2, and 120. Deployment installs `tl3-harness` alongside the workers.
The trusted installed harness must be outside the role's worktree. No field is
added to protocol v1. Ticket #38 wires this binding into the maker's runtime.
A missing binding returns an explicit infrastructure error without submitting.

The factory copies current regular files through the confined worktree handle,
including uncommitted and ignored files. It omits symlinks, special files, hard
links, and `.git`, matching file tool confinement. Concurrent mutation is refused
when a file becomes unsafe to open. The agent calls tools serially; the maker
must keep other writers out while freezing input.

The tool stages the trusted installed harness outside that copy. Three fixed
Git commands create a private snapshot repository without source hooks or Git
configuration. One detached `pbrun.py` action declares the image, CPU and memory
demand, native thread limits, portable placement and a `tests` progress phase.
It has no total execution timeout. `pbwait.py --endings --until cleaned` waits
for verified ending and cleanup evidence. An expired observation allowance
reattaches to the same action.

The admitted action reuses the check harness's `container_command`, admission
check, progress writer and report plugin. The checkout and harness mounts are
read-only. Tests get no network, fleet mount, host socket, inherited credentials,
or PB progress token. The summary has the harness's existing in-process report
custody: test code shares the reporter's interpreter. This is not a tamper-proof
check verdict.

## Results

Results contain `status`, `tests`, `totals`, `note`, and, after submission,
`action_key`. Each retained test has its name, `outcome` and failure `message`.
Outcomes are `passed`, `failed` for assertions, `error` for setup, teardown,
collection or runtime errors, and `skipped`. Xfail and xpass count as skipped,
as in the check harness. Collection failures name the collector.

The JSON result is at most 16,384 bytes using Python's default JSON encoding.
Messages are clipped to 1,000 characters. Entries that exceed the remaining
budget are omitted. A plain truncation note explains either cut; totals include
all reported tests. An empty suite reports zero totals and an explicit note.
Submission, containment, missing reports and unverifiable PB evidence return
`status="infrastructure_error"` with zero test totals, never a failed test.

## Verification

From a clean commit, submit the worker and harness suites through PrismaBuild:

```sh
python3 /mnt/shared/prismabuild-fleet/repo/tools/pbrun.py \
  --cwd "$PWD" --anywhere --cpus 2 --demand mem_gb=4 --priority -10 \
  --env PATH=/home/rob/.local/bin:/usr/local/bin:/usr/bin:/bin \
  --env OMP_NUM_THREADS=1 --env MKL_NUM_THREADS=1 --env OPENBLAS_NUM_THREADS=1 \
  --env UV_CONCURRENT_DOWNLOADS=2 --env UV_CONCURRENT_BUILDS=2 --detach -- \
  /home/rob/.local/bin/uv run --project workers --locked --no-editable \
  --python 3.14+gil python workers/tools/verify_tests.py
```

Use `pbwait.py --endings --until cleaned ACTION_KEY` and inspect verified logs
and receipt evidence. The suite uses fake `pbrun.py` and `pbwait.py`; it never
submits recursively. The runner tests execute fixture pytest directly inside
that admitted action.

For a real trial, install both packages and run `workers/tools/trial_tests.py
IMAGE_DIGEST` from the submitting session, outside any admitted action. It calls
the actual factory on a tiny uncommitted worktree and submits one action. Its
summary must show one containment test passed, one deliberate assertion failed,
and one test skipped. The script returns nonzero for any other result.
