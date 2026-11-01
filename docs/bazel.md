# Bazel

Bazel builds every Cargo crate and runs every test the Light checks run, one
target per Rust test binary and per Python test module. Cargo and uv stay the
sources of truth: `Cargo.toml`, `Cargo.lock` and `workers/uv.lock` drive the
generated Bazel inputs.

## Running tests

Run Bazel on BuildBuddy, never with a local Bazel server on a fleet host.
`bb remote` runs the Bazel client on a BuildBuddy runner, mirrors your
uncommitted changes, and sends test actions to remote execution:

```sh
BUILDBUDDY_API_KEY="$(cat ~/.config/buildbuddy/api_key)" GIT_REPO_DEFAULT_BRANCH=main \
  bb remote \
  --container_image=docker://gcr.io/flame-public/rbe-ubuntu24-04@sha256:5f0c01e5f6d79e58d7a996330de33aa38ce599bf8431eaf27521e7602daadf50 \
  --runner_exec_properties=EstimatedCPU=8 --runner_exec_properties=EstimatedMemory=16GB \
  test //... --config=remote
```

Replace `//...` with a package such as `//crates/store/...` or a single
target such as `//workers/tests:test_maker`. Read the verdict from the
command's exit status and the invocation it links, not from the start message.

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

Pass the API key header from the environment or a private rc file. Never commit
it.
