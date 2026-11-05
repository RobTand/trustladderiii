use serde::Deserialize;
use tl3_harness_core::red_match::{
    Declaration, PythonText, RawRedFacts, RedMatch, RewrittenAssertion, Token, Tokenization, decide,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    python_codepoints: Vec<u32>,
}

impl Text {
    fn points(self) -> PythonText {
        assert!(self.python_codepoints.iter().all(|&cp| cp <= 0x10ffff));
        self.python_codepoints
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Lexed {
    whitespace_removed_key: Text,
    tokens: Vec<Lexeme>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Lexeme {
    kind: u32,
    text: Text,
}

impl Lexed {
    fn facts(self) -> Tokenization {
        Tokenization {
            whitespace_removed_key: self.whitespace_removed_key.points(),
            tokens: self
                .tokens
                .into_iter()
                .map(|t| Token {
                    kind: t.kind,
                    text: t.text.points(),
                })
                .collect(),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Assertion {
    has_explicit_msg: bool,
    segment: Option<Lexed>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Declared {
    text: Text,
    tokenization: Option<Lexed>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Facts {
    reason_raw: Text,
    assertion: Option<Assertion>,
    declared: Option<Declared>,
}

impl Facts {
    fn facts(self) -> RawRedFacts {
        RawRedFacts {
            reason_raw: self.reason_raw.points(),
            assertion: self.assertion.map(|a| RewrittenAssertion {
                has_explicit_msg: a.has_explicit_msg,
                segment: a.segment.map(Lexed::facts),
            }),
            declared: self.declared.map(|d| Declaration {
                text: d.text.points(),
                tokenization: d.tokenization.map(Lexed::facts),
            }),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Expected {
    reason: Text,
    r#match: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    origin: String,
    facts: Facts,
    expected: Expected,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Corpus {
    kind: String,
    cases: Vec<Case>,
}

fn corpus() -> Corpus {
    serde_json::from_str(include_str!("fixtures/red_match.json")).unwrap()
}

fn parity(origin: &str, count: usize) {
    let corpus = corpus();
    assert_eq!(corpus.kind, "red-match-facts");
    let mut executed = 0;
    for case in corpus
        .cases
        .into_iter()
        .filter(|case| case.origin == origin)
    {
        let facts = case.facts.facts();
        let actual = decide(&facts);
        let expected_match = case.expected.r#match.as_deref().map(|kind| match kind {
            "message" => RedMatch::Message,
            "source-assertion" => RedMatch::SourceAssertion,
            "mismatch" => RedMatch::Mismatch,
            other => panic!("unexpected match: {other}"),
        });
        assert_eq!(
            actual.reason,
            case.expected.reason.points(),
            "{} reason",
            case.id
        );
        assert_eq!(actual.matched, expected_match, "{} match", case.id);
        executed += 1;
    }
    assert_eq!(executed, count);
    println!("{origin}: {executed} real-producer RED decisions and reasons agree");
}

#[test]
fn all_seventeen_original_vectors_use_independent_raw_facts() {
    parity("core", 17);
}

#[test]
fn boundaries_token_failures_and_python_codepoints_are_lossless() {
    parity("edge", 17);
}
