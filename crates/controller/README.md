# TL3 controller

`tl3-controller` connects the pure kernel to the Postgres store and upstream
executors. Read [the architecture note](../../docs/architecture.md) for the
transaction and recovery contracts.

All commands use the same TOML configuration as `tl3-config`. This release
supports explicit simulation only. It never loads deployment GitHub or model
credentials.

Both commands use `Config::load`, which checks secret-file metadata, including
the optional store password. Only the store reads its password at runtime.
`tl3 status` uses `Store::open_read_only`. It never migrates or writes and
reports an uninitialized database clearly.

```text
tl3 status --config /absolute/path/to/config.toml
tl3d --config /absolute/path/to/config.toml --simulate /absolute/path/to/fake-records
```

The daemon reads one JSON `cli::Input` per line on stdin. For example:

```json
{"LabelledIssue":{"delivery":"fixture-42","owner":"RobTand","repo":"tl3-sandbox","installation":1,"number":42,"actor":24923402,"label":"tl3"}}
```

The example repository, installation and actor must match the configuration.
Fake receipts live outside the controller process. Keep their directory when
restarting against the same database. `--drain` recovers persisted work and exits
without reading stdin. EOF ends an interactive simulation.

Register a replacement executor with one `Registry::register` call. The five
modules under `src/adapters` each contain a fake. The controller owns interning
and persistence. Executors own authoritative reconciliation and epoch fencing.
`adapters::simulation` is currently the only registry builder. A test starts
with it and replaces one entry with `register`. The `mixed_registry` scenario
uses the real `tl3-worktrees` client in a temporary repository and delegates
the remaining worktree effects to its fake. The other four upstreams stay fake.

Each adapter ticket extends `Delivery` with the fields its real client needs.
The tracer does not predefine issue text, branch text, or PR text fields.

Run development verification through PrismaBuild from a clean commit:

```bash
python3 /mnt/shared/prismabuild-fleet/repo/tools/pbrun.py \
  --cwd "$PWD" --anywhere --cpus 2 --demand mem_gb=6 --priority -10 \
  --env PATH=/home/rob/.cargo/bin:/usr/local/bin:/usr/bin:/bin \
  --env RUSTUP_HOME=/home/rob/.rustup --env CARGO_HOME=/home/rob/.cargo \
  --env CARGO_BUILD_JOBS=2 --env RUST_TEST_THREADS=2 \
  --env OMP_NUM_THREADS=1 --env MKL_NUM_THREADS=1 --env OPENBLAS_NUM_THREADS=1 \
  --detach -- bash crates/controller/tools/verify.sh
```

Run this from the repository root. Use `pbwait.py --endings KEY` and inspect the
exit code, logs and verified CAS receipt. CI runs separately on Mergify runners.

## GitHub ingress in simulation

`ingress::Source` runs on its own thread and sends `cli::Input` values to one
controller thread. Register a source with `Sources::register`. The stdin reader
uses that same interface. `ingress::run` drains persisted deliveries before
applying inputs and drives the controller until no work remains.

To exercise the receiver against a local GitHub mock server, supply
`--github-listen 127.0.0.1:8081 --github-api http://127.0.0.1:8082/` alongside
`--simulate`. `github.webhook_listen` in the TOML file also sets the listen
address. The mock App credentials come from `github.app_credentials_file`.
The API must be loopback HTTP in this simulation release. Ordinary stdin
simulation still needs no App credentials. Live startup and tunnel deployment
remain tickets #20 and #19.

POST deliveries to `/webhooks/github` with GitHub's delivery, event and signature
headers. The library verifies the signature. The receiver inserts the complete
translated delivery into `tl3_ingress` before returning 202. Its connection never
changes kernel rows or the controller revision. Delivery ids remain unique after
consumption. A crash between applying an input and marking its delivery consumed
replays the same intake or observation key.

`Webhooks` runs catch-up once at startup. A tunnel supervisor calls
`WebhookControl::gap` after an outage. A failed persistence or response also marks
a gap, and the next connection event retries catch-up. There is no polling timer.
Catch-up uses the library's paging and original label actors. Its transaction
persists every recovered input and the new cursor together. Overlapping webhook
and catch-up inputs name one request per issue and one observation per comment.
Rob's comments stay in `github_comments` for ticket #14 to classify.

`adapters::github::Adapter` uses the REST client for kernel notices. It invokes
`workers::writer::Writer`, checks every draft and allows two correction attempts.
`FakeWriter` returns fixed prose in simulation. The seam uses the existing writer
role's protocol v1 input and reply shapes. Questions retain the kernel's allowed
options. Closing summaries close the issue, except when the request is replaced.
PR effects still use this module's fake until ticket #13. Incident and replacement
notices also stay on the fake for their owning tickets. Incidents need the
operations issue, and replacements need a new request, not another comment.
The checker includes TL3's tracked paths, generated during the build, the
request's declared paths, and any further paths supplied in the adapter policy.

The `github` integration target uses the recorded webhook fixtures, the actual
HTTP receiver, a local REST server and embedded Postgres. It covers delivery
persistence, unauthorized labels, gap recovery, duplicate reconciliation, writer
rejections, epoch fencing and SIGKILL after posting before acknowledgement.

For the review-fix evidence, create a clean detached worktree at the final
commit, then generate the campaign there:

```bash
python3 crates/controller/tools/review_manifest.py > /tmp/tl3-review-campaign.json
python3 /mnt/shared/prismabuild-fleet/repo/tools/pbcampaign.py --detach /tmp/tl3-review-campaign.json
```

The campaign adds the kernel tests and three Kani harnesses covering retryable
acknowledgements, publishing transitions, and queued progress. Read each terminal
record, log, and CAS receipt before reporting a pass.
