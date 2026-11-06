# Proof assembly measurement for #902

This package is a disposable experiment based on R1 commit `3eadde59269dd3fb39ea585d340ebfaa5eb81a0d`. Do not merge it. It is outside the Cargo workspace and the Bazel graph.

The test-only `corpus/proof_corpus.py` plugin snapshots assembly inputs and Python outcomes. `corpus/export_bb.py` runs that plugin on BuildBuddy. The frozen JSON lines live in `corpus/data/`; the canonical report and exported data are under `/home/rob/tmp/tl3-build/rust-fv/harness-rs/spike/`.

Submit `corpus/run_pb.py` with `pbrun.py`, four CPUs, eight GiB of memory, Cargo jobs bounded to four, and native library threads bounded to one. Do not run the package or its comparison scripts on the coordinator. The runner builds, runs the frozen corpus, checks Clippy, and emits a compressed evidence archive into its CAS-captured stdout.

`corpus/compare.py` compares verdicts, primary R1 rule ids, receipt objects, and staged-tree digests. It also compares parser normalization, approximate scope facts, and literal shapes. The Rust side receives no Python outcomes inside `assemble`.

The Rust scope model, loader exceptions, and literal evaluator are incomplete. The `--reduced` switch is only a sensitivity experiment that bypasses some support validation. It does not implement #855's runtime guard or symmetric GREEN and must not be used as a validator. Conditional source-size estimates are separate from that switch's results.

Python latency in the export includes filesystem staging on BuildBuddy. Rust latency uses in-memory maps on PrismaBuild. Do not interpret their ratio as a speedup.
