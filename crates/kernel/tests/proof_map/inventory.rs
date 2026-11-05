//! #806: fail closed when a kernel function escapes the required Verus selectors.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Debug, PartialEq, Eq)]
enum Token {
    Name(String),
    Text(String),
    Punct(char),
}

impl Token {
    fn name(&self) -> Option<&str> {
        match self {
            Self::Name(name) => Some(name),
            _ => None,
        }
    }
}

// Rust and Verus share lexical delimiters. Keep strings distinct from identifiers,
// skip nested comments, and leave macro token trees visible except macro definitions.
fn tokens(source: &str) -> Vec<Token> {
    let bytes = source.as_bytes();
    let mut result = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"//") {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
        } else if bytes[i..].starts_with(b"/*") {
            i += 2;
            let mut depth = 1;
            while depth > 0 {
                assert!(i < bytes.len(), "unterminated block comment");
                if bytes[i..].starts_with(b"/*") {
                    depth += 1;
                    i += 2;
                } else if bytes[i..].starts_with(b"*/") {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
        } else if bytes[i].is_ascii_whitespace() {
            i += 1;
        } else {
            let raw_start = if bytes[i..].starts_with(b"br") {
                i + 2
            } else if bytes[i] == b'r' {
                i + 1
            } else {
                i
            };
            let mut quote = raw_start;
            while quote < bytes.len() && bytes[quote] == b'#' {
                quote += 1;
            }
            if raw_start > i && quote < bytes.len() && bytes[quote] == b'"' {
                let end = format!("\"{}", "#".repeat(quote - raw_start));
                let content = quote + 1;
                let length = source[content..]
                    .find(&end)
                    .expect("unterminated raw string");
                result.push(Token::Text(source[content..content + length].into()));
                i = content + length + end.len();
            } else if bytes[i] == b'"' {
                let start = i + 1;
                i = start;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
                assert!(i < bytes.len(), "unterminated string");
                result.push(Token::Text(source[start..i].into()));
                i += 1;
            } else if bytes[i] == b'\''
                && ((i + 2 < bytes.len() && bytes[i + 2] == b'\'')
                    || (i + 3 < bytes.len() && bytes[i + 1] == b'\\' && bytes[i + 3] == b'\''))
            {
                i += if bytes[i + 1] == b'\\' { 4 } else { 3 };
            } else if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' || bytes[i] >= 128 {
                let start = i;
                i += 1;
                while i < bytes.len()
                    && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] >= 128)
                {
                    i += 1;
                }
                result.push(Token::Name(source[start..i].into()));
            } else {
                result.push(Token::Punct(bytes[i] as char));
                i += 1;
            }
        }
    }
    result
}

#[derive(Debug)]
struct Function {
    module: String,
    path: String,
    name: String,
    verifiable: bool,
}

fn functions(source: &str, module: &str) -> Vec<Function> {
    fn walk(ts: &[Token], module: &str, owner: &str, verifiable: bool, result: &mut Vec<Function>) {
        let mut i = 0;
        let mut annotated = false;
        while i < ts.len() {
            if ts[i] == Token::Punct('#') && ts.get(i + 1) == Some(&Token::Punct('[')) {
                let mut end = i + 2;
                let mut depth = 1;
                while depth > 0 {
                    let token = ts.get(end).expect("unterminated attribute");
                    if *token == Token::Punct('[') {
                        depth += 1;
                    }
                    if *token == Token::Punct(']') {
                        depth -= 1;
                    }
                    annotated |= token.name() == Some("verus_spec");
                    end += 1;
                }
                i = end;
                continue;
            }
            if matches!(
                ts[i].name(),
                Some("struct" | "enum" | "static" | "type" | "trait" | "mod")
            ) {
                annotated = false;
            }
            if ts[i].name() == Some("macro_rules") {
                let macro_name = ts.get(i + 2).and_then(Token::name).expect("unnamed macro");
                assert!(
                    matches!(
                        macro_name,
                        "kernel_snapshot"
                            | "kernel_loop"
                            | "kernel_while"
                            | "kernel_proof"
                            | "kernel_fn"
                    ),
                    "new kernel macro needs explicit inventory support: {macro_name}"
                );
                let start = (i..ts.len()).find(|&j| ts[j] == Token::Punct('{')).unwrap();
                let end = closing(ts, start);
                assert!(
                    !ts[start..end]
                        .windows(2)
                        .any(|pair| pair[0].name() == Some("fn") && pair[1].name().is_some()),
                    "literal function in a macro definition needs inventory support"
                );
                i = end + 1;
                continue;
            }
            if matches!(ts[i].name(), Some("kernel_fn" | "verus"))
                && ts.get(i + 1) == Some(&Token::Punct('!'))
                && ts.get(i + 2) == Some(&Token::Punct('{'))
            {
                let close = closing(ts, i + 2);
                walk(&ts[i + 3..close], module, owner, true, result);
                i = close + 1;
                continue;
            }
            if ts[i].name() == Some("fn")
                && let Some(name) = ts.get(i + 1).and_then(Token::name)
            {
                result.push(Function {
                    module: module.into(),
                    path: format!("{owner}::{name}"),
                    name: name.into(),
                    verifiable: verifiable || annotated,
                });
            }
            if ts[i].name() == Some("fn") {
                annotated = false;
            }
            if ts[i].name() == Some("impl") || ts[i].name() == Some("mod") {
                let end = (i + 1..ts.len())
                    .find(|&j| matches!(ts[j], Token::Punct('{') | Token::Punct(';')))
                    .expect("module/impl without delimiter");
                if ts[end] == Token::Punct('{') {
                    let close = closing(ts, end);
                    let names: Vec<_> = ts[i + 1..end].iter().filter_map(Token::name).collect();
                    let name = names.last().expect("unnamed module/impl");
                    if ts[i].name() == Some("mod") {
                        let child = format!("{module}::{name}");
                        walk(&ts[end + 1..close], &child, &child, verifiable, result);
                    } else {
                        walk(
                            &ts[end + 1..close],
                            module,
                            &format!("{module}::{name}"),
                            verifiable,
                            result,
                        );
                    }
                    i = close + 1;
                    continue;
                }
            }
            i += 1;
        }
    }
    let mut result = Vec::new();
    walk(&tokens(source), module, module, false, &mut result);
    result
}

fn closing(ts: &[Token], start: usize) -> usize {
    let mut depth = 0;
    for (i, token) in ts.iter().enumerate().skip(start) {
        match token {
            Token::Punct('{') => depth += 1,
            Token::Punct('}') => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
    }
    panic!("unbalanced braces")
}

fn table<'a>(source: &'a str, marker: &str) -> &'a str {
    let rest = source
        .split_once(marker)
        .expect("missing inventory table")
        .1;
    rest.split_once('\n')
        .unwrap()
        .1
        .split_once("        }")
        .unwrap()
        .0
}

fn selectors(source: &str) -> Vec<(String, String)> {
    let ts = tokens(table(source, "        groups = {"));
    let texts: Vec<_> = ts
        .iter()
        .filter_map(|t| match t {
            Token::Text(s) => Some(s.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(texts.len() % 2, 0, "malformed selector table");
    texts
        .as_chunks::<2>()
        .0
        .iter()
        .map(|s| (s[0].clone(), s[1].clone()))
        .collect()
}

fn exemptions(source: &str) -> BTreeMap<String, String> {
    let ts = tokens(table(source, "        unverified_functions = {"));
    let texts: Vec<_> = ts
        .iter()
        .filter_map(|t| match t {
            Token::Text(s) => Some(s.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(texts.len() % 2, 0, "malformed unverified table");
    let mut result = BTreeMap::new();
    for pair in texts.as_chunks::<2>().0 {
        assert!(
            !pair[1].trim().is_empty(),
            "unverified function needs a reason"
        );
        assert!(
            result.insert(pair[0].clone(), pair[1].clone()).is_none(),
            "duplicate exemption"
        );
    }
    result
}

fn check(
    functions: &[Function],
    groups: &[(String, String)],
    unverified: &BTreeMap<String, String>,
) -> Result<(), String> {
    let mut used = BTreeSet::new();
    let mut matched = BTreeSet::new();
    let mut errors = Vec::new();
    for function in functions {
        let selected: Vec<_> = groups
            .iter()
            .filter(|(_, selector)| {
                let (module, name) = selector
                    .split_once('#')
                    .map_or((selector.as_str(), None), |(m, f)| (m, Some(f)));
                module == function.module && name.is_none_or(|name| name == function.name)
            })
            .map(|(group, _)| group)
            .collect();
        if unverified.contains_key(&function.path) {
            used.insert(function.path.clone());
        } else if !function.verifiable {
            errors.push(format!(
                "{}: ordinary Rust function needs an explicit unverified reason",
                function.path
            ));
        } else if selected.len() != 1 {
            errors.push(format!(
                "{}: expected exactly one Verus selector, found {:?}",
                function.path, selected
            ));
        }
        matched.extend(selected.into_iter().cloned());
    }
    for name in unverified.keys() {
        if !used.contains(name) {
            errors.push(format!("stale unverified function: {name}"));
        }
    }
    for (name, _) in groups {
        if !matched.contains(name) {
            errors.push(format!("empty Verus selector: {name}"));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}

fn collect(directory: &Path, prefix: &str, result: &mut Vec<Function>) {
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            let child = format!("{prefix}::{}", path.file_name().unwrap().to_str().unwrap());
            collect(&path, &child, result);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            let name = path.file_stem().unwrap().to_str().unwrap();
            if matches!(name, "proofs") || (name == "progress" && directory.ends_with("verus")) {
                continue;
            }
            let module = if matches!(name, "lib" | "mod" | "headroom" | "progress_specs") {
                prefix.into()
            } else {
                format!("{prefix}::{name}")
            };
            result.extend(functions(&std::fs::read_to_string(path).unwrap(), &module));
        }
    }
}

#[test]
fn every_kernel_function_has_exactly_one_selector_or_a_reason() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut fs = Vec::new();
    collect(&root.join("src"), "kernel", &mut fs);
    collect(&root.join("verus"), "kernel", &mut fs);
    fs.extend(functions(include_str!("../../verus/progress.rs"), "root"));
    fs.extend(functions(
        include_str!("../../../controller/src/kernel_boundary.rs"),
        "controller_boundary",
    ));
    let groups = selectors(super::VERUS_GROUPS);
    let unverified = exemptions(super::VERUS_GROUPS);
    if let Err(error) = check(&fs, &groups, &unverified) {
        panic!("{error}");
    }
    println!(
        "Selector inventory: {} functions, {} groups, {} explicit unverified reasons",
        fs.len(),
        groups.len(),
        unverified.len()
    );
}

#[test]
fn unlisted_duplicate_stale_and_reasonless_entries_are_rejected() {
    let fs = functions(
        "kernel_fn! { fn first() {} fn extracted() {} }",
        "kernel::facts",
    );
    let one = vec![("first".into(), "kernel::facts#first".into())];
    assert!(
        check(&fs, &one, &BTreeMap::new())
            .unwrap_err()
            .contains("kernel::facts::extracted")
    );
    let all = vec![("all".into(), "kernel::facts".into())];
    let ordinary = functions("fn unchecked() {}", "kernel::facts");
    assert!(
        check(&ordinary, &all, &BTreeMap::new())
            .unwrap_err()
            .contains("ordinary Rust function")
    );
    let acknowledged = BTreeMap::from([(
        "kernel::facts::unchecked".into(),
        "Boundary adapter outside the proof".into(),
    )]);
    assert!(check(&ordinary, &all, &acknowledged).is_ok());
    assert!(check(&fs, &all, &BTreeMap::new()).is_ok());
    let mut duplicate = all.clone();
    duplicate.extend(one);
    assert!(check(&fs, &duplicate, &BTreeMap::new()).is_err());
    let no_reason = "        unverified_functions = {\n            \"kernel::facts::extracted\": \"  \",\n        }";
    assert!(std::panic::catch_unwind(|| exemptions(no_reason)).is_err());
    let stale = BTreeMap::from([("kernel::facts::missing".into(), "not verified".into())]);
    assert!(check(&fs, &all, &stale).is_err());
}

#[test]
fn lexical_noise_does_not_hide_a_function_or_invent_one() {
    let source = "/* fn fake() {} /* nested */ */ impl Node { kernel_fn! { fn real() { let s = r###\"fn fake() {}\"###; } } } // fn fake() {}\n mod nested { fn helper() {} }";
    assert!(
        std::panic::catch_unwind(|| functions(
            "macro_rules! hidden { () => { fn escaped() {} } }",
            "kernel::facts"
        ))
        .is_err()
    );
    let fs = functions(source, "kernel::facts");
    assert_eq!(
        fs.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
        ["kernel::facts::Node::real", "kernel::facts::nested::helper"]
    );
}
