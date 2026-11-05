//! #709/#713: full RED matching over Python-produced code-point/token facts.
#![allow(unexpected_cfgs)]
// SEAM(planner-contracts): replace local facts with the shared S4c contract.

/// Python text, including surrogate code points. Producers validate 0..=0x10ffff.
/// No UTF-8 decoding, surrogate joining or display escaping occurs in this core.
pub type PythonText = Vec<u32>;

#[cfg_attr(not(verus_keep_ghost), derive(Clone, Debug, PartialEq, Eq))]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
pub struct Token {
    pub kind: u32,
    pub text: PythonText,
}

#[cfg_attr(not(verus_keep_ghost), derive(Clone, Debug, PartialEq, Eq))]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
pub struct Tokenization {
    pub whitespace_removed_key: PythonText,
    pub tokens: Vec<Token>,
}

#[cfg_attr(not(verus_keep_ghost), derive(Clone, Debug, PartialEq, Eq))]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
pub struct RewrittenAssertion {
    pub has_explicit_msg: bool,
    pub segment: Option<Tokenization>,
}

#[cfg_attr(not(verus_keep_ghost), derive(Clone, Debug, PartialEq, Eq))]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
pub struct Declaration {
    pub text: PythonText,
    pub tokenization: Option<Tokenization>,
}

#[cfg_attr(not(verus_keep_ghost), derive(Clone, Debug, PartialEq, Eq))]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
pub struct RawRedFacts {
    pub reason_raw: PythonText,
    pub assertion: Option<RewrittenAssertion>,
    pub declared: Option<Declaration>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
pub enum RedMatch {
    Message,
    SourceAssertion,
    Mismatch,
}

#[cfg_attr(not(verus_keep_ghost), derive(Clone, Debug, PartialEq, Eq))]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
pub struct RedDecision {
    pub reason: PythonText,
    pub matched: Option<RedMatch>,
}

#[cfg(verus_keep_ghost)]
use vstd::prelude::*;
#[cfg(verus_keep_ghost)]
#[path = "../verus/red_match_spec.rs"]
pub mod spec;
#[cfg(verus_keep_ghost)]
use spec::*;

#[cfg(not(verus_keep_ghost))]
macro_rules! red_while {
    (($condition:expr) [$($contract:tt)*] $body:block) => { while $condition $body };
}
#[cfg(verus_keep_ghost)]
macro_rules! red_while {
    (($condition:expr) [$($contract:tt)*] $body:block) => {
        vstd::prelude::verus_exec_expr! { while $condition $($contract)* $body }
    };
}
#[cfg(not(verus_keep_ghost))]
macro_rules! red_proof {
    ($($body:tt)*) => {};
}
#[cfg(verus_keep_ghost)]
macro_rules! red_proof {
    ($($body:tt)*) => { vstd::prelude::verus_exec_expr! { proof { $($body)* } } };
}

mod equality {
    use super::*;
    /// #709/#713: equality reads every code point, including spaces and surrogates.
    /// Domain: arbitrary finite u32 sequences, a superset of Python code points.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == (a@ == b@)))]
    pub(super) fn text_equal(a: &PythonText, b: &PythonText) -> bool {
        if a.len() != b.len() {
            return false;
        }
        let mut i = 0;
        red_while! { (i < a.len()) [
            invariant a@.len() == b@.len(), i <= a@.len(),
                forall|j: int| 0 <= j < i ==> a@[j] == b@[j],
            decreases a.len() - i,
        ] {
            if a[i] != b[i] {
                return false;
            }
            i += 1;
        } }
        red_proof! { assert(a@ =~= b@); }
        true
    }

    /// #836 / Q27: source equality includes the raw key, token types and token text.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == lexed_equal(a, b),
))]
    pub(super) fn tokens_equal(a: &Tokenization, b: &Tokenization) -> bool {
        if !text_equal(&a.whitespace_removed_key, &b.whitespace_removed_key)
            || a.tokens.len() != b.tokens.len()
        {
            return false;
        }
        let mut i = 0;
        red_while! { (i < a.tokens.len()) [
            invariant i <= a.tokens@.len(), a.tokens@.len() == b.tokens@.len(),
                a.whitespace_removed_key@ == b.whitespace_removed_key@,
                forall|j: int| 0 <= j < i ==> token_equal(a.tokens@[j], b.tokens@[j]),
            decreases a.tokens.len() - i,
        ] {
            if a.tokens[i].kind != b.tokens[i].kind
                || !text_equal(&a.tokens[i].text, &b.tokens[i].text) {
                return false;
            }
            i += 1;
        } }
        true
    }
}
mod normalization {
    use super::*;
    #[cfg(verus_keep_ghost)]
    vstd::prelude::verus! {
    /// #836: copying a prefix without replacement preserves every code point.
    proof fn prefix_identity(s: Seq<u32>, end: int)
        requires 0 <= end <= s.len(),
        ensures normalized_prefix(s, end, false) == s.take(end),
        decreases end,
    {
        if end > 0 {
            prefix_identity(s, end - 1);
            assert(s.take(end) =~= s.take(end - 1) + seq![s[end - 1]]);
        }
    }
    }

    /// #836: match the literal eight-code-point pytest explanation boundary.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures out == boundary_at(s@, i as int)))]
    fn is_boundary(s: &PythonText, i: usize) -> bool {
        if s.len() < 8 || i > s.len() - 8 {
            return false;
        }
        s[i] == 10
            && s[i + 1] == 97
            && s[i + 2] == 115
            && s[i + 3] == 115
            && s[i + 4] == 101
            && s[i + 5] == 114
            && s[i + 6] == 116
            && s[i + 7] == 32
    }

    /// #836: partition at the first boundary, never the last one.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out <= s@.len(), out as int == first_boundary(s@, 0),
))]
    fn partition(s: &PythonText) -> usize {
        let mut i = 0;
        red_while! { (i < s.len()) [
            invariant i <= s@.len(), first_boundary(s@, 0) == first_boundary(s@, i as int),
            decreases s.len() - i,
        ] {
            if is_boundary(s, i) {
                return i;
            }
            i += 1;
        } }
        i
    }

    /// #836: Python's non-overlapping replacement of newline plus two spaces.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out =>
    requires i < s@.len(),
    ensures out == drop_indent_space(s@, i as int),
))]
    fn is_indent_space(s: &PythonText, i: usize) -> bool {
        s[i] == 32
            && ((i >= 1 && i < s.len() - 1 && s[i - 1] == 10 && s[i + 1] == 32)
                || (i >= 2 && s[i - 1] == 32 && s[i - 2] == 10))
    }

    /// #709/#713 / Q27: normalize only recovered explicit assertion messages.
    /// Arbitrary finite lengths; loop variants decrease by one code point.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out@ == normalized(s@, explicit),
))]
    pub fn normalize_reason(s: &PythonText, explicit: bool) -> PythonText {
        let end = if explicit { partition(s) } else { s.len() };
        let replace = explicit && end < s.len();
        let mut out = Vec::new();
        let mut i = 0;
        red_while! { (i < end) [
            invariant i <= end, end <= s@.len(),
                end as int == (if explicit { first_boundary(s@, 0) } else { s@.len() as int }),
                replace == (explicit && end < s@.len()),
                out@ == normalized_prefix(s@, i as int, replace),
            decreases end - i,
        ] {
            if !replace || !is_indent_space(s, i) {
                out.push(s[i]);
            }
            i += 1;
        } }
        red_proof! {
            if !replace { prefix_identity(s@, end as int); }
        }
        out
    }
}
mod matching {
    use super::*;
    /// #709/#713: a missing assertion, explicit message or lexer failure blocks fallback.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == source_match(assertion, declared),
))]
    fn matches_source(assertion: &Option<RewrittenAssertion>, declared: &Declaration) -> bool {
        match assertion {
            Some(a) => {
                if a.has_explicit_msg {
                    return false;
                }
                match (&a.segment, &declared.tokenization) {
                    (Some(segment), Some(declaration)) => {
                        equality::tokens_equal(segment, declaration)
                    }
                    _ => false,
                }
            }
            None => false,
        }
    }

    /// #836 / Q27: normalize complete text, then MESSAGE, then source, then mismatch.
    /// Missing declaration is None. No display value enters this function.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out.reason@ == normalized(facts.reason_raw@, explicit_assertion(facts.assertion)),
    out.matched == decision(facts, out.reason@),
))]
    pub fn decide(facts: &RawRedFacts) -> RedDecision {
        let explicit = match &facts.assertion {
            Some(a) => a.has_explicit_msg,
            None => false,
        };
        let reason = normalize_reason(&facts.reason_raw, explicit);
        let matched = match &facts.declared {
            None => None,
            Some(declared) => {
                if equality::text_equal(&reason, &declared.text) {
                    Some(RedMatch::Message)
                } else if matches_source(&facts.assertion, declared) {
                    Some(RedMatch::SourceAssertion)
                } else {
                    Some(RedMatch::Mismatch)
                }
            }
        };
        RedDecision { reason, matched }
    }
}
pub use matching::decide;
pub use normalization::normalize_reason;
