//! #751/#760: extraction equivalence under changing I/O and crashes.
#[path = "mergify_io/mod.rs"]
mod mergify_io;
pub use mergify_io::{Error, Result, decode_stored, executor};

pub use mergify_io::{evidence, snapshot};
pub use tl3_controller::pr_body;
pub mod findings {
    pub use tl3_controller::findings::*;
    pub const MAX_REPORT_CHARS: usize = 16000;
    pub fn clipped(t: &str, n: usize) -> String {
        t.chars().take(n).collect()
    }
}
pub mod memory {
    use regex::Regex;
    use std::sync::LazyLock;
    pub(crate) fn redact_text(text: &str) -> String {
        // Like #302's Redactor, accept quotes and whitespace escaped by nested JSON.
        static SECRET: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(
        r#"(?is)(?:bearer\s+\S+|-----BEGIN[^\r\n]*PRIVATE KEY-----.*?(?:-----END[^\r\n]*PRIVATE KEY-----|$)|\b(?:sk-|gh[pousr]_|github_pat_|hf_|glpat-|xox[a-z]-)[a-zA-Z0-9_-]+|AKIA[A-Z0-9]{16}|\b[a-fA-F0-9]{32,}\b|eyJ[a-zA-Z0-9_-]+\.[a-zA-Z0-9_-]+\.[a-zA-Z0-9_-]+|(?:https?|postgres(?:ql)?)://[^/\s:@]+:[^/\s@]+@|(?:[a-z0-9_]*_api_key|[a-z0-9_]*_secret[a-z0-9_]*|[a-z0-9_]*_token|api[_-]?key|password|passwd|secret|authorization|cookie)\\*["']?(?:\s|\\+[nrt])*[=:](?:\s|\\+[nrt])*(?:(?:bearer|basic)\s+)?(?:\\+"[^"\r\n]*"|"[^"]*"|\\*'[^']*'|\S+)|\btoken\\*["']?(?:\s|\\+[nrt])*=(?:\s|\\+[nrt])*(?:(?:bearer|basic)\s+)?(?:\\+"[^"\r\n]*"|"[^"]*"|\\*'[^']*'|\S+)|\btoken\\*["']?(?:\s|\\+[nrt])*:(?:\s|\\+[nrt])*(?:\\*["'][^"']+["']|[a-zA-Z0-9_-]*[0-9_-][a-zA-Z0-9_-]*))"#
    ).expect("memory secret pattern")
        });
        let text = text.replace('\0', "");
        SECRET.replace_all(&text, "[redacted]").into_owned()
    }
}
