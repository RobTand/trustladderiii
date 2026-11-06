pub(crate) fn clipped(text: &str, cap: usize) -> String {
    text.chars().take(cap).collect()
}
