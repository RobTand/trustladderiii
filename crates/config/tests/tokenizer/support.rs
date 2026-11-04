use base64::{Engine, engine::general_purpose::STANDARD};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{fs, path::Path, sync::OnceLock};
use tiktoken_rs::{CoreBPE, O200K_BASE_PAT_STR};

pub const PIN: &str = "446a9538cb6c348e3516120d7c08b09f57c36495e2acfffe59a5bf8b0cfb1a2d";
const CACHE_KEY: &str = "fb374d419588a4632f3f557e76b4b70aebbca790";

#[derive(Deserialize)]
pub struct Oracle {
    pub python_tiktoken: String,
    pub bpe_sha256: String,
    pub pattern: String,
    pub random_seed: u64,
    pub cases: Vec<Case>,
}

#[derive(Deserialize)]
pub struct Case {
    pub group: String,
    pub name: String,
    pub text: String,
    #[serde(default = "one")]
    pub repeat: usize,
    pub truncate_bytes: Option<usize>,
    #[serde(default)]
    pub prefix: String,
    #[serde(default)]
    pub suffix: String,
    pub bytes: usize,
    pub sha256: String,
    pub tokens: usize,
}

fn one() -> usize {
    1
}

impl Case {
    pub fn expanded(&self) -> String {
        let mut text = self.text.repeat(self.repeat);
        if let Some(bytes) = self.truncate_bytes {
            text.truncate(bytes);
        }
        let text = format!("{}{text}{}", self.prefix, self.suffix);
        assert_eq!(text.len(), self.bytes, "{} input bytes", self.name);
        assert_eq!(
            digest(text.as_bytes()),
            self.sha256,
            "{} input hash",
            self.name
        );
        text
    }
}

pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn load(cache: &Path) -> Result<CoreBPE, String> {
    let bytes = fs::read(cache.join(CACHE_KEY)).map_err(|_| "missing pinned BPE")?;
    if digest(&bytes) != PIN {
        return Err("pinned BPE sha256 mismatch".into());
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| "invalid BPE UTF-8")?;
    let ranks = text
        .lines()
        .map(|line| {
            let (token, rank) = line.split_once(' ').expect("pinned BPE line");
            (
                STANDARD.decode(token).expect("pinned BPE token"),
                rank.parse().expect("pinned BPE rank"),
            )
        })
        .collect();
    let special = [
        ("<|endoftext|>".into(), 199999),
        ("<|endofprompt|>".into(), 200018),
    ]
    .into_iter()
    .collect();
    CoreBPE::new(ranks, special, O200K_BASE_PAT_STR).map_err(|error| error.to_string())
}

pub fn tokenizer() -> &'static CoreBPE {
    static ENCODING: OnceLock<CoreBPE> = OnceLock::new();
    ENCODING.get_or_init(|| {
        let cache = std::env::var_os("TIKTOKEN_CACHE_DIR")
            .expect("preload TIKTOKEN_CACHE_DIR; no runtime download");
        load(Path::new(&cache)).expect("load pinned offline tokenizer")
    })
}

pub fn count(encoding: &CoreBPE, text: &str) -> usize {
    encoding.encode_ordinary(text).len()
}

pub fn oracle() -> Oracle {
    let oracle: Oracle = serde_json::from_str(include_str!("oracle.json")).unwrap();
    assert_eq!(oracle.python_tiktoken, "0.14.0");
    assert_eq!(oracle.bpe_sha256, PIN);
    assert_eq!(oracle.pattern, O200K_BASE_PAT_STR);
    assert_eq!(oracle.random_seed, 748);
    oracle
}

pub fn compare(group: &str) {
    let oracle = oracle();
    let cases: Vec<_> = oracle
        .cases
        .iter()
        .filter(|case| case.group == group)
        .collect();
    assert!(!cases.is_empty());
    let mut mismatches = Vec::new();
    let mut upper_bound = true;
    let mut max_delta = 0;
    for case in &cases {
        let actual = count(tokenizer(), &case.expanded());
        if actual != case.tokens {
            mismatches.push(format!(
                "{}: Rust={actual} Python={} delta={}",
                case.name,
                case.tokens,
                actual as i64 - case.tokens as i64
            ));
        }
        upper_bound &= actual >= case.tokens;
        max_delta = max_delta.max(actual.abs_diff(case.tokens));
    }
    println!(
        "{group}: {} cases, {} mismatches, max absolute delta={max_delta}, Rust >= Python={upper_bound}",
        cases.len(),
        mismatches.len()
    );
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}
