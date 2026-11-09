# Redaction policy

`crates/redaction/src/source.rs` defines the policy. Its checked
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

`tl3-redaction` is the authoritative policy and matcher. The controller uses it
with error conversion; the action uses its structured redactor for Python API
callers through a subprocess. Python defines no second worker policy.

The generated harness policy and minimal `Redactor.window` dependency closure
remain solely for frozen supervision until slice 6, after "Flux executor
verified". Runtime, supervision and staging are unchanged in slice 1.

```sh
tl3 redaction-policy --check crates/redaction/src/policy.json
tl3 redaction-policy --harness-check harness/src/tl3_harness/_redaction_policy.py
```

Rust tests generate synthetic credentials at runtime from the committed fixed
seeds. `TL3FAKE` marks values where their format permits. The committed corpus
contains generator inputs only, and production detection checks every decoded
JSON string to prevent encoded secrets from entering it. Runtime values drive
secret-fragment assertions across text, nested JSON, tool calls, sensitive files,
Unicode and truncation boundaries. The controller keeps independent Python
`re` syntax, class, flag and Unicode boundary checks. Python production-redactor
oracles and worker policy identity artifacts have been removed.
