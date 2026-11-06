use vstd::prelude::*;
#[path = "../src/red_match.rs"]
mod red_match;
use red_match::*;
use red_match::spec::*;

verus! {
/// #836 / Q27: arbitrary finite text follows the first-boundary/replace rule.
fn reason_normalization(raw: Vec<u32>, explicit: bool) {
    let reason = normalize_reason(&raw, explicit);
    assert(reason@ == normalized(raw@, explicit));
    assert((!explicit || first_boundary(raw@, 0) == raw@.len()) ==> reason@ == raw@);
}

/// #709/#713: absence of a declaration preserves reason and returns None.
fn undeclared_has_no_match(f: RawRedFacts)
    requires f.declared is None,
{
    let result = decide(&f);
    assert(result.matched is None);
    assert(result.reason@ == normalized(f.reason_raw@, explicit_assertion(f.assertion)));
}

/// #709/#713: complete, whitespace-sensitive MESSAGE equality precedes source.
fn message_precedence(f: RawRedFacts) {
    let result = decide(&f);
    match &f.declared {
        Some(d) => assert((result.matched == Some(RedMatch::Message)) == (result.reason@ == d.text@)),
        None => assert(result.matched is None),
    }
}

/// #836 / Q27: fallback requires recovery, a bare assert, both lexings, raw key,
/// and every ordered token type and spelling. No bounded text abstraction.
fn source_assertion_exact(f: RawRedFacts) {
    let result = decide(&f);
    match &f.declared {
        Some(d) => assert((result.matched == Some(RedMatch::SourceAssertion)) ==
            (result.reason@ != d.text@ && source_match(&f.assertion, d))),
        None => assert(result.matched is None),
    }
}

/// #836: all remaining declared cases mismatch, deterministically.
fn mismatch_complement(f: RawRedFacts) {
    let a = decide(&f);
    let b = decide(&f);
    assert(a.reason@ == b.reason@ && a.matched == b.matched);
    match &f.declared {
        Some(d) => assert((a.matched == Some(RedMatch::Mismatch)) ==
            (a.reason@ != d.text@ && !source_match(&f.assertion, d))),
        None => assert(a.matched is None),
    }
}

/// #709: agreeing through display character 1000 does not admit a later change.
/// Bound: arbitrary finite lengths above 1000, no upper length bound.
fn display_independence(raw: Vec<u32>, declared: Vec<u32>)
    requires raw@.len() > 1000, declared@.len() > 1000,
        raw@.take(1000) == declared@.take(1000), raw@ != declared@,
{
    let facts = RawRedFacts { reason_raw: raw, assertion: None,
        declared: Some(Declaration { text: declared, tokenization: None }) };
    let result = decide(&facts);
    assert(result.matched == Some(RedMatch::Mismatch));
}

/// #836: inhabited normalization branches, including first of two boundaries.
fn normalization_witnesses() {
    reveal_with_fuel(first_boundary, 32);
    reveal_with_fuel(normalized_prefix, 32);
    let raw = vec![97u32, 10, 32, 32, 98, 10, 97, 115, 115, 101, 114, 116, 32,
        10, 97, 115, 115, 101, 114, 116, 32];
    let manual = normalize_reason(&raw, false);
    assert(manual@ == raw@);
    let explicit = normalize_reason(&raw, true);
    assert(explicit@ == seq![97u32, 10, 98]);
    let absent = vec![97u32, 10, 32, 32, 98];
    let unpartitioned = normalize_reason(&absent, true);
    assert(unpartitioned@ == absent@);
    let spaces = vec![10u32, 32, 97, 10, 32, 32, 32, 98, 10, 97, 115, 115, 101, 114, 116, 32];
    let replaced = normalize_reason(&spaces, true);
    assert(replaced@ == seq![10u32, 32, 97, 10, 32, 98]);
    let empty = Vec::<u32>::new();
    let normalized_empty = normalize_reason(&empty, true);
    assert(normalized_empty@ == Seq::<u32>::empty());
}

/// #836: every output is reachable, including missing lexing and explicit guard.
fn decision_witnesses() {
    let undeclared = RawRedFacts { reason_raw: vec![0xd800u32], assertion: None, declared: None };
    let none = decide(&undeclared);
    assert(none.matched is None && none.reason@ == seq![0xd800u32]);
    let exact = RawRedFacts { reason_raw: vec![32u32], assertion: None,
        declared: Some(Declaration { text: vec![32u32], tokenization: None }) };
    let message = decide(&exact);
    assert(message.matched == Some(RedMatch::Message));
    let source = RawRedFacts { reason_raw: vec![1u32],
        assertion: Some(RewrittenAssertion { has_explicit_msg: false,
            segment: Some(Tokenization { whitespace_removed_key: vec![2u32],
                tokens: vec![Token { kind: 1, text: vec![2u32] }] }) }),
        declared: Some(Declaration { text: vec![2u32],
            tokenization: Some(Tokenization { whitespace_removed_key: vec![2u32],
                tokens: vec![Token { kind: 1, text: vec![2u32] }] }) }) };
    let fallback = decide(&source);
    assert(fallback.matched == Some(RedMatch::SourceAssertion));
    let unequal = RawRedFacts { reason_raw: vec![32u32], assertion: None,
        declared: Some(Declaration { text: vec![32u32, 32], tokenization: None }) };
    let mismatch = decide(&unequal);
    assert(mismatch.matched == Some(RedMatch::Mismatch));
}
}
fn main() {}
