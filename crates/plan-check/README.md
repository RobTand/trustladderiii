# Pure planner rules

`tl3-plan-check` contains library rules for the planner migration in #820. It has no controller dependency or runtime callers.

- `paths::full_match` implements CPython 3.14 `PurePosixPath.full_match`. It normalizes `.` and repeated separators, preserves the POSIX double-slash root, and parses bracket classes using `fnmatch._translate` semantics. Its typed pattern feeds the verified NFA evaluator in `paths/decision.rs`.
- `heuristics::{mentions, asks_to_edit}` preserves the planner's sentence splits, Unicode word boundaries, lowercase behavior, verb sets, and seven-word window. These string heuristics have parity and property tests, with no formal proof.
- `prerequisites::check_prerequisites` checks source quotes and distinct kind/number pairs. Its trailing-digit guard uses Python's Unicode decimal category.
- `citations::check_citations` preserves diagnostic precedence and Python whitespace normalization, including the final content for duplicate memory IDs.
- `subjects::{subject_covers, overlaps}` compares byte prefixes at exact `::` boundaries. Coverage and transitivity are verified for arbitrary-length sequences.

`tests/fixtures/p2a.json` contains all 26 P2a glob cases, 50 English cases, and 91 prerequisite/citation calls. It retains source case IDs and the original corpus SHA-256. Repeated recorder metadata is omitted. No tests are deleted.

`tools/vectors.py` reads the real planner heuristic functions through Python's AST and generates 9,336 additional glob cases, 4,000 English cases, and 2,000 subject cases. It also generates the Unicode tables used by production Rust. Python remains here only as the migration oracle.

Run this command on BuildBuddy with CPython 3.14 to regenerate the committed files. Add `--check` to check drift.

```sh
python3.14 crates/plan-check/tools/vectors.py
```

The glob parser follows the algorithms in CPython 3.14's `pathlib`, `glob.translate`, and `fnmatch._translate`, distributed under the Python Software Foundation license. The offered WR1 salvage was inspected but no code was adopted.

The Bazel targets are `test_test_parity`, `test_test_generated`, `verus_paths`, and `verus_subjects` under `//crates/plan-check`. The proof groups include witnesses and have no assumptions, admissions, or external bodies. Neither the corpus nor the proof bounds truncate production inputs. Parser normalization and character-class recognition remain covered by the Python corpus and property tests.
