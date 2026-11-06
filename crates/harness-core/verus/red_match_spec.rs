use super::*;
use vstd::prelude::*;

verus! {
pub open spec fn boundary_at(s: Seq<u32>, i: int) -> bool {
    0 <= i && i + 8 <= s.len() && s[i] == 10 && s[i+1] == 97
        && s[i+2] == 115 && s[i+3] == 115 && s[i+4] == 101
        && s[i+5] == 114 && s[i+6] == 116 && s[i+7] == 32
}

pub open spec fn first_boundary(s: Seq<u32>, i: int) -> int
    decreases s.len() - i,
{
    if i >= s.len() { s.len() as int }
    else if boundary_at(s, i) { i }
    else { first_boundary(s, i + 1) }
}

pub open spec fn drop_indent_space(s: Seq<u32>, i: int) -> bool {
    s[i] == 32 && ((i >= 1 && i + 1 < s.len() && s[i-1] == 10 && s[i+1] == 32)
        || (i >= 2 && s[i-1] == 32 && s[i-2] == 10))
}

// This is the left-to-right, non-overlapping replace("\n  ", "\n").
// Each removed space is either immediately after a newline or its first space.
pub open spec fn normalized_prefix(s: Seq<u32>, end: int, replace: bool) -> Seq<u32>
    decreases end,
{
    if end <= 0 { Seq::empty() }
    else {
        normalized_prefix(s, end - 1, replace)
            + if replace && drop_indent_space(s, end - 1) { Seq::empty() }
                else { seq![s[end - 1]] }
    }
}

pub open spec fn normalized(s: Seq<u32>, explicit: bool) -> Seq<u32> {
    let end = if explicit { first_boundary(s, 0) } else { s.len() as int };
    if explicit && end < s.len() { normalized_prefix(s, end, true) } else { s }
}

pub open spec fn token_equal(a: Token, b: Token) -> bool {
    a.kind == b.kind && a.text@ == b.text@
}

pub open spec fn lexed_equal(a: &Tokenization, b: &Tokenization) -> bool {
    a.whitespace_removed_key@ == b.whitespace_removed_key@
        && a.tokens@.len() == b.tokens@.len()
        && forall|i: int| 0 <= i < a.tokens@.len() ==> token_equal(a.tokens@[i], b.tokens@[i])
}

pub open spec fn explicit_assertion(a: Option<RewrittenAssertion>) -> bool {
    match a { Some(a) => a.has_explicit_msg, None => false }
}

pub open spec fn source_match(a: &Option<RewrittenAssertion>, d: &Declaration) -> bool {
    match a {
        Some(a) => !a.has_explicit_msg && match (&a.segment, &d.tokenization) {
            (Some(s), Some(d)) => lexed_equal(s, d),
            _ => false,
        },
        None => false,
    }
}

pub open spec fn decision(f: &RawRedFacts, reason: Seq<u32>) -> Option<RedMatch> {
    match f.declared {
        None => None,
        Some(d) => if reason == d.text@ { Some(RedMatch::Message) }
            else if source_match(&f.assertion, &d) { Some(RedMatch::SourceAssertion) }
            else { Some(RedMatch::Mismatch) },
    }
}
}
