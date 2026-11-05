//! #709/#713: full RED matching over Python-produced code-point/token facts.
// SEAM(planner-contracts): replace local facts with the shared S4c contract.

/// Python text, including surrogate code points. Producers validate 0..=0x10ffff.
/// No UTF-8 decoding, surrogate joining or display escaping occurs in this core.
pub type PythonText = Vec<u32>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: u32,
    pub text: PythonText,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tokenization {
    pub whitespace_removed_key: PythonText,
    pub tokens: Vec<Token>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RewrittenAssertion {
    pub has_explicit_msg: bool,
    pub segment: Option<Tokenization>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Declaration {
    pub text: PythonText,
    pub tokenization: Option<Tokenization>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawRedFacts {
    pub reason_raw: PythonText,
    pub assertion: Option<RewrittenAssertion>,
    pub declared: Option<Declaration>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RedMatch {
    Message,
    SourceAssertion,
    Mismatch,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RedDecision {
    pub reason: PythonText,
    pub matched: Option<RedMatch>,
}

/// #836 / Q27: pure counterpart of red_message.capture, before display clipping.
pub fn decide(_facts: &RawRedFacts) -> RedDecision {
    todo!("#836: implement normalization and RED matching")
}
