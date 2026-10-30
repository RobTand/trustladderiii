# tl3-worktrees

Rust edition 2024 workspace adapter for the controller's Git operations on Unix.
The crate uses the Git CLI. It does not depend on the controller or GitHub client.

```rust,no_run
use tl3_worktrees::{BranchName, Worktrees};

let repo = Worktrees::open("/srv/tl3/repos/sandbox", "/srv/tl3/worktrees")?;
let name = BranchName::new("tl3/issue-29")?;
let branch = repo.create_branch(&name)?;
let path = repo.create_worktree(&name, "/srv/tl3/worktrees/issue-29")?;
let files = repo.changed_files(&branch.base, &repo.head(&name)?)?;
# Ok::<(), tl3_worktrees::Error>(())
```

## Operations

| Method | Result |
|---|---|
| `create_branch` | Branch name and durable current base commit |
| `create_worktree`, `remove_worktree` | Reconcile a checkout at an explicit path |
| `head`, `remote_head` | Local or remote commit, with absent remote branches returning `None` |
| `publish` | Publish an immutable head with a lease against the recorded remote head |
| `delete_remote_branch_at`, `delete_local_branch` | Lease remote deletion; remove the local branch and base |
| `rebase_onto` | Rebase onto an exact target; return base and head or conflicts |
| `branch_state` | Current `(head, base)` from the branch and recorded base refs |
| `finish_prepare` | Verify an unfinished checkout; repair only at its recorded base |
| `verify_removal_clean`, `resume_removal` | Record a clean preflight, then resume deletion-only retries |
| `changed_files` | Paths changed between two commits, including both sides of a rename |

All handles sharing a common Git directory serialize operations with an in-process
mutex. One controller process must own each repository; other processes must not
mutate it. No OS file locks are used because of the deployment kernel bug.
`create_branch` reads local main. For recorded effects, `fetch_main` fetches into
`refs/tl3/upstream-main`. `create_branch_at` and `rebase_onto` use the exact
recorded commit without changing a checked-out main.
The controller chooses effect identities. This client does not persist controller
state, retry network failures, or mark a new head approved. Every new head still needs checks and review.

`BranchName` rejects invalid ref names and option injection. Mutations reserve `main`.
`CommitId` accepts full SHA-1 or SHA-256 object ids. Paths use Unix bytes, including
newlines and non-UTF-8 names. `changed_files` compares the two trees directly and
disables rename detection so both the old and new path count toward the footprint.

## Recovery

New branch and `refs/tl3/bases/<branch>` refs are created in one Git ref transaction.
The base stays fixed when main advances. A clean rebase updates the branch and
base together in a ref transaction that checks both old values. The new base is
the exact main commit used by the rebase. An existing branch without a base
records its merge base with main. A surviving base without a branch recreates
the branch at that base. Local branch deletion removes both refs atomically.

The configured worktrees root must exist when the handle opens. Creation and
removal resolve symlinks and reject destinations outside that root, including the
root itself, before creating directories or changing Git state.

Worktree creation accepts a pre-existing empty directory. A registered checkout
whose directory disappeared is removed from Git's registry and recreated. A stale
`.git` file pointing into this repository can be repaired after its registration
directory disappeared. Recovery creates a temporary registration under
`<common-git-dir>/tl3-worktree-staging/<branch>`, initializes its index, and replaces
the orphan's gitlink before repairing the registration. It preserves working files.
An unfinished Prepare separately verifies the checkout against its branch head.
If population was interrupted, `finish_prepare` resets it only while the head
and recorded base still match. It also clears an interrupted index lock after
the owning process and Git children have ended.
A retry reuses the staging registration or finishes its cleanup. Other nonempty
directories and another branch's checkout return `WorktreeOccupied`.

Removal refuses tracked edits and untracked files. Ignored build files are allowed.
The controller records a clean preflight in its Remove intent. Only that record
authorizes `resume_removal`, which also accepts deletion-only interrupted removals.
If deletion removed `.git` first, recovery restores that link from a registration
whose path and branch both match before checking the remaining files.
It recovers a missing registration before removing a checkout. Repeating removal or branch
deletion after success has no effect.

Rebase requires a clean checkout. Ignored build files are allowed. A dirty checkout
is rejected without changing its files. An interrupted rebase for the requested branch
is aborted before retrying. A conflicting rebase captures the unmerged paths,
aborts, and verifies a clean checkout at the old head before returning. Filesystem
or Git failures during abort return `Recovery` so the controller retains the
resource and reports the problem. There is no automatic conflict resolution.
A per-checkout marker recovers detached rebases interrupted before or after the atomic branch and base update.

Pushes use immutable commit ids and explicit refs. `publish` requires the
previously published head from the controller's record. Its lease prevents a concurrent collaborator push from being
overwritten. A retry that finds the intended head already published succeeds.
Remote deletion leases the recorded ref, and an absent ref already counts as done.

## Credentials

`Remote::https` accepts only `github.com` HTTPS URLs without userinfo, query
strings, or fragments.
`Remote::local` converts a local repository into a `file://` URL for tests.
Network methods take `Option<&InstallationToken>`. Pass `None` for local remotes;
the client also withholds an accidentally supplied token from file transport.
`publish` leases an exact commit against the controller's recorded remote head.
`delete_remote_branch_at` leases deletion against a recorded head. Neither type has a debug
implementation. Neither does `InstallationToken`.

Authenticated Git processes receive the Basic authorization header through
`GIT_CONFIG_COUNT`, `GIT_CONFIG_KEY_n`, and `GIT_CONFIG_VALUE_n` environment variables.
The authorization header is scoped to `https://github.com/`.
The token and its encoding never enter argv, remote URLs, or repository config.
The adapter clears inherited environment variables, including Git tracing and
config injection. It disables interactive credentials, configured credential
helpers, redirects, and network protocols other than HTTPS. File transport supports
the local tests. Each process captures its output, and errors expose only the
operation name and exit code, never argv, URLs, stdout, or stderr.

Repository config and executables are trusted deployment inputs. Environment
delivery protects argv and persistent configuration, but it does not hide secrets
from processes with permission to read the Git process's environment. Credentials
are supplied only to network operations. Checkout and rebase never receive them.

## Verification

`tools/verify.sh` runs formatting, the local-repository tests, and Clippy. Submit it
through PrismaBuild from a clean commit with 2 CPUs, 4 GiB, `--anywhere`, and:

```text
PATH=/home/rob/.cargo/bin:/usr/local/bin:/usr/bin:/bin
RUSTUP_HOME=/home/rob/.rustup
CARGO_HOME=/home/rob/.cargo
CARGO_BUILD_JOBS=2
RUST_TEST_THREADS=2
OMP_NUM_THREADS=1
MKL_NUM_THREADS=1
OPENBLAS_NUM_THREADS=1
```

The tests create temporary repositories and a local bare remote. They never call
GitHub or a network remote. Every adapter Git invocation records the exact argv
at the spawn boundary. Tests check it for the fake token and its Basic encoding,
along with output, errors, config, reflogs, and remote URLs. A failing remote that
deliberately emits the fake token verifies that raw Git diagnostics never escape
through the public error type.
