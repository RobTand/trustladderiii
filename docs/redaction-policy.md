# Redaction policy

`crates/controller/src/redaction/source.rs` defines the policy. Its checked
patterns use a Rust enum, so adding a check requires an exhaustive dispatch arm.
`unicode.rs` freezes the Python 3.14 Unicode 16.0 character ranges used by the
Rust regex compatibility layer. The matching expressions retain Python syntax,
including lookarounds, backreferences, and possessive quantifiers.

Generate both committed consumers with the built controller CLI:

```sh
tl3 redaction-policy --output crates/controller/src/redaction/policy.json
tl3 redaction-policy --output workers/src/tl3_workers/policy.json
```

Check either file with `tl3 redaction-policy --check FILE`. The controller's
Gate tests check both files against the Rust serialization. Python loads the
packaged `policy.json` beside `tracing.py`; it defines no policy expressions.
A missing or invalid policy prevents the tracing module from loading.

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
measurement. CLI process startup and regex compilation are included in its
per-span time.
