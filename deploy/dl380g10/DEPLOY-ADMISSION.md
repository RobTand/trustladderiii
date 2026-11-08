# Failed terminal closure

An ended Flux job without `result.json` can release the admission hold only
through `tl3.deploy.closed.v2` with `outcome: FAILED`. This closure permits a
new attempt. It never writes `receipt.json`, returns a successful stage digest,
or qualifies a deployment. Existing result-bearing closure and success checks
retain their original behavior.

## Terminal evidence

The caller verifies the original Flux jobspec against the persisted intent and
binds the job ID, numeric ID, generation, source and jobspec hash. It retains
status, eventlog, query return codes and the `clean` observation. The immutable
Flux `jobid-path` must be `/sys` in the pinned three-broker generation.

The terminal parser accepts two failure cases:

- `NEVER_STARTED` requires no allocation, start or finish, one fatal exception,
  and a matching failed, canceled or timed-out status. No payload waitstatus is
  permitted, except Flux's unknown sentinel `-1`.
- `STARTED` requires ordered allocation, start, nonzero finish and clean. The
  finish and status waitstatus must agree. A fatal exception, when present,
  must agree with the status and occur between start and finish.

The submit event must identify the coordinator's UID. Events require ordered,
finite timestamps. Missing, duplicate, malformed or contradictory records,
query failure and a successful status all refuse this closure. The command's
exit code must agree with Flux's POSIX waitstatus and exception floor; the
exit code alone never proves failure or cleanup.

## Independent cleanup evidence

The observer must run as `rob` on `dl380g10`. It verifies the owned cgroup
ancestry rooted at `/sys/fs/cgroup` and the user manager's `flux-jobs.slice`.
Flux 0.91 names sdexec shell units from the f58plain job ID and the immutable
instance path. The observer checks absence of the exact names with suffixes
`:sys.service`, `:0-sys.service`, `:1-sys.service` and `:2-sys.service`. It also
checks absence of both the Unicode and ASCII spellings of this job's scratch
path. It records cgroup slice and cleanup-root device/inode identities.

Neither native `clean` nor a missing payload result proves this absence. A
remaining scope, even an empty one, refuses because the payload supplied no
original inode. Any scratch entry, symlink, unknown owner, missing root or
changed identity also refuses. The observer never deletes evidence or scratch,
removes a cgroup, resets state, or changes a running job.

This path intentionally refuses allocation without a start record, multiple
exceptions, a missing user slice and other cgroup layouts. Those cases need
additional identity evidence or the separately authorized operator procedure.
They cannot be inferred safe from missing files.

A real payload killed mid-run by SIGTERM can leave scratch behind because it
has no SIGTERM cleanup handler. Closure then refuses. Any operator recovery
must follow the separately authorized evidence-preserving procedure before
reattaching the original job; this closure does not authorize scratch removal.
Reboot, remount or a user-manager restart can also change recorded physical
identities and hold later admissions. A FAILED closure proves job cleanup only,
not the safety or completeness of partial deployment side effects.

## Retained format and replay

`closed.json` contains `schema`, `outcome`, `intent`, `generation`, `job_id`,
`jobspec_sha256`, the parsed `terminal`, cleanup booleans and `files`.
`failure-identity.json` retains numeric ID conversion and instance-path queries.
`failure-cleanup.json` retains exact absent paths and physical root identities.
Each retained input has its SHA-256 and byte length in `files`.

A `null` file reference asserts absence. Only `result.json` and the never-started
job's missing stdout/stderr may use it. No empty replacement log is fabricated.
Replay verifies hashes, asserted absences, bindings and terminal semantics,
and independently rechecks cleanup and physical identities. Any successful
receipt alongside a failed closure refuses. The same global lock and
own-dispatch-only reattach exemption apply to both closure versions; a failed
closure cannot exempt another unresolved dispatch. Reattachment collects the
original ID. A new admission creates a new intent and Flux submission.

These are private wrapper observations, not native attestation. Synthetic
fixtures do not qualify a live deployment or authorize operator recovery.
Manual evidence-preserving recovery remains in effect until FX-L1 deploys.

## Reproduction

`fixtures/fx-l1-evidence` runs the two missing-result regressions against
immutable source `746c9797d3841baf8c018f26bbcbcd1b85cd00a4`. It also runs the
original M1 methods against landed 746c and pre-fix 591418e2. The script reports
source, original/effective fixture hashes, assertions and setup/runtime errors.
Its fixture adaptations preserve each source's walltime and lock layout.
Production baseline bytes are loaded unchanged into a temporary directory.

`green` runs the focused original admission companions and the full current
Flux fixture module, with syntax and baseline-compared Ruff checks. It excludes
unchanged helper, PGUTIL and MLflow suites. Run both modes only through
PrismaBuild or approved free remote execution. The original M1 diff adds eight
test methods and one `coordinator` helper, despite the review's count of nine
new tests. The evidence matrix retains every actual method and records that
count discrepancy without inventing a ninth test.
