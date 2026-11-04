//! Offline Verus process boundary for #770.

fn check_summary(_output: &str, _expected: u64) -> Result<u64, String> {
    Err("Verus summary validation not implemented".into())
}

fn main() {}

#[cfg(test)]
mod tests {
    use super::check_summary;

    #[test]
    fn accepts_exact_positive_obligation_count() {
        assert_eq!(check_summary("verification results:: 2 verified, 0 errors\n", 2), Ok(2));
    }

    #[test]
    fn rejects_zero_missing_duplicate_and_wrong_counts() {
        for (output, expected) in [
            ("verification results:: 0 verified, 0 errors\n", 1),
            ("nothing ran", 1),
            ("verification results:: 2 verified, 0 errors\n", 1),
            ("verification results:: 2 verified, 1 errors\n", 2),
            ("verification results:: 2 verified, 0 errors\n", 0),
            ("verification results:: 2 verified, 0 errors\nverification results:: 2 verified, 0 errors\n", 2),
        ] {
            assert!(check_summary(output, expected).is_err(), "{output}");
        }
    }
}
