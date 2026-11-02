<img src="assets/icon.svg" width="64" height="64" alt="">

# TrustLadderIII

Created in [T3 Code](https://t3.codes).

## Review exhaustion

Q44 gives an exhausted reviewer one fresh conversation on the next reviewer
model. If that review also runs out of room, Planning must split the leaf using
the maker's credit rules. The [decision log](docs/state-machine.md) separates
Rob's decision from the coordinator defaults that Rob still needs to check.

## Queue a pull request

Wait for the required `Rust` and `Python` checks to pass.
Then use Mergify's `queue` command in a PR comment or its queue control in GitHub.
For the comment command, type an at-sign, `mergifyio`, a space, and `queue`.
Nothing queues automatically. Use the corresponding `dequeue` command to withdraw it.

The queue checks one PR at a time, in place. It merges `main` into the PR branch
when needed, reruns checks on that head, then merges the PR with a merge commit.
It does not rebase or force-push the branch. This repo's bootstrap queue follows
[PrismaBuild's configuration](https://github.com/RobTand/prismabuild/blob/main/.mergify.yml).
Served repos' exact-head approval policy is a separate controller integration.

## CI gate

`.github/workflows/gate.yml` runs on every PR, every Mergify queue draft and
every push to `main`. Branch protection and Mergify's `merge_conditions`
require the checks `Rust` and `Python`.

- **Bazel** runs on BuildBuddy: the client on a Remote Bazel runner, every
  build and test action on remote execution. It diffs the queue-aware base and
  head with bazel-diff, runs the impacted tests, and uploads the impacted
  Bazel packages to Mergify as the PR's scopes. Build, lock and workflow
  changes test everything. Detection that cannot say what to test fails.
- **Rust** requires Bazel's Rust tests, rustfmt and Clippy with warnings
  denied, and the feature and Kani compiles.
- **Python** requires Bazel's Python tests, Ruff's syntax and Pyflakes rules,
  and the lock checks.
- **Post-merge full run**: each push to `main` runs every test on amd64 and
  then on arm64, and records whether the merged PRs' own selection included
  any failing test.

[docs/bazel.md](docs/bazel.md) explains how to run tests and how to add one.
New crates and test files are discovered; generated inputs and test parity
fail CI with the one command that fixes them.

If Mergify posts no check or comment on the first PR, open
[Mergify's installation page](https://github.com/apps/mergify/installations/new),
choose `RobTand`, choose **Only select repositories**, select `trustladderiii`,
and click **Install** or **Save** for an existing installation. The coordinator
must land `.mergify.yml` on `main` before the queue can use this configuration.
