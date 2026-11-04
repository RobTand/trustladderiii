# Verus proofs

`//crates/mergify:verus_delivery` checks two obligations over a delivery
acknowledgement reference step. The same plain Rust module compiles in Cargo
and in the Verus proof root. Conditional `verus_spec` attributes add contracts
only for the verifier. This model does not prove controller I/O, persistence,
HTTP ordering, fencing, or exactly-once delivery.

`tools/bazel/verus_pins.bzl` pins Verus 0.2026.11.02.8bf68ee, its bundled Z3
4.16.0, and Rust 1.99.1 with rustc-dev and rust-std. The repository rule checks
all archive digests and the Z3 executable digest, then assembles a private
sysroot. It never registers that sysroot as the workspace Rust toolchain.
Archive bytes and the URL/digest manifest are declared proof inputs.

`verus_test` declares a proof root, executable modules, transitive specs, and
an exact positive obligation count. Its runner uses one thread, rlimit 30,
a 45-second process timeout, and a five-second termination grace period.
The test fails on a process error, signal, timeout, missing or repeated summary,
verification errors, zero obligations, or a count different from the declaration.
Verification runs offline on amd64 with the private sysroot and bundled Z3.

Proof targets carry `rust` and `verus` tags and cannot be `manual`. The Gate
selects them through ordinary Bazel dependency impact and classifies their
failures into the required Rust check. Static parity validates proof metadata
separately from Cargo binary inventories. Successful proofs write an
`inventory.json` receipt with their declared and verified obligation counts.
The full amd64 run includes every proof; the arm64 pass excludes Verus targets.
The nightly Verus workflow runs the complete proof inventory on BuildBuddy.

`tools/ci/verus_qualification.py` runs on a disposable BuildBuddy runner.
Its cases check real module/spec impact, unrelated documentation selection,
a seeded step mutant, an empty proof, and a URL-pin change with identical
archive bytes. The last case requires an executed, uncached proof. Scratch
commits remain on the remote runner. The mutant and empty-proof cases also
check that the batch result fails `remote_gate.py aggregate rust`.

Python changes are limited to the existing Bazel/GitHub adapters and remote
qualification orchestration. Verifier execution and summary validation are Rust.
