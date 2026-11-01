# Pytest check harness

This package runs a check plan on a base commit and a head commit in one
PrismaBuild action. Install it outside the served checkout. The controller
adapter is a separate ticket.

## Interface

The trusted outer process calls one function:

```python
from pathlib import Path
from tl3_harness import Check, Container, Kind, run_checks

results = run_checks(
    Path("/served-repo"),
    base_commit,
    head_commit,
    (
        Check("new behavior", Kind.DEMONSTRATING, ("tests/test_value.py::test_value",)),
        Check("existing behavior", Kind.REGRESSION, ("tests/test_other.py::test_other",)),
    ),
    container=Container(image_digest),
)
```

The process must already be inside an admitted PrismaBuild action. Declare
the image with `--container-image`, a `tests` progress phase with an allowance
appropriate to the suite, CPU and memory demand, and native thread limits.
Do not set a total-duration timeout. Use `--anywhere`. The image must already
be local on an eligible worker. Python 3.14 and pytest must be in the image.
The trusted outer process needs Python 3.14 and Git.

`run_checks` returns immutable `Result` objects. Each contains the check's name,
kind, verdict, and each named test's base outcome, head outcome, pre-existing
failure flag, pass flag, and an optional failure message capped at 1,000 characters.
Older result readers can ignore the additive `message` field. The controller sends
at most 20 failing tests back to a maker. `execution.result_json` serializes these objects.
Every result also records its report custody. Today that is always `in-process`:
pytest's reporting plugin runs in the tests' own interpreter. Slice one accepts
this. Rob decided on 2026-10-30 that hostile code isn't a concern at this point.
Tamper-proof custody needs PB #2403.
The verdict is `pass`, `fail`, or `infrastructure-failure`. Invalid inputs or
changed test configuration raise `ValueError`. A named test outside the
configured test files returns a failing check result and fails the round. Missing admission raises
`RuntimeError`. Git failures retain their subprocess error.

The served repository supplies this protected configuration in `pyproject.toml`:

```toml
[tool.tl3.harness]
command = ["python", "-m", "pytest"]
test-files = ["tests/**/*.py"]
```

Slice one accepts `python -m pytest` and `python3 -m pytest`. The trusted runner
invokes the installed pytest module. Pytest reads its repository settings as
usual. `test-files` uses Python's `PurePath.full_match` patterns. These patterns
identify all test edits to apply to the demonstrating base. They must exclude
production files. The harness rejects changes to pytest configuration,
`pyproject.toml`, tox, nox, and any `conftest.py` file. It never copies
`conftest.py` into the demonstrating base. The caller must keep its installed
harness and the served checkout separate.

Test selectors name pytest functions or methods, optionally with parameter IDs.
A bare function or method selects all its parametrized cases, as pytest does.
Its outcome is passed only when every case passes, otherwise the first failing
case's outcome in collection order. No matching cases means uncollected.
Regression and behavior-preserving checks must name functions defined at the
base. The planner checks those definitions statically. The harness fails the
round if the head edits or deletes a named existing function, including its
decorators, and tells the maker to restore it.
The harness collects the named files and selects the named items itself. It
disables automatic third-party plugin loading. Dependencies and any plugins
explicitly required by repository settings belong in the trusted image.

## Verdict rules

Demonstrating checks run the head's test edits against the base production
code. Every named test must fail there with a call-phase assertion and pass
on the head. A setup assertion does not count. The state table also permits
a missing name added by the change. The harness recognizes a call-phase
module attribute error, an import of a new module, or an import of a function
or class the round adds. A module-level import failure counts too. Collection
errors apply only to tests in that file. Setup errors and unrelated import
failures cannot demonstrate a change.

Regression checks run against the original base and the head. A base failure
is recorded as pre-existing. It can excuse a head failure, but cannot excuse
a skipped, missing, or uncollected test. A passing head also cannot excuse an
unavailable base test. Behavior-preserving checks require passes on both trees.

Reproducing checks use `Kind.REPRODUCING` and require the `reviewed` commit
argument to `run_checks`. They run the fix round's test edits against that
commit's production code, then require the tests to pass on the fixed head.
Their failure rules match demonstrating checks. Setup errors, runtime errors,
skips, missing tests and uncollected tests fail. In a reproducing result, `base`
records the reviewed tree's outcome. Other check kinds still use the original
base. After a clean rebase, the controller replays the reviewed state onto the new
base and retains it under an owned ref. The before tree uses that rebased
state, including upstream test configuration, with only the round's test edits.
If that replay conflicts, the controller drops the reproducing checks and
supplies a reason for the next review to judge. Ordinary checks still run.

The original base results are cached by base commit, image and selected tests
within the admitted action's trusted process. No untrusted cache enters a
container. Cache lifetime does not cross actions. A controller can retain
verified base evidence when its adapter is implemented.

The reporting plugin uses pytest's phase reports and exception type. It does
not parse terminal messages. Xfail and xpass supply no passing evidence.
Missing, malformed, duplicate, inconsistent, or incomplete reports produce an
infrastructure verdict. A collection error with a complete report is a test
error. A demonstrating base setup or collection error fails its check.

## Execution boundary

The outer process reads committed Git blobs, ignoring dirty files and Git
archive attributes. It rejects symlinks and submodules. It exports the original
base and the head. Demonstrating checks get another base export with only test
edits applied. All these runs share one admitted action.

Each tree runs in a container launched with the `docker` command on PB's PATH.
The harness refuses to use an unmanaged Docker executable. PB's shim supplies
the action's CPU affinity, memory scope, ownership and cleanup. The harness
adds no CPU mask. It declares only two read-only bind mounts, the exported tree
and the installed harness. The container has no network, fleet mount, host
socket, inherited host environment, or writable host directory. It runs as
UID and GID 65534, drops capabilities, disables privilege escalation, and uses
a private temporary filesystem. Use a trusted image without embedded secrets.

The plugin emits JSON events through stdout. The runner suppresses pytest and
test output. The outer process journals each event and parses it outside the
container. After each test's teardown report is durable, it atomically writes
PB's progress record. Counts remain cumulative across trees and invocations
inside the same action. The PB progress token never enters a container.

PB #2403 remains open. The published runtime offers managed containers and
resource containment, but no enforced `hostile-v1` submission profile or
worker-owned report custody. These explicit flags exercise the ticket's
container restrictions. They do not qualify PB's broader hostile-code lane.
In particular, a read-only Python plugin shares an interpreter with the tests.
Read-only files do not prevent deliberate in-process monkeypatching or forged
events. So the reports are honest about their custody, not tamper-proof.
Callers cannot set the custody through the result's constructor. Moving custody
to PrismaBuild once PB #2403 lands needs a follow-up integration, which adds a
second custody value.

## Verification

`tools/prepare_image.py` prepares dependencies in a PB-owned container and
commits that container to an immutable image. It needs a locally available
Ubuntu 26.04 base image. This is trusted preparation with network access;
test containers have none. PB refuses plain `docker build` because a builder
can escape its resource scope.

From a clean commit, submit the verifier with the prepared image:

```sh
python3 /mnt/shared/prismabuild-fleet/repo/tools/pbrun.py \
    --cwd "$CHECKOUT" --anywhere --cpus 1 --demand mem_gb=2 --priority -10 \
    --container-image "$IMAGE" --progress-phase tests=120 \
    --env PYTHONPATH=harness/src --env OMP_NUM_THREADS=1 \
    --env MKL_NUM_THREADS=1 --env OPENBLAS_NUM_THREADS=1 --detach -- \
    /usr/bin/python3.14 harness/tools/verify.py "$IMAGE"
```

Then use `pbwait.py --endings` and inspect the verified logs, receipt and cleanup
record. A queued action is not a pass.

The verifier runs the package tests inside a contained Python 3.14 process.
It then creates fixture Git repos and calls the real entry point through PB's
Docker shim. Fixtures cover every check kind passing and failing, setup and
collection errors, runtime errors, missing files and names, skipped tests,
xfail, uncollected tests, pre-existing failures, and a process exiting without
a report. A live fixture checks the exact admitted CPU mask, blocked network,
absent fleet and socket paths, absent inherited environment, and read-only
checkout and harness mounts.

## Prepare and roll a repository image

Build from a clean commit through PrismaBuild. The preparation script uses
PB's managed Docker shim and commits the admitted container. Its default
`sandbox` profile retains the original dependencies. The `prismabuild` profile
adds GCC, libc development headers, bubblewrap, pytest-xdist 3.8.0, PyYAML
6.0.3, and setuptools 80.9.0. Both profiles use pytest 9.0.2 and Python 3.14.
The `prismaquant` and `tessera` profiles use the reviewed frozen dependency
lists in `tools/images/`. They add g++, make, Python development headers,
libgomp, CPU-only Torch 2.11.0, Triton and the repositories' Python dependencies.
PrismaQuant uses pytest 9.1.1 and Triton 3.6.0; Tessera uses pytest 8.4.2
and Triton 3.8.0. Build tools are bootstrapped before installing the frozen
lists without dependency resolution or build isolation. Only the pinned Torch
wheel uses the PyTorch CPU index. Other indexed packages come from PyPI.
Both indexes are available only during trusted preparation. The served repository
is not installed into its own image; checks import the exported candidate.
PrismaQuant's frozen Tessera dependency is a separate, pinned Git install.

Bubblewrap's presence does not grant user namespaces or privileges to tests.
Use a distinct tag for a candidate so preparation preserves the current image.

On dl380g10, with a locally available Ubuntu 26.04 base image:

```sh
python3 /mnt/shared/prismabuild-fleet/repo/tools/pbrun.py \
    --cwd "$CHECKOUT" --tag dl380g10 \
    --cpus 2 --demand mem_gb=4 --priority -9 \
    --container-image "$BASE_IMAGE_ID" \
    --env OMP_NUM_THREADS=1 --env MKL_NUM_THREADS=1 \
    --env OPENBLAS_NUM_THREADS=1 --detach -- \
    /usr/bin/python3.14 harness/tools/prepare_image.py "$BASE_IMAGE_ID" \
    --profile prismabuild --tag tl3-harness:pb-candidate
```

Inspect the terminal record, verified logs, CAS receipt and cleanup. The log's
`container_image` is the immutable ID to qualify and configure. The tag is
only a preparation label. The image ID is the real pin: apt packages and pip's
transitive dependencies resolve at build time. An image built on dl380g10 is amd64 and local to dl380g10;
the ARM workers need a separate native build and its own immutable image ID.
The pinned Torch CPU and Triton releases publish Python 3.14 aarch64 wheels.
Native arm64 builds are worth qualifying to use the Sparks' CPU capacity;
CPU Torch itself is not an architecture blocker. An amd64 image does not
qualify an arm64 build or prove the remaining native dependencies there.
PB's `--container-image` requirement keeps checks on a worker that positively
reports the image. Do not add a host tag to ordinary checks.

Make a clean, unshallowed clone of the served repository and create a bundle:

```sh
git -C "$SERVED_CLONE" bundle create "$INPUT/served.bundle" HEAD
```

Install the trusted harness outside the served checkout. Snapshot the bundle,
trusted package and inventory script together in a separate committed input
repository. Submit `tools/inventory.py` with the bundle path, image ID and
selected test file paths. For PB, use the files listed in `tools/verify.py`'s
`FAST` tuple plus the existing test files for each pilot issue. The script
extracts function and method node IDs and calls `run_checks` on the exact
bundled revision for both base and head. Pytest expands parametrized cases.
Functions explicitly marked `__test__ = False` are excluded.

```sh
python3 /mnt/shared/prismabuild-fleet/repo/tools/pbrun.py \
    --cwd "$INPUT" --anywhere --cpus 1 --demand mem_gb=4 --priority -9 \
    --container-image "$IMAGE_ID" --progress-phase tests=300 \
    --env PYTHONPATH=trusted --env OMP_NUM_THREADS=1 \
    --env MKL_NUM_THREADS=1 --env OPENBLAS_NUM_THREADS=1 --detach -- \
    /usr/bin/python3.14 inventory.py --per-file served.bundle "$IMAGE_ID" \
    tests/test_example.py tests/test_other.py
```

The runner bounds each report record to 4 MiB and also bounds the record count. For a large expanded suite,
use `--per-file` to call `run_checks` separately for each selected file inside
one admitted action. Results are combined in one JSON inventory, and each call
continues the action's cumulative progress count.

An inventory action exits nonzero if any selected node does not pass. Its
JSON retains every node's outcome and failure message. Record skips and
classify failures as image dependencies, host requirements, or test defects.
Do not grant root, GPUs, the shared mount, or additional container access to
make a pilot pass. A Git export has no `.git` directory, and `/tmp` is tmpfs;
tests requiring repository metadata, executable temporary files, or qualified disk scratch need separate
pilot eligibility decisions.

Repeat the same selection in the candidate image, run the sandbox suite
through `run_checks`, and run `tools/verify.py` for the harness boundary checks.
Record action keys and every remaining limitation in the PR. After review,
the coordinator sets the immutable ID on the relevant `[[github.repos]]`
entry, or updates the global default. Keep the previous ID for rollback.
Preparation and verification do not edit live configuration.

Plans whose selected node IDs exceed the report budget are rejected by the
planner and before PrismaBuild submission, with an instruction to reduce the
selection. The harness checks the same budget at entry. Pytest can expand a
short function selector into many parameter cases; if that collection exceeds
the budget, it reports a test-plan error rather than an infrastructure failure.
Selections enter the container through stdin so large plans fit independently
of the host's per-argument limit.

The plan also reserves space for every test verdict within the controller's
8 MiB result limit, including repeated tests across checks and JSON escaping.
Results include messages for the first 20 failing tests, clipped to 1,000
characters each. Every named test retains its outcome and pass flag.
After collection, each event is limited to the longest collected ID plus 8 KiB.
The trusted reader also limits the whole journal to 32 MiB and bounds its
record count. These bounds apply independently.

Each test receives a temporary directory that is removed after its function
fixtures finish, including when setup, the test or teardown fails. `tmp_path`
and `tmpdir` use pytest's numbered, test-named directory. Python's default
temporary directory and the `TMPDIR`, `TEMP` and `TMP` environment variables point there during the test,
so cooperating subprocesses use the same scratch. Module and session fixtures
retain their separately owned temporary directories for their scope.
Cleanup is best effort: a file recreated by a background writer must not turn a
passing test into a teardown error. Any residue stays within the fixed tmpfs
and is discarded with the container.
The container has one disposable 512 MiB `/tmp` tmpfs. This is RAM charged to
the action's memory slice, alongside Python, subprocesses and other test memory.
The scratch verifier runs at production demand, 1 CPU and 2 GiB, including
four consecutive tests that each write 384 MiB. Cleanup bounds cumulative
plan use. One test must fit within that boundary; paths hardcoded outside its temporary directory and
wider-scope fixture data still consume the shared bound.

## Executable scratch

`Container(image_digest, exec_scratch=True)` allows executables and native
libraries on the private `/tmp` tmpfs. The default is noexec. Each test still
gets its own pytest temporary directory as TMPDIR, TEMP and TMP, with cleanup
after teardown. The mount retains its 512 MiB bound, nosuid and nodev.
Tmpfs uses RAM charged to the action's declared memory demand. It shares that
budget with the tests and their children. Cleanup errors retain the test's
real outcome; retained files and session temporary data share the same bound.

The controller selects this option from `[[github.repos]] exec_scratch` for
checks and maker runs. Base-result caching includes the setting. Git metadata
and disk scratch remain unavailable. See the
[design note](../docs/design/executable-scratch.md).

The executable-scratch verifier needs an image containing gcc and C library
development headers, such as the `prismabuild`, `prismaquant` or `tessera`
profile. It compiles a shared library and checks both exec and noexec behavior,
containment, maker summaries and per-test cleanup. Run it separately from the
general verifier, which also supports images without gcc:

```sh
python3 /mnt/shared/prismabuild-fleet/repo/tools/pbrun.py \
    --cwd "$CHECKOUT" --anywhere --cpus 1 --demand mem_gb=2 --priority -10 \
    --container-image "$IMAGE" --progress-phase tests=300 \
    --env PYTHONPATH=harness/src --env OMP_NUM_THREADS=1 \
    --env MKL_NUM_THREADS=1 --env OPENBLAS_NUM_THREADS=1 --detach -- \
    /usr/bin/python3.14 harness/tools/verify_exec_scratch.py "$IMAGE"
```

Use `pbwait.py --endings --until cleaned` and inspect the terminal status,
verified logs and CAS receipt before accepting the result.
