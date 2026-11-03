<img src="assets/icon.svg" width="64" height="64" alt="">

# TrustLadderIII

Created in [T3 Code](https://t3.codes).

## Delegated commands

`[operations].delegated_attributions` lists exact standing-approval attribution
lines, for example `"(Posted by the TL3 build coordinator under Rob's standing approval.)"`.
The default is `[]`. Only unedited comments authored by the configured Rob account
qualify. The attribution must be the final nonempty plain Markdown line, outside
quotes, code blocks and HTML. Edited comments keep normal classification.
TL3 records the matching line as the delegate and classifies the remaining text as
Rob's command. Exact attributed `restart` commands use code directly when restart
is allowed. Other authors and non-matching lines keep their existing behavior.
Entries must be unique nonempty single lines without surrounding whitespace,
with at most 32 entries and 1000 bytes per entry. See `crates/config/example.toml`.

## Review exhaustion

Q44 gives an exhausted reviewer one fresh conversation on the next reviewer
model. If that review also runs out of room, Planning must split the leaf using
the maker's credit rules. The [decision log](docs/state-machine.md) separates
Rob's decision from the coordinator defaults that Rob still needs to check.

## Repository test resources

A `[[github.repos]]` entry can set `test_demand = { cpus = 4, mem_gb = 12 }`
for PrismaBuild tests. Independent Checking and the maker's `run_tests` and
`run_generator` actions use it. Omitting it keeps 1 CPU and 2 GiB. CPU and memory must be integers in
1..16 and 1..64 GiB respectively; GPU requests are excluded. See the
[config reference](crates/config/README.md) and
[example](crates/config/example.toml) for parallel-run sizing and failure receipts.

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

Helper diagnostics evidence uses planning capability method `helper_target_observer`.
It stages only new observer tests over the real base helper and records the old
and new source and tree identities for review. See docs/state-table.md, Q27 #549.

Helper arguments can select existing setup with `{"$setup": "_failed_queue", "$call": "execute"}`
when method `helper_target_observer.setup_function` is installed. The factory
must already be an undecorated synchronous function in the target's base test
module, callable without arguments and unchanged on head. Its return value is
one target argument. The declared callable method must be replaceable. A trusted
forwarder requires its real return before accepting the target assertion, so a
missing-row assertion cannot stand in for the failed outcome. No new code, imports or fixture dependency resolution is
accepted by this declaration. Setup exceptions cannot prove helper failure.
The existing case, driver and tree identities bind setup in the receipt. The
helper must still raise AssertionError on both versions. Select this mode when
ordinary staging cannot prove the requested change, rather than requiring it
for every helper in a test file.
