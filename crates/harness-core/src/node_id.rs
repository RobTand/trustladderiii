//! Pytest selector admission and collection matching (#824, #717).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Exact,
    SelectedFile,
    AlwaysTest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParseError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeId<'a> {
    raw: &'a str,
    path_end: usize,
}

impl<'a> NodeId<'a> {
    pub fn parse(_raw: &'a str, _mode: Mode) -> Result<Self, ParseError> {
        todo!("#824 parser")
    }

    pub fn as_str(self) -> &'a str {
        self.raw
    }

    pub fn path(self) -> &'a str {
        &self.raw[..self.path_end]
    }

    pub fn names(self) -> impl Iterator<Item = &'a str> {
        self.raw.split("::").skip(1)
    }

    pub fn selects(self, _collected: &str) -> bool {
        todo!("#824 collection matching")
    }
}

pub const MAX_ALWAYS_TESTS: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlwaysTestsError {
    Shape,
    TooMany,
    NonString,
    InvalidEntry(usize),
}

pub fn parse_always_tests(_tests: &serde_json::Value) -> Result<Vec<NodeId<'_>>, AlwaysTestsError> {
    todo!("#824 always-tests boundary")
}
