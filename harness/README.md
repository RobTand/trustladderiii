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
changed test configuration raise `ValueError`. Missing admission raises
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

Test selectors are exact pytest node IDs, including parameter IDs when needed.
The harness collects the named files and selects the named items itself. It
disables automatic third-party plugin loading. Dependencies and any plugins
explicitly required by repository settings belong in the trusted image.

## Verdict rules

Demonstrating checks run the head's test edits against the base production
code. Every named test must fail there with a call-phase assertion and pass
on the head. A setup assertion does not count. The state table also permits
a missing name added by the change. The harness recognizes a call-phase
module attribute error when the head adds that module-level function or class.
Other exceptions cannot demonstrate a change.

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
base. The reviewed commit can be on another history after a rebase.

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
