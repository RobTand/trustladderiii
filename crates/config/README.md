# TL3 config

`tl3-config` reads slice one's single TOML file. It is a standalone Rust 2024
crate with its own lockfile. The coordinator adds it to the Cargo workspace.
[example.toml](example.toml) cites the source of each setting.

`Config::parse(text)` rejects unknown fields at every level and validates the
values without accessing endpoints or secret files. `Config::load(path)` also
checks secret-file metadata. `validate_secret_files()` is available separately.
On Unix, each secret target must be a regular file with owner-read permission
and no group or other permissions. These checks never read the secret bytes.
They follow symlinks and do not replace runtime file-open checks.

The example passes offline validation. Model names, maximum effort and output
budget, the app installation id, and the container digest are
deployment placeholders. Validation checks their shape, not service availability.
Hindsight's endpoint uses dl380g10's local port 8890. Port 8888 is celestia's tunnel.
The proxy owns the supported models and efforts. Budget means the worker's
per-response `max_completion_tokens`, including reasoning where supported.
It must be positive and no larger than the configured context.

The maker has exactly three attempts. Attempts 1 and 2 share a model and family,
attempt 2 has an explicit effort and at least attempt 1's budget, and attempt 3
uses a different family. The operator sets the proxy's maximum effort and budget
for attempt 2. An omitted or empty reviewer ladder defaults to maker attempts 1
and 3, in that order. `models.reviewer_for(maker_family)` returns the first
reviewer whose family differs from the current attempt's maker, ignoring ASCII
case, or `None` if none exists. Validation requires a different-family reviewer
for every family in the maker ladder, including with an explicit reviewer ladder.
Other role ladders must contain at least one model. No model names or effort
vocabulary are built into the crate.

`models.worker_model(step)` serializes exactly the worker's model fields:
`base_url`, `name`, `effort`, and `budget_tokens`. Family stays in config.
The supervisor supplies `models.key_file` and a worktree under `worktrees_root`
as separate worker request fields.

`github.app_credentials_file` points to the JSON written by
`deploy/github-app/`. At runtime the controller loads it as
`tl3_github::AppConfig`, whose fields are `id`, `slug`, `pem`, and
`webhook_secret`. This crate neither duplicates nor reads that secret type.
Each served repo supplies the separate installation id that `GitHub::new`
requires. Rob's numeric id authorizes label events, while his login is display
metadata. Config does not resolve GitHub identities over the network.

`prismabuild.tools_dir` contains `pbrun.py`, `pbwait.py`, and the other published
tools. The harness image uses `sha256:<64 lowercase hex digits>` or
`repository@sha256:<64 lowercase hex digits>`, matching `harness.Container`.
PB's `content:sha256:` form is not a harness image reference.

`store` configures `tl3-store` with `host`, `port`, `database`, `user`,
`connect_timeout_secs` and an optional `password_file`. Omitting the section
uses local TCP port 5432 and database/user `tl3`. The store accepts only
`localhost`, `127.0.0.1`, `::1`, or an absolute Unix socket directory, without
TLS. A remote deployment needs a protected tunnel with a loopback endpoint.
Config validates an optional password file's metadata, and the store alone
reads its bytes at runtime. It never searches environment variables or default
credential files. Omitting the file uses the server's local authentication.

All paths are absolute. No tilde or environment-variable expansion occurs.
The worktrees root and tools directory need not exist during offline validation.
Allowances use positive integer seconds. Expiry triggers a look at the upstream,
not a work-duration deadline. Credit reserve and free rechecks allow zero;
W must be 1 for slice one. Listen addresses are numeric socket addresses with a
fixed nonzero port. HTTP endpoints allow LAN HTTP and reject embedded credentials,
queries, and fragments. Parse diagnostics omit source lines.

Run `cargo test --locked --manifest-path crates/config/Cargo.toml` through
PrismaBuild from a clean commit. Tests use temporary fixture secret files only.
