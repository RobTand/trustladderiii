# Production measurement modes

The fetched main and rust-fv refs both equal
2a3d6665fb51d1e9df840124d49ca635766c0e3a. Three failed disposable frontend
preparations motivate this change; their historical evidence stays unchanged.

The smallest seam is a committed runner execution mode. Toolchain::new owns
canonical verifier, Z3 and rustc resolution and derives sysroot from the canonical
rustc executable's two parents. Every mode calls the same command builder and
uses the same environment, selection and source attributes. Ordinary proof and
legacy --measure behavior stay compatible. Explicit frontend-only and normal
modes add reporting without replacing runner source. Diagnostic compatibility
is retained separately and cannot silently instrument unsupported modern modes.

Frontend uses --no-verify without --build-sst. Pinned Verus 8bf68ee,
source/rust_verify/src/verifier.rs, verify_crate skips verify_crate_inner when
no_verify is true and build_sst is false. Zero verified obligations, zero errors,
empty SMT measurements and zero SMT resource counters are required independently.
Positive expected counts describe source selection only. FRONTEND_ONLY is never
a proof or resource acceptance verdict.

Normal mode keeps proof validation and adds --time-expanded --output-json.
Resources exclude INIT and retain INIT and function totals separately. The 20s
wall threshold warns; the resource acceptance gate belongs to the caller.
Frontend prerequisites bind exact source, tooling, runner, mode and commands;
normal submission refuses missing or mismatched completed frontend evidence.
Wrapper/scanner/cleanup ownership and pins remain with ops. Live normal
qualification stays UNKNOWN until captain runs the landed exact head.

The current K4 candidate predates the runner capability. It is not an eligible
measurement source for these modes. Captain must assemble reviewed tooling into
an explicit source revision and qualify that exact revision; this producer does
not patch or run the candidate. Tooling provenance names the executing driver
hash separately from measured source hashes. Production runner bytes are always
tracked bytes from the measured source.

Normal and diagnostic submission reaudits the frontend own terminal parent and
scanned CAS evidence. It binds a compact ticket into preparation's exact argv.
The remote command refuses a missing/mismatched ticket before Bazel. Collection
checks raw source/pin/runner identities and actual tools again. Legacy diagnostic
configuration remains available on capable sources with its existing explicit
authorization, timeout and optional query logs. Unsupported older diagnostic
sources require an explicit update; disposable patch fallback is removed.

Own remote runner-test logs exposed an existing collector ambiguity before the
live smoke: BuildBuddy's shell trace includes the pinned wrapper's literal
print statement as quoted Python source. The collector records that exact
non-JSON source line separately. It rejects changed/repeated source echoes and
all embedded, duplicate or conflicting emitted JSON markers. Source text grants
no wrapper verdict or fallback authority. The wrapper and scanner are unchanged.
