<img src="assets/icon.svg" width="64" height="64" alt="">

# TrustLadderIII

Created in [T3 Code](https://t3.codes).

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

## Light CI checks

`.github/workflows/light-checks.yml` runs on GitHub-hosted Ubuntu runners for
every PR and push to `main`. It needs only read access to repository contents.
Branch protection should require the GitHub Actions checks `Rust` and `Python`.
The coordinator applies branch protection after review.

- `Rust` discovers every tracked `Cargo.toml` outside `spikes/` and `fixtures/`.
  Cargo metadata groups members by workspace, so each workspace runs once.
  It runs rustfmt, Clippy with warnings denied, and unit and integration tests
  for all targets. Commit the lockfile for each standalone crate or workspace.
  Adding the root workspace requires no workflow edit.
- `Python` lints every tracked Python file outside `spikes/` and `fixtures/`
  with Ruff's syntax and Pyflakes rules. It discovers every `pyproject.toml`
  outside `spikes/` and `fixtures/`, requires a `[project]` table, installs each
  project with uv, and runs pytest in it. Each file belongs to its nearest
  ancestor project, so parent pytest runs exclude nested projects.
  It uses `uv.lock` when present, default dependency groups, and the `test` extra
  when declared. Other directories containing `test_*.py` run through unittest.
  `tools/board.py` gets the same lint check as the packages.

New crates and Python packages need no CI list entry. Keep light Python tests
in each package's normal pytest suite. Container or service verification belongs
in separate fleet scripts, such as `harness/tools/verify.py`. Deliberately broken
test fixtures and historical spikes are excluded from discovery and linting.
The harness's ordinary unit suite runs here; its container verification and all
Kani proofs stay outside these light checks. Ticket #8 adds proof CI.

The runner calls `python tools/ci/check.py rust` and `python tools/ci/check.py python`.
Development verification of these commands must run through PrismaBuild under
Rob's execution policy. GitHub CI itself does not submit to PrismaBuild.

If Mergify posts no check or comment on the first PR, open
[Mergify's installation page](https://github.com/apps/mergify/installations/new),
choose `RobTand`, choose **Only select repositories**, select `trustladderiii`,
and click **Install** or **Save** for an existing installation. The coordinator
must land `.mergify.yml` on `main` before the queue can use this configuration.
