# FX activation binding

The compiled `src/adapters/flux_checks/activation/qualification.json` pin records completed historical qualification.
`cargo run -p tl3-controller --example generate-fx-qualification -- --check`
uses `../tools/generate_fx_qualification.rs` to derive it from the retained manifests,
acceptance records, parent audits, and the parent's exact reconciliation ruling
in `fx-exact42b/`. The generator checks their original SHA256 values before
reading them. The regeneration test requires byte-for-byte equality.

All 37 historical inventory entries match. CANARY2 deliberately used
`/opt/tl3/bin/python-canary2-absent` to exercise launch failure. Its original
interpreter provenance is retained. The positive CANARY3 and SUCCESS runs
qualify `/opt/tl3/bin/python`. Activation rejects the fault interpreter.
The three historical publisher commits stay in the pin as provenance, separate
from acceptance source `42b68e30fcb3d37cc03c0a70166af9e912b25dc6`.
The parent reconciliation supplies original retained-record file:line citations.

The pin fixes every helper and imported module, the collector inventory, and
execution metadata. Only the three profile data files may vary: image map,
image inspection, and preflight. The deployment binding fixes their exact
bundle and preflight hashes and full image identity. Rust's existing bundle
reader verifies canonical addressing, full inventory, actual bytes, modes,
ownership, and cleanup presence. Its existing preflight validator checks the
image inventory, identity, and mode prerequisites. None of these offline
checks claims a live image or fleet observation. The backend's existing native
generation guard remains authoritative at execution.

`[flux.activation]` carries an absolute `record` path and its `sha256`.
`DeploymentRecord` in `src/adapters/flux_checks/activation.rs` defines the JSON
format. The schema is `tl3.fx_activation.v1`. `qualification_sha256` is the
SHA256 of the compiled pin. `producer_commit` is the frozen deploy candidate
used by ops to publish the new profile bundles, and must match each bundle.
Before provisioning, ops must compare `producer_commit` with the exact
captain fast-forward SHA that will be deployed, and check every listed bundle
against that same SHA. Retain that equality in the deploy runbook. Refuse
provisioning on a mismatch. It does not replace qualification by exact code equality.

`identity` hashes the UTF-8 bytes from Rust `serde_json::to_vec` for each field.
The generation, endpoint, staging root, and complete effective `Options` have
separate hashes. Options include explicit environment, arguments, executable,
bootstrap CLI/config paths, and journal root. CLI fields hash actual binary
bytes without executing them. The record contains identities and hashes only;
backend access settings and private configuration values do not belong in it.
Validation diagnostics contain field names only. Ambient environment does not
participate in activation.

`acceptance_scope` must be `planned_fresh_init`. `profiles` is exactly the
configured bundle set, with no duplicates, additions, or omissions. Every entry
pins its canonical manifest, image identity, preflight, a positive
`required_native_success_runs` count, and `required_cleanup: "verified"`.
The record grants only those exact selections to controller staging and worker
Prepare. A worker Activation command needs its retained profile and selected
image; it returns hashes, never a global permission for future profiles.
Existing Resume uses its original retained request and intent bindings.

This record describes planned acceptance. It does not claim that new profiles
have already run successfully. Ops publishes bundles only at the frozen
candidate, records the publisher's returned canonical locators, and provisions
the immutable hash-pinned record. After the released fresh-init acceptance,
ops retains actual native success and verified cleanup evidence separately,
bound to the record SHA256, exact profile bundle, candidate, and generation.
The record is never rewritten to add completion fields. Before real volume,
only profiles with successful completed acceptance belong in approved go-live
scope. A failed or unexercised profile is not accepted. A later profile requires
its own deployment binding and acceptance run. This source change releases no
native or deployment actions.

The canonical-path fixture tests are explicitly marked for PB execution because
RBE cannot create the fixed shared runtime root. Run the activation and worker
execution test selections with `--include-ignored --test-threads=1` in an admitted
job. The environment-override helper also runs as an isolated test subprocess.
These tests publish synthetic bundles only, never images or production profiles.

`tools/fx-scratch-check.sh` registers the activation and direct worker-Prepare
fixtures in the admitted PB lane with serial execution. Qualification
regeneration is also a default, non-ignored Rust test.
