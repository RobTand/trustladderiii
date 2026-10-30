# Check the sandbox merge gate

Run `python3 deploy/sandbox/check_settings.py` from an authenticated `gh` session.
The script makes REST GET requests only. Exit 0 means every setting and file
matches `approved.json`. Exit 1 reports drift or incomplete reads. Exit 2 means
the baseline or REST tooling could not be loaded. No command updates the baseline.

The checker compares both rulesets, all classic branch protections on protected
branches, repository merge settings, and SHA-256 hashes of the three gate files
on `main`. It also checks the local approved file copies. Pagination covers
rulesets and branches. Only classic protection and file reads treat HTTP 404 as
absence. Permission errors fail the check. All mismatches are reported, including
extra rulesets, bypass actors, and protections. Rule ordering and GitHub's root
ruleset identifiers and timestamps do not affect the comparison. App identifiers
inside rules and protections remain significant.

`files/` holds the reviewed copies deployed by sandbox PR #15. The bootstrap
merge was explicitly authorized because Mergify reads configuration from the
default branch. Later changes go through TL3 approval and the queue label.
Update the approved copies and hashes in the same reviewed change as any intended
sandbox policy change. The checker deliberately does not learn the live policy.

The branch ruleset covers `refs/heads/tl3/**/*`, including the controller's
`tl3/issue-<issue>-node-<node>` branches. Only integration 5542171 can create,
update, force-push, or delete them. Main requires a PR, the `TL3 approval` check
from that integration, and `Sandbox tests` from GitHub Actions. It has no bypass
actors. Code-owner review protects `.github/` and `.mergify.yml`; GitHub's
`require_extra_approval_for_unattributed_changes` default is explicitly retained.
Classic branch protection is absent because these requirements live in rulesets.

The current GitHub adapter still fakes `PostApproval` pending ticket #13.
`TL3 approval` is the served-repository contract in `crates/mergify/README.md`.
The live-trial helper uses `tl3-github::GitHub::create_check_run` with that name,
and Mergify's client for labels. It reads App credentials in-process, discovers
the installation with a signed JWT, and never prints credentials or tokens:

```bash
cargo run --locked -p tl3-mergify --example sandbox_gate -- approve HEAD_SHA
cargo run --locked -p tl3-mergify --example sandbox_gate -- queue PR_NUMBER HEAD_SHA
cargo run --locked -p tl3-mergify --example sandbox_gate -- dequeue PR_NUMBER
```

These commands perform authorized live sandbox operations, not offline tests.
The helper also supports `branch tl3/NAME SHA` and `delete tl3/NAME` for cleanup.
It is restricted to `RobTand/tl3-sandbox` and verifies the expected App identity.
`crates/mergify/tools/record.py` records GET-only queue snapshots under `evidence/`.
Webhook observations must come from deliveries, not reconstructed REST payloads.

Run the checker's offline tests through PrismaBuild from a clean commit:

```bash
python3 /mnt/shared/prismabuild-fleet/repo/tools/pbrun.py \
  --cwd "$PWD" --anywhere --priority -10 --cpus 1 --demand mem_gb=1 \
  --env OMP_NUM_THREADS=1 --env MKL_NUM_THREADS=1 --env OPENBLAS_NUM_THREADS=1 \
  -- python3 -m unittest discover -s deploy/sandbox -p 'test_*.py' -v
```

CI on Mergify runners runs directly. Agent development tests always use
PrismaBuild. Test records and live results are documented on issue #17.

The trial also found that GitHub Actions check outputs can have a null title.
The GitHub model preserves that absence; Mergify treats it as an unknown title.
The regression fixture is a real response from sandbox PR #16.
