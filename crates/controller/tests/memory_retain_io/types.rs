const SECTION_BYTES: usize = 6000;
const RECORD_BYTES: usize = 32768;

const PER_RUN: usize = 3;
const EVIDENCE_LINES: u64 = 40;
const EVIDENCE_FILE_BYTES: u64 = 1024 * 1024;

const FILE_EXTENSIONS: &[&str] = &[
    "py", "rs", "md", "toml", "json", "yaml", "yml", "ts", "tsx", "js", "jsx", "sh", "txt", "cfg",
    "ini", "lock", "sql", "go", "java", "c", "h", "cc", "cpp", "hpp", "cs", "rb", "html", "css",
    "bzl", "bazel", "proto", "xml", "ipynb",
];

struct Run<'a> {
    role: &'a str,
    repo: &'a str,
    request: &'a str,
    id: &'a str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LessonKind {
    Lesson,
    Decision,
    Convention,
}

/// A durable lesson, decision or convention for later recall.
pub struct Lesson<'a> {
    pub kind: LessonKind,
    pub source: &'a str,
    pub repo: &'a str,
    pub topic: Option<&'a str>,
    pub text: &'a str,
    /// Stable identity, so a replay queues the same remote operation.
    pub identity: &'a str,
}

