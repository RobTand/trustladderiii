# Bazel

Bazel builds every Cargo crate and runs every test the Light checks run, one
target per Rust test binary and per Python test module. Cargo and uv stay the
sources of truth: `Cargo.toml`, `Cargo.lock` and `workers/uv.lock` drive the
generated Bazel inputs.

## Running tests

Run Bazel on BuildBuddy, never with a local Bazel server on a fleet host.
Never run the `bb` CLI's `remote` command. Its argv carries the API key,
which process listings expose.

For committed workflows, use the in-repo Run API client,
[`tools/ci/remote_gate.py`](../tools/ci/remote_gate.py). It reads
`BUILDBUDDY_API_KEY` from the environment and sends the key only in an HTTP
header. The client starts the gate on a BuildBuddy runner and checks the
runner's result.

On dl380g10, agents use `/home/rob/tmp/bazel-wave2/tools/bbrun.py` for narrow
test runs. This helper lives outside the repository:

```sh
python3 -I /home/rob/tmp/bazel-wave2/tools/bbrun.py test \
  --repo https://github.com/RobTand/trustladderiii.git \
  --commit <pushed-SHA> --branch <branch> --timeout 10 \
  -- bazel test --config=remote --jobs=16 //workers/tests:test_maker
```

Run only the targets your change touches. Do not run full suites during
development. Keep `--jobs=16` after the configuration flags. Read the verdict
from the command's exit status and the invocation it links, not from the start
message.

Keep credentials out of logs and uploaded files:

- Never list processes with `ps`, `pgrep`, or `/proc/*/cmdline`.
- Never print the environment in a test, script, or log.
- Never write BEP or build-event files into uploaded or artifacts directories.
- Scrub API keys and tokens from every child process that is not Bazel.

Local Bazel is only for short Gazelle, generation and query commands. Put
`startup --max_idle_secs=300` in your own bazelrc (for example one passed with
`bazel --bazelrc=...`), not in the repository's `.bazelrc`.

## Adding a test file

- **Rust:** add `crates/<crate>/tests/<name>.rs` or
  `crates/<crate>/tests/<name>/main.rs`. Bazel finds it by glob; no BUILD edit
  and no generation step. A test that needs a `[[test]]` entry in `Cargo.toml`
  (`harness = false`, or a custom `path`) also needs `tools/bazel/generate.sh`.
- **Python:** add `test_*.py` where the Light checks already collect tests,
  then run `tools/bazel/generate.sh` and commit the BUILD changes Gazelle
  writes.
- **Runtime inputs** that neither Cargo nor Gazelle can see (fixtures in
  another directory, scripts a test starts) go in the tables at the top of
  `tools/bazel/rust.bzl` or `tools/bazel/python.bzl`, keyed by package.
- **A test that cannot be hermetic** goes in `tools/bazel/nonhermetic.bzl`
  with its reason. Remote test actions have no network unless listed there.

CI fails when a generated input is stale (`tools/ci/bazel_generated.py`) or
when Bazel's tests differ from what `cargo metadata` and `tools/ci/check.py`
discover (`tools/ci/bazel_parity.py static`). Both name the missing file and
the fix: `tools/bazel/generate.sh`.

## Configurations

- `--config=remote`: BuildBuddy cache, remote execution on the pinned
  Ubuntu 24.04 image, BES, `--jobs=80`.
- `--config=remote-arm64`: the same, but every action, including compiles
  and tools, runs on BuildBuddy's linux/arm64 executors. Remote Bazel runners
  are amd64 only, so the client stays on amd64. The post-merge run uses it.
- `--config=ci`: gRPC and compact execution logs, CI build metadata, and the
  per-test receipts parity reads. CI adds `COMMIT_SHA` and `BRANCH_NAME`.

Keep the API key in the environment for the Run API client. Never put the key
in command arguments or committed files.

## CI

The Gate workflow (`.github/workflows/gate.yml`) starts
`tools/ci/bazel_gate.py` on a Remote Bazel runner through
`tools/ci/remote_gate.py`, and reads back `gate-result.json`, which the runner
leaves as a BuildBuddy artifact. The gRPC and execution logs come back as a
7-day workflow artifact.

Selection fails closed. The gate fails when the queue-aware base is missing,
bazel-diff fails, a changed file is read by no target and is not in
`IRRELEVANT`, build-relevant changes impact no target, or Mergify does not
receive the scope list. Changes matching `FALLBACK` (Bazel, Cargo and uv
locks and manifests, toolchains, workflows, `tools/bazel/`) test everything
and mark the PR as touching every scope.

Each push to `main` runs every test on amd64 and arm64. A failure names the
merged PRs and says whether their selection included the failing test. The
job summary and the `post-merge-<sha>` artifact hold one `POST_MERGE_RECORD`
JSON line per run.
