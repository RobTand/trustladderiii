#[path = "tokenizer/support.rs"]
mod support;

#[test]
fn code_unicode_whitespace_and_adversarial_bpe_match_python() {
    support::compare("curated");
}

#[test]
fn missing_and_corrupt_cache_are_rejected_without_a_downloader() {
    let directory = tempfile::tempdir().unwrap();
    assert!(
        support::load(directory.path())
            .err()
            .unwrap()
            .contains("missing pinned BPE")
    );
    std::fs::write(
        directory
            .path()
            .join("fb374d419588a4632f3f557e76b4b70aebbca790"),
        b"bad cache",
    )
    .unwrap();
    assert!(
        support::load(directory.path())
            .err()
            .unwrap()
            .contains("sha256 mismatch")
    );
}
