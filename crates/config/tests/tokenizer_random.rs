#[path = "tokenizer/support.rs"]
mod support;

#[test]
fn fixed_random_unicode_scalar_corpus_matches_python() {
    support::compare("random");
}
