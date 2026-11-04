# Harness parity vectors

`contracts/` pins the check-report shape from #734. `pytest-node-ids.json`
is the original grammar and selection corpus. Both remain unchanged.

`core/<kind>.json` pins Python behavior before wave B. Each file contains a
`kind` and `cases`, each with a stable `id`, `input`, and `expected` value.
The generator calls the production functions. It does not implement their
rules. Regenerate with Python 3.14 and pytest installed:

```sh
python harness/tests/core_vectors.py
python harness/tests/core_vectors.py --check
```

Run checks on BuildBuddy, following `docs/bazel.md`. Run generation on an
admitted PrismaBuild worker. `--output DIR` can write to a shared staging
directory for copying the generated files back to the checkout. Commit the
outputs after an intended behavior change. The Python test names the kind
and case on drift, following the `contracts/` generator/check pattern.

A Python string containing any surrogate code point is represented as an
object `{"python_codepoints": [120, 55296, 121]}`. This means the three code
points `x`, U+D800, `y`, rather than JSON's invalid lone-surrogate escape.
Other strings are ordinary JSON strings. The wrapper applies recursively to
inputs and outputs. Rust consumers must retain this distinction when they
model Python strings. It also keeps the whole corpus readable by serde_json.

Boundary inputs use `char` and `count` to avoid multi-megabyte fixtures. Expand
them before calling `validate_plan_size`. `collection_bytes` counts the exact
`sort_keys=True, ensure_ascii=False` UTF-8 output with backslash replacement
and a final newline. Summary `json_bytes` instead counts default Python JSON,
including ASCII escapes and spaces. These encodings are intentionally different.

The node-ID grammar cases distinguish exact checks, selected-regression file
selectors, and repository `always_tests`. Selection cases call `selects`.
Judge cases enumerate Kind × Outcome² and all forced Outcome³ combinations,
then add missing or mismatched RED-evidence cases. RED cases use pytest's real
assertion rewriting and the production capture function. Phase cases feed
captured tuples to the real `Reporter.pytest_runtest_logreport` fold.

Wave B entry points are each case's `input` and `expected`. Rust currently
consumes the exact node-ID grammar in `reproducing.rs` and verifies that all
seven files parse with serde_json. The other behavior consumers
are unwritten until their ports exist, as #749 permits. No failing stub or
manual Bazel target is added. This corpus adds no production implementation,
proof or test pruning. Glob parity belongs to the later selection/config port;
this ticket's seven kinds do not port path matching. Unicode `\w` is covered
by forcing targets; none of these current functions uses a `\d` regex.

The checker compares canonical JSON encodings, including numeric types.
Changing `1.0` to `1`, or either number to a boolean, is drift. All mismatched
case IDs are reported together.

`plan-size-collection` expands `count` distinct node IDs with empty parameter
labels, then distributes enough ASCII padding across their labels to reach
`total_bytes`. Its `largest_phase_bytes` fact stays below the per-phase limit,
so the aggregate collection guard alone decides these boundary cases.

The coverage revision also pins file-prefix selection boundaries, exactly 20
`always_tests`, forbidden path characters, Python's rejection of combining
marks and join controls in `\w`, U+001F trimming, and complete long or multiline
RED messages.
