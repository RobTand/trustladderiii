# tl3-github

Rust edition 2024. This crate provides GitHub operations for the slice-one controller
without depending on it. It uses REST only.

## Client

Deserialize the app's credential JSON into `AppConfig`. Its fields are `id`, `slug`,
`pem` and `webhook_secret`, as described in
[app setup](../../deploy/github-app/README.md). The crate never opens a credential file.
Unused `client_id` and `client_secret` JSON fields are ignored during deserialization.
Credentials and tokens have no debug implementation. Errors omit response bodies and tokens.
The client keeps only the app id, slug and parsed signing key, plus its installation token.
Keep the webhook secret separately for the verifier before passing the config to the client.
The client drops the PEM text and webhook secret after construction.

`GitHub::new(config, installation_id, ClientOptions::default())` creates a clonable
client. It signs RS256 JWTs with the app id, backdates issuance by 60 seconds and
expires them nine minutes ahead. It exchanges them for installation tokens and
caches each token until one minute before expiry. Clones share the cache and serialize
HTTP requests. A 401 invalidates that token for the next call.

`installation_token()` exposes the cached installation credential to the Git
worktree adapter. `installation_id()` binds that client to a served repository.
Never log or persist the returned token.

Every call pins `X-GitHub-Api-Version: 2022-11-28`. Slice one needs no newer endpoint
behavior, so the older supported version avoids adopting unrelated breaking changes.
The API URL defaults to GitHub. A different HTTPS API root must end in a slash.
Loopback HTTP supports fake-server tests. Redirects return their HTTP status rather
than forwarding credentials to a new URL.

Public operations:

| Methods | Purpose |
|---|---|
| `issue`, `comments`, `issue_events` | Read an issue and all pages of comments |
| `post_comment`, `edit_comment`, `reconcile_comments`, `delete_comment` | Post, edit and deduplicate comments |
| `close_issue` | Close with `Completed` or `NotPlanned` |
| `create_check_run`, `check_runs`, `check_suites` | Create a check with a head and external id; read all check pages |
| `open_pull_request`, `reopen_pull_request`, `pull_request`, `pull_requests_for_branch`, `close_pull_request` | Open, inspect, reconcile and close PRs |
| `branch`, `delete_branch` | Read or delete a branch, including names containing slashes |
| `catch_up` | Recover changed issues, comments, label actors, PRs and tracked-head checks |

List calls request 100 rows per page until the final page. Missing branches return
`None`. Deletes accept 404. A branch deletion's 422 confirms only if a follow-up
read finds the branch absent. Permission and validation failures remain errors.

Reads, edits and deletes retry transport errors and HTTP 500, 502, 503 and 504,
with bounded exponential backoff. Token issuance can also retry. Comment, check-run
and PR creation never blindly retry an uncertain write. Reconcile by marker,
check external id or branch before attempting them again. The controller owns
effect sequencing and persistent retries.

The client honors `Retry-After`, exhausted primary limits and secondary-limit errors.
It shares a blocked-until time across clones, including after a successful response
that exhausts the primary limit. It returns `RateLimited { retry_after }` if a wait
would exceed `max_wait`, or retry attempts run out. The caller must respect that
delay. Mutation requests are spaced one second apart by default. Independent clients
and processes must coordinate their limits through the controller.

## Comment markers

Each posted or edited comment begins with an invisible HTML comment:

```text
<!-- tl3:effect:HEX_UTF8_EFFECT_KEY -->
```

Hex encoding allows any effect key without breaking the HTML comment.
Callers pass the visible body separately. Bodies containing an embedded TL3 marker
are rejected. Lookup requires an exact marker line in a comment by this app's bot.
`post_comment` reconciles before posting. Clones serialize this operation.
`reconcile_comments` retains the lowest comment id and deletes extra app copies,
including those from a cross-process race. This follows the state table's
at-least-once comment contract. The controller calls it during recovery too.

## Webhooks

`webhook::verify_and_parse(secret, signature, delivery_id, event_name, raw_body)`
verifies the raw bytes using constant-time HMAC-SHA256 comparison before parsing.
Pass the unchanged `X-Hub-Signature-256`, `X-GitHub-Delivery` and `X-GitHub-Event`.
Unsupported event actions return a delivery with no event.

Typed events cover issue labels, created issue comments, closed or merged PRs,
synchronized PRs, check-run updates and completed check suites. Label events retain the
sender's numeric id. `Event::starts_request(rob_user_id)` accepts only Rob's
`tl3` label on an issue. The controller also validates the served repo and installation.
Webhook signatures authenticate GitHub delivery, not permission to start work.

`webhook::receive(..., inbox)` verifies and inserts a delivery through `Inbox`.
Its unique insert must atomically persist the delivery id and event. Return false
for a duplicate. Commit failures must leave both absent so redelivery can retry.
Production implements `Inbox` with the controller's database transaction.
The in-memory inbox exists only in the test suite. After persistence, the controller
feeds stored events to the kernel. A delivery must never be marked consumed before
its event is durable.

## Outage recovery

`catch_up(served_repos, since)` returns current snapshots and a new cursor captured
before the pass starts and rounded down to a whole second. Queries look back one
additional second because GitHub timestamps have second precision and its since
filters exclude the boundary. It includes open and closed issues updated since the
cursor, repository comments, label changes with their original actors, and PRs
updated since the cursor. PR listing sorts by update time, newest first, and stops at
the first row older than the cursor, independently of issue timestamps. Label recovery
uses the repository-wide `/issues/events` endpoint, which includes issue numbers,
labels, actors and timestamps. The endpoint has no `since` filter or documented
chronological ordering. The pass stops after a full page of events older than the
cursor only while all event ids seen so far strictly descend, including across
page boundaries. An equal, increasing or missing id disables early stopping for
the rest of the pass. It rereads
checks on every supplied `tracked_heads` entry and every changed PR head, since a
check can finish without updating an issue.

Apply the snapshots idempotently and persist the new cursor only after every result
is durable. A failed pass returns no cursor. Inclusive timestamps and the cursor
captured before reads deliberately overlap passes. Supply every head still relevant
to recovery, including stopped or closed requests. GitHub's issue events preserve
label actors, so recovery never infers authorization from a label's current presence.
These are current facts, not a replay of every intermediate PR or check state.
Mergify queue recovery belongs to its own integration.

## Message checker

`message::check(&Draft, &Policy)` is a pure function returning `CheckedMessage` or
a list of `Violation` values. Only a successful check exposes rendered text.
`check_prose` applies the same content rules to notes and summaries without
question-only fields.
The draft supplies the original issue title and an exact request quote, a one- or
two-sentence obstacle, two or three actions with outcomes, a recommendation index
and reason, and answer instructions. The original title comes from the issue,
not from the message-writing agent.

The checker scans only TL3-written fields. It rejects exactly these vocabulary
forms as whole words, ignoring case: leaf, lineage, lineages, replan, replans,
replanned, replanning, re-plan, re-plans, re-planned, re-planning, measure, measures.
It allows leaves, measured and measuring.
It also checks code identifiers, hashes, issue ids, file paths and recognizable
logs. A hex id has at least seven hex characters, at least one digit and at least
one letter a-f. Plain numbers and words such as defaced pass.
Plain English such as yes/no, and/or and 24/7 passes, as do product names such as
GitHub, PostgreSQL, YouTube, Node.js and Next.js.
Matching normalizes Unicode and strips invisible formatting characters. `Policy::internal_terms` adds actual
internal names and ids known to the controller. It must include identifiers that
have no recognizable code shape, including internal CamelCase names.
The controller supplies `Policy::known_paths`, a set of repo-relative files and
directories from the request's `Delivery.paths` and any explicit adapter policy. Known entries must
contain a slash. Trailing slashes are normalized. A token is a path when it equals
a known entry, starts with `/`, `~/`, `./` or `../` followed by at least one path
segment, or contains a slash and its last segment ends with a dot and one to five
lowercase letters or digits. Surrounding prose punctuation, Markdown emphasis,
curly quotes and trailing `:line` or `:line:column` suffixes are ignored.
There are no other path rules. Unknown names such as deploy/github-app and
workers/src pass until the controller includes them in the set. Dates such as
10/30/2026 and phrases such as pass/fail/skip, Mon/Tue/Wed, TCP/IP/UDP and approx./
pass too.
Required fields are checked for nonempty text.
The checker cannot prove that prose is true, that an outcome follows from an option,
or that advice is sensible. Those remain the writer's and reviewer's responsibility.
The original issue title is quoted verbatim and excluded from content scanning.

## Verification

Run the integration tests against their in-process fake HTTP servers through
PrismaBuild. Tests generate a throwaway RSA key. They never read real credentials
or call GitHub. Fixture provenance is in [tests/fixtures](tests/fixtures/README.md).

From a clean commit, submit with 2 CPUs, 4 GiB, `--anywhere`, and:

```text
PATH=/home/rob/.cargo/bin:/usr/local/bin:/usr/bin:/bin
RUSTUP_HOME=/home/rob/.rustup
CARGO_HOME=/home/rob/.cargo
KANI_HOME=/home/rob/.kani
CARGO_BUILD_JOBS=2
RUST_TEST_THREADS=2
OMP_NUM_THREADS=1
MKL_NUM_THREADS=1
OPENBLAS_NUM_THREADS=1
```

With the action's working directory set to `crates/github`, run `cargo test --locked`,
then `cargo clippy --all-targets --locked -- -D warnings` in the admitted action.
A submission acknowledgement is not a pass.

GitHub references: [JWT claims](https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/generating-a-json-web-token-jwt-for-a-github-app),
[REST rate limits and retries](https://docs.github.com/en/rest/using-the-rest-api/best-practices-for-using-the-rest-api),
[webhook payloads](https://docs.github.com/en/webhooks/webhook-events-and-payloads).
