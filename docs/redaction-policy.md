# Redaction policy

`crates/controller/src/redaction/source.rs` defines the policy. Its checked
patterns use a Rust enum, so adding a check requires an exhaustive dispatch arm.
`unicode.rs` freezes the Python 3.14 Unicode 16.0 character ranges used by the
Rust regex compatibility layer. The matching expressions retain Python syntax,
including lookarounds, backreferences, and possessive quantifiers. The adapter
expands Python's four-way I/i/İ/ı case equivalence in literals and classes, tracks
scoped case/multiline flags, and translates strict `\Z` and soft `$` anchors.

Generate both committed consumers with the built controller CLI:

```sh
tl3 redaction-policy --output crates/controller/src/redaction/policy.json
tl3 redaction-policy --output workers/src/tl3_workers/policy.json
tl3 redaction-policy --identity-output workers/src/tl3_workers/_policy_identity.py
```

Check either file with `tl3 redaction-policy --check FILE`. The controller's
Gate tests check both files against the Rust serialization. Python loads the
packaged `policy.json` beside `tracing.py`; it defines no policy expressions.
The generated `_policy_identity.py` pins the schema version, exact file SHA-256,
required field names, rule counts, ordered check names, and scalar values to the
worker build. Every worker package import verifies the hash and structure before
matching. A stale, missing, empty, or malformed policy aborts worker startup.
Check the identity with `tl3 redaction-policy --identity-check FILE`.

G0 keeps Python's span matching inside the exporter process. Controller output
uses Rust matching. The shared matrix, corpus, planted forms, and a seeded
proptest compare exact output. The migration test also fingerprints every
original matching input, excluding the former Python source-file hash.
Existing tracing tests cover structured carriers, truncation boundaries, and
exporter behavior.

`tl3 redact` reads one UTF-8 span from stdin and writes its redacted text to
stdout. It shares the controller engine and its one-MiB input bound.
`workers/tools/redaction_benchmark.py` compares fresh CLI calls with in-process
Python on planted spans. Run the benchmark through PrismaBuild as a paired
measurement. Each fresh process rebuilds the Unicode-expanded compiled regexes. This accounts
for the measured roughly 396 ms per span; it is not a `tl3` startup measurement.
An in-process PyO3 redactor can reuse those compiled expressions in a later wave.
