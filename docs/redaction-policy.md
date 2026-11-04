# Redaction policy

`crates/controller/src/redaction/source.rs` defines the policy. Its checked
patterns use a Rust enum, so adding a check requires an exhaustive dispatch arm.
`unicode.rs` freezes the Python 3.14 Unicode 16.0 character ranges used by the
Rust regex compatibility layer. The matching expressions retain Python syntax,
including lookarounds, backreferences, and possessive quantifiers. The adapter
parses the complete expression with fancy-regex, then parses character classes
with regex-syntax. Folding changes Unicode sets before negation; HIR serialization
preserves literal hyphens and class boundaries. The parsed flags preserve scoped
case/multiline behavior, and the adapter translates strict `\Z` and soft `$`
anchors. Generation compiles every expression and aborts on unsupported syntax.

Before parsing, `grammar.rs` validates an explicit Python lexical subset without
editing the source. It accepts `i/m/s` flags, ASCII capture names and named
backreferences, standard character escapes, and fixed-width `\x/\u/\U` scalar
escapes. Verbose mode, numeric escapes/backreferences, named Unicode escapes,
`\z`, comment groups, and other parser extensions are outside the subset.
Literal U+000B/U+000C must use escaped forms. Unsupported tokens fail with their
construct name and byte offset before fancy-regex can reinterpret them.
The CI Python oracle independently calls `re.compile` on all 90 generated
expressions, including every helper, and checks an invalid unused-helper mutant.

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
An eight-shard generated conformance target mutates planted seeds with each
expression's alphabet, every ASCII delimiter, whitespace, hyphens, and special
Unicode folds. It compares both full production redactors. Every class occurrence
also gets in-class and adjacent boundary witnesses under both case modes.
Existing tracing tests cover structured carriers, truncation boundaries, and
exporter behavior.

`tl3 redact` reads one UTF-8 span from stdin and writes its redacted text to
stdout. It shares the controller engine and its one-MiB input bound.
`workers/tools/redaction_benchmark.py` compares fresh CLI calls with in-process
Python on planted spans. Run the benchmark through PrismaBuild as a paired
measurement. Each fresh process rebuilds the Unicode-expanded compiled regexes. This accounts
for the measured roughly 396 ms per span; it is not a `tl3` startup measurement.
An in-process PyO3 redactor can reuse those compiled expressions in a later wave.
