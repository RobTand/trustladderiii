# TL3 workers

`workers/` is a Python 3.14 uv project. It runs one agent role per process and
has no controller dependency. `.python-version` selects standard CPython with the
GIL. Its first roles are `ingest` and `audit`.

```bash
cd workers
~/.local/bin/uv sync --locked
~/.local/bin/uv run --locked python -m tl3_workers run < request.json
```

## Protocol v1

The supervisor starts `python -m tl3_workers run`, writes one JSON document to
stdin, then closes stdin. The worker reads through EOF. It accepts no second
document. Fields not described by the schemas are errors.

```json
{
  "protocol": 1,
  "operation": {"node": 1, "generation": 0, "kind": "ingest"},
  "role": "ingest",
  "model": {
    "base_url": "http://proxy.example.invalid:8317/v1",
    "name": "configured-model",
    "effort": "high",
    "budget_tokens": 32000
  },
  "key_file": "/run/tl3/model-key",
  "inputs": {
    "issue": {
      "number": 42,
      "title": "Fix the greeting",
      "body": "Return hello.",
      "author": "requester",
      "comments": [{"author": "requester", "body": "Keep the interface."}]
    },
    "repo": {
      "readme": "A greeting library.",
      "file_list": ["greeting.py"],
      "materials": [{"path": "greeting.py", "content": "def greet(): ..."}]
    },
    "memories": [{"id": "memory-1", "content": "The public API is greet()."}]
  },
  "worktree": "/srv/tl3/worktrees/request-42"
}
```

`operation.node` and `generation` are nonnegative integers. `kind` is a nonempty
string, preserved as supervisor metadata. The worker does not interpret it or
perform stale-operation checks. The supervisor associates the process with the
operation. `role` selects the prompt and schemas. No operation ID is added to
events, matching ticket #24.

`key_file` and `worktree` are absolute paths. The worker changes its working
directory to `worktree` before running the role. It reads and strips the key
file at runtime, after validating the inputs. It gives only `inputs` to the
model. Neither a key nor a key path enters the role prompt. These roles have no
host filesystem tools. Deployment owns filesystem and network isolation.

Models use `langchain_openai.ChatOpenAI` with Chat Completions explicitly
selected, pointed at `base_url`. `name` passes through unchanged. `effort` maps
to `reasoning_effort` and can be null to omit it. `budget_tokens` is the positive
per-response `max_completion_tokens`, including model reasoning where the
provider supports it. It is not a total run budget. The proxy decides supported
model names and effort values. Calls stream tokens. Provider retries are off,
so failures return to the supervisor rather than replaying a model call here.

The worker writes only flushed newline-delimited JSON to stdout. A completed
model turn produces `model_turn`, and a tool result produces `tool_result`.
An excluded tool's rejection also counts as a tool result. Token chunks,
startup, middleware hooks and elapsed time do not produce progress events.

```json
{"event":"progress","unit":"model_turn"}
{"event":"reply","role":"ingest","reply":{"kind":"brief","brief":"Fix the greeting.","goals":[{"description":"Return hello.","verification":"test"}],"memory_ids":[]}}
```

Exactly one reply ends a successful run, followed by exit 0. Any other exit is
a failed run, even if progress was emitted. Schema failures never emit a reply.
Malformed JSON, duplicate keys, non-finite numbers, invalid requests, missing
keys, provider errors and invalid replies exit 1. Diagnostics go to stderr as a
fixed request or run failure, without provider exception text or tracebacks that
might contain a secret or model content. Library stdout is redirected to stderr.
A failure does not produce an error event. CLI argument errors use argparse's
exit 2. The supervisor owns start, stop, liveness, checkpoints, reconciliation
and retries. This package does not implement #9.

## Schemas and role payloads

All schemas are plain JSON Schema draft 2020-12 in
[`src/tl3_workers/schemas`](src/tl3_workers/schemas). They ship in the wheel.
The files are the single source for validation. Python loads the same documents
through an offline reference registry. A Rust consumer must register those
files under their `$id` URIs before resolving references. The `.invalid` URIs
are identifiers, never network locations to fetch.

| Schema | Payload |
|---|---|
| `request.json` | The process start document above. |
| `ingest-input.json` | Required `issue` and `repo`, optional `memories` and `audit_objections`. |
| `ingest-reply.json` | A brief with goals, or an early end with a reason. |
| `audit-input.json` | Required `issue`, `repo` and `ingest`, optional `memories`. |
| `audit-reply.json` | Agreement with no objections, or disagreement with at least one objection. |

Every issue has `number`, `title`, `body`, `author` and `comments`. Each comment
has `author` and `body`. Empty bodies and empty comment lists are valid.
Repository material requires `readme` and `file_list`. Optional `materials`
contains `path` and `content` pairs. These are supplied text, never paths for the
worker to fetch. Optional memories contain `id` and `content`. Ingest retries
can receive an `audit_objections` list. Fetching and Hindsight recall happen
outside these roles. An upstream outage must not be represented as empty data.

Ingest returns one of these objects:

```json
{
  "kind": "brief",
  "brief": "Fix the greeting while keeping the interface.",
  "goals": [
    {"description": "Return hello.", "verification": "test"},
    {"description": "Keep the interface clear.", "verification": "review"}
  ],
  "memory_ids": ["memory-1"]
}
```

```json
{"kind":"early_end","outcome":"duplicate","reason":"Issue 2 requests the same fix."}
```

A brief requires at least one goal. Each goal's `verification` is `test` or
`review`. The prompt asks ingest to check its reading against the original
issue. The reply records a proposed verification method, not a claim that tests
ran. `memory_ids` lists supplied memories used. Early outcomes are exactly
`duplicate`, `already_done` and `not_actionable`. Independent audit is required
before the controller acts on an early end.

The brief has a schema limit of 80,000 Unicode characters, an approximation of
20,000 tokens at four characters per token. Actual counts vary by tokenizer
and language. The prompt also requests at most about 20,000 tokens. This is a
brief limit, not request sizing. The reply contains no request size, forecast
or credit. The controller's forecaster in #11 computes those later.

Audit receives the original issue and repository material plus either ingest
reply as `inputs.ingest`. It returns one of these objects:

```json
{"verdict":"agree","objections":[]}
{"verdict":"disagree","objections":["The issue also asks for punctuation."]}
```

Agreement with objections and disagreement without objections both fail the
schema. The schemas validate shape. Prompts and independent audit check
meaning, including evidence, scope and resistance to instructions embedded in
issue or repository text. The worker does not decide controller transitions.

## Deep Agents profile

The implementation inspected PyPI and the wheel source for
[Deep Agents 0.7.32](https://pypi.org/project/deepagents/0.7.32/), especially
`graph.py`, `profiles/harness/harness_profiles.py`, `middleware/filesystem.py`
and `middleware/_tool_exclusion.py`. It also inspected LangChain OpenAI 1.7.3
and LangChain Core 1.7.3 source for model settings and fake models. Dependencies
are locked in `uv.lock`.

Each role has a TL3-written system prompt in `src/tl3_workers/prompts/`.
`HarnessProfile` uses an empty base prompt and suffix, disables the default
general-purpose sub-agent, overrides built-in tool descriptions, and excludes
all built-in tools. Both roles declare an empty tool set. No sub-agents, skills,
external memory middleware or host filesystem backend are installed.
Summarization, tool-call patching, provider prompt caching and unsupported
content middleware are removed. The internal parent notifier remains inert
without an async parent.

Deep Agents forbids removing filesystem middleware and requires `read_file` in
its filesystem allowlist. TL3 keeps that middleware with an in-memory
`StateBackend`, disables input and tool-result offloading, and excludes
`read_file` at both model and dispatch boundaries. Its tool is registered
internally but unavailable to the agent. This profile is not an OS sandbox.
The profile is registered for the configured model before graph creation.
The ChatOpenAI subclass reports the `tl3_proxy` profile namespace, so built-in
model profiles cannot add their own instructions or tools. The provider request
still uses the original model name and the OpenAI-compatible endpoint.
Tests inspect the final model request to catch any injected prompts or tools.

The worker asks for a JSON object directly and validates it against the schema.
It does not use structured-output tool calls or model repair loops, so no
implicit output tool or extra model turn can change the declared role tools.

## Verification

`tests/worker_process.py` starts the actual module entry point in a child
process, writes the start document, closes stdin, and collects events and exit
status. The test launcher replaces only model construction with LangChain's
`FakeMessagesListChatModel`, using scripted messages. It is not a production
protocol field, CLI flag or environment switch. The fake never reads a key.
Tests deny socket connections and DNS in both parent and child. All key files
created by tests contain fixture text.

Run from a clean commit through PrismaBuild. For example, from the repo root:

```bash
python3 /mnt/shared/prismabuild-fleet/repo/tools/pbrun.py \
  --cwd "$PWD/workers" --anywhere --cpus 2 --demand mem_gb=4 --priority -10 \
  --env PATH=/home/rob/.local/bin:/usr/local/bin:/usr/bin:/bin \
  --env OMP_NUM_THREADS=1 --env MKL_NUM_THREADS=1 --env OPENBLAS_NUM_THREADS=1 \
  --env UV_CONCURRENT_DOWNLOADS=2 --env UV_CONCURRENT_BUILDS=2 \
  --env CARGO_BUILD_JOBS=2 \
  --env LANGSMITH_TRACING=false --env LANGCHAIN_TRACING_V2=false \
  --detach -- /home/rob/.local/bin/uv run --locked --python 3.14+gil pytest -q
```

The first run downloads dependencies from PyPI before the tests start.
Use the published `pbwait.py` to wait for the action key, then inspect its
terminal record, logs and CAS receipt. An acknowledgement is not a pass.
