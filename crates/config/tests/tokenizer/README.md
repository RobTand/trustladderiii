# o200k_base parity for wave D

These tests evaluate a tokenizer for #748. They do not change forecast or any
production caller. `tl3-config` hosts the dev-dependency because the ticket
forbids a new Cargo package. The tokenizer is unrelated to its public API.

`tiktoken-rs` 0.12.0 exposes `CoreBPE::new` and the o200k regex. The test loader
reads the same SHA-1-named cache entry as `forecast.optional_encoding`, checks
SHA-256 `446a9538cb6c348e3516120d7c08b09f57c36495e2acfffe59a5bf8b0cfb1a2d`,
and supplies those ranks directly. It never calls the crate's embedded
`o200k_base()` constructor. There is no download path in the wrapper.
Bazel supplies the existing SHA-256-pinned `@o200k_base//file` input and its
runtime prepares `TIKTOKEN_CACHE_DIR`. Cargo users must preload that cache.

Alternatives considered:

- [bpe-openai](https://github.com/github/rust-gems/tree/main/crates/bpe-openai)
  supports o200k and fast counting, but its standard API loads serialized,
  build-generated static tokenizers. `CoreBPE::new` makes testing the worker's
  exact runtime cache file simpler.
- [Hugging Face tokenizers](https://github.com/huggingface/tokenizers) has generic
  BPE support. Matching tiktoken also requires its regex pre-tokenizer and
  ordinary-token handling. `tiktoken-rs` already exposes both.

`oracle.json` records Python tiktoken 0.14.0 counts, input byte lengths and
SHA-256 hashes. Repeated inputs use a compact recipe. The generator invokes
the actual worker's `optional_encoding` definition in isolation, so generation
uses its regex and offline pin check without loading the agent framework.
Python is retained only as the reference oracle for the language migration.

Regenerate inside admitted PrismaBuild compute, with the pinned cache supplied:

```sh
uv run --no-project --python 3.14 --with tiktoken==0.14.0 \
  python crates/config/tools/tokenizer_oracle.py --benchmark
```

The corpus includes code in Rust, Python, TypeScript, Go, SQL and shell;
normalization forms, emoji sequences, Unicode case and whitespace, control
characters and literal special tokens; repetitive BPE cases; and the original
`test_planner_forecast` boundary/Unicode inputs and `test_large_files`
`ARCHITECTURE`/`POOL` fixtures. The 2,048 random cases use seed 748, alternating
mixed text with arbitrary Unicode scalar values. They are committed once
rather than starting a Python subprocess for every Rust proptest example.
Lone Python surrogates cannot enter Rust `str` and are outside this corpus.

Three Bazel targets keep the corpus independently runnable:

```text
//crates/config:test_test_tokenizer_curated
//crates/config:test_test_tokenizer_random
//crates/config:test_test_tokenizer_large
```

The comparison reports every mismatching case, both counts, the signed delta,
the maximum absolute delta and whether Rust counts bound Python counts above.
Equality is required. A corpus pass is empirical evidence, not a proof for all
strings. No Kani proof is claimed for regex/BPE behavior.

The large-file test prints five-call median times after initialization and an
untimed parity pass. Python's generator prints the same measurement, including
the exact-boundary file, architecture document and numbered Python module.
Record build profile and compute platform when comparing these values. These
are observations with no speed threshold in CI.
