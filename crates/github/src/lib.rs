#![forbid(unsafe_code)]

mod client;
pub mod message;
mod models;
pub mod webhook;

pub use client::{
    API_VERSION, ActionsRefusal, AppConfig, ClientOptions, Error, FailureClass, GitHub,
};
pub use models::*;

/// Local closing references, deduplicated for intake and future catch-up.
pub fn closing_issues(body: &str) -> Vec<u64> {
    static CLOSING: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let expression = CLOSING.get_or_init(|| {
        regex::Regex::new(r"(?i)\b(?:close[sd]?|fix(?:e[sd])?|resolve[sd]?)\s+#([0-9]+)\b")
            .expect("constant closing-reference expression")
    });
    let mut issues: Vec<_> = expression
        .captures_iter(body)
        .filter_map(|capture| capture[1].parse::<u64>().ok())
        .filter(|number| *number > 0)
        .collect();
    issues.sort_unstable();
    issues.dedup();
    issues
}
