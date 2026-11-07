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
