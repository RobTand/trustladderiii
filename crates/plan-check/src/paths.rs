//! CPython 3.14 PurePosixPath.full_match, including path normalization.
mod decision;
use decision::{Atom, Step, evaluate};

#[derive(Debug)]
enum Token {
    Literal(char),
    Any,
    Class {
        negated: bool,
        ranges: Vec<(char, char)>,
    },
    Star,
    NonemptyStar,
    Recursive,
    Directories,
}

fn normalize(path: &str) -> String {
    let root = if path.starts_with("//") && !path.starts_with("///") {
        "//"
    } else if path.starts_with('/') {
        "/"
    } else {
        ""
    };
    let tail = path
        .split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .collect::<Vec<_>>()
        .join("/");
    format!("{root}{tail}")
}

// fnmatch._translate removes reversed ranges after splitting at range hyphens.
fn class_ranges(stuff: &[char]) -> (bool, Vec<(char, char)>) {
    let mut chunks: Vec<Vec<char>> = Vec::new();
    let mut begin = 0;
    let mut search = if stuff.first() == Some(&'!') { 2 } else { 1 };
    while search < stuff.len() {
        let Some(relative) = stuff[search..].iter().position(|&c| c == '-') else {
            break;
        };
        let at = search + relative;
        chunks.push(stuff[begin..at].to_vec());
        begin = at + 1;
        search = at + 3;
    }
    if begin < stuff.len() {
        chunks.push(stuff[begin..].to_vec());
    } else if let Some(last) = chunks.last_mut() {
        last.push('-');
    }
    for index in (1..chunks.len()).rev() {
        if chunks[index - 1].last() > chunks[index].first() {
            let right = chunks.remove(index);
            chunks[index - 1].pop();
            chunks[index - 1].extend_from_slice(&right[1..]);
        }
    }
    let negated = chunks.first().and_then(|chunk| chunk.first()) == Some(&'!');
    if let Some(first) = chunks.first_mut()
        && first.first() == Some(&'!')
    {
        first.remove(0);
    }
    let mut ranges = Vec::new();
    // Each join is one genuine range; escaped hyphens inside chunks stay literal.
    let mut previous: Option<char> = None;
    for (index, chunk) in chunks.iter().enumerate() {
        if index > 0
            && let Some(left) = previous.take()
            && let Some(&right) = chunk.first()
        {
            ranges.push((left, right));
        }
        for (position, &c) in chunk.iter().enumerate() {
            if index > 0 && position == 0 {
                continue;
            }
            if let Some(last) = previous.replace(c) {
                ranges.push((last, last));
            }
        }
    }
    if let Some(last) = previous {
        ranges.push((last, last));
    }
    (negated, ranges)
}

fn component(part: &str, tokens: &mut Vec<Token>) {
    let chars: Vec<_> = part.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        i += 1;
        match c {
            '*' => {
                tokens.push(Token::Star);
                while chars.get(i) == Some(&'*') {
                    i += 1;
                }
            }
            '?' => tokens.push(Token::Any),
            '[' => {
                let mut end = i;
                if chars.get(end) == Some(&'!') {
                    end += 1;
                }
                if chars.get(end) == Some(&']') {
                    end += 1;
                }
                while end < chars.len() && chars[end] != ']' {
                    end += 1;
                }
                if end == chars.len() {
                    tokens.push(Token::Literal('['));
                } else {
                    let stuff = &chars[i..end];
                    let (negated, ranges) = class_ranges(stuff);
                    tokens.push(Token::Class { negated, ranges });
                    i = end + 1;
                }
            }
            _ => tokens.push(Token::Literal(c)),
        }
    }
}

fn parse(pattern: &str) -> Vec<Token> {
    let parts: Vec<_> = pattern.split('/').collect();
    let mut tokens = Vec::new();
    for (index, &part) in parts.iter().enumerate() {
        let last = index + 1 == parts.len();
        match part {
            "*" => {
                tokens.push(Token::NonemptyStar);
                if !last {
                    tokens.push(Token::Literal('/'));
                }
            }
            "**" => {
                if last {
                    tokens.push(Token::Recursive);
                } else if parts[index + 1] != "**" {
                    tokens.push(Token::Directories);
                }
            }
            _ => {
                component(part, &mut tokens);
                if !last {
                    tokens.push(Token::Literal('/'));
                }
            }
        }
    }
    tokens
}

pub fn full_match(path: &str, pattern: &str) -> bool {
    let path: Vec<_> = normalize(path).chars().collect();
    let tokens = parse(&normalize(pattern));
    let separators = path.iter().map(|&c| c == '/').collect::<Vec<_>>();
    let atoms = tokens
        .into_iter()
        .map(|token| {
            let kind = match token {
                Token::Star => Step::Star,
                Token::NonemptyStar => Step::NonemptyStar,
                Token::Recursive => Step::Recursive,
                Token::Directories => Step::Directories,
                _ => Step::Character,
            };
            let accepts = path
                .iter()
                .map(|&c| match &token {
                    Token::Literal(literal) => c == *literal,
                    Token::Any => c != '/',
                    Token::Class { negated, ranges } => {
                        *negated != ranges.iter().any(|&(start, end)| start <= c && c <= end)
                    }
                    _ => false,
                })
                .collect();
            Atom { kind, accepts }
        })
        .collect::<Vec<_>>();
    evaluate(&atoms, &separators)
}
