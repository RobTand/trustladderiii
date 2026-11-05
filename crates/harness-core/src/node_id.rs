//! Pytest selector admission and collection matching (#824, #717).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Exact,
    SelectedFile,
    AlwaysTest,
}

struct Rules {
    require_names: bool,
    reject_empty_path_segments: bool,
    reject_dot_path_segments: bool,
    reject_path_metacharacters: bool,
    reject_control_characters: bool,
}

impl Mode {
    const fn rules(self) -> Rules {
        match self {
            Self::Exact => Rules {
                require_names: true,
                reject_empty_path_segments: false,
                reject_dot_path_segments: false,
                reject_path_metacharacters: false,
                reject_control_characters: false,
            },
            Self::SelectedFile => Rules {
                require_names: false,
                reject_empty_path_segments: false,
                reject_dot_path_segments: false,
                reject_path_metacharacters: false,
                reject_control_characters: false,
            },
            Self::AlwaysTest => Rules {
                require_names: false,
                reject_empty_path_segments: true,
                reject_dot_path_segments: true,
                reject_path_metacharacters: true,
                reject_control_characters: true,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParseError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeId<'a> {
    raw: &'a str,
    path_end: usize,
}

impl<'a> NodeId<'a> {
    /// #824/#717: admit exactly the existing grammar for this boundary's mode.
    pub fn parse(raw: &'a str, mode: Mode) -> Result<Self, ParseError> {
        let rules = mode.rules();
        let (path, names) = match raw.split_once("::") {
            Some((path, names)) => (path, Some(names)),
            None => (raw, None),
        };
        if !path.ends_with(".py")
            || path.starts_with(['/', '-'])
            || path.contains('\\')
            || raw.contains('\0')
            || rules.require_names && names.is_none()
            || names.is_some_and(|names| names.split("::").any(str::is_empty))
            || path.split('/').any(|part| {
                part == ".."
                    || rules.reject_empty_path_segments && part.is_empty()
                    || rules.reject_dot_path_segments && part == "."
            })
            || rules.reject_path_metacharacters && path.contains([':', '*', '?', '[', ']'])
            || rules.reject_control_characters
                && raw
                    .chars()
                    .any(|ch| (ch as u32) < 32 || (127..=159).contains(&(ch as u32)))
        {
            return Err(ParseError);
        }
        Ok(Self {
            raw,
            path_end: path.len(),
        })
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

    /// #824: apply an admitted selector to raw pytest collection facts.
    pub fn selects(self, collected: &str) -> bool {
        if self.raw == collected {
            return true;
        }
        let Some(suffix) = collected.strip_prefix(self.raw) else {
            return false;
        };
        self.path_end == self.raw.len() && suffix.starts_with("::")
            || !self.raw.contains('[') && suffix.starts_with('[') && suffix.ends_with(']')
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

/// #824: the existing trusted-setting boundary, including error precedence.
pub fn parse_always_tests(tests: &serde_json::Value) -> Result<Vec<NodeId<'_>>, AlwaysTestsError> {
    let tests = tests.as_array().ok_or(AlwaysTestsError::Shape)?;
    if tests.len() > MAX_ALWAYS_TESTS {
        return Err(AlwaysTestsError::TooMany);
    }
    tests
        .iter()
        .enumerate()
        .map(|(index, test)| {
            let raw = test.as_str().ok_or(AlwaysTestsError::NonString)?;
            NodeId::parse(raw, Mode::AlwaysTest).map_err(|_| AlwaysTestsError::InvalidEntry(index))
        })
        .collect()
}
