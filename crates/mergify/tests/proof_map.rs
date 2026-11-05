//! Each enqueue contract and shared obligation has one required Verus selector.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const GROUPS: &str = include_str!("../../../tools/bazel/verus.bzl");

#[derive(Clone, Debug, PartialEq, Eq)]
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
            } else if bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] >= 128 {
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

#[derive(Clone, Debug)]
struct Item {
    name: String,
    contracts: Vec<String>,
    obligations: usize,
}

fn closing(ts: &[Token], start: usize) -> usize {
    let (open, close) = match ts[start] {
        Token::Punct('[') => ('[', ']'),
        Token::Punct('{') => ('{', '}'),
        Token::Punct('(') => ('(', ')'),
        _ => panic!("expected opening delimiter"),
    };
    let mut depth = 0;
    for (i, token) in ts.iter().enumerate().skip(start) {
        if *token == Token::Punct(open) {
            depth += 1;
        }
        if *token == Token::Punct(close) {
            depth -= 1;
            if depth == 0 {
                return i;
            }
        }
    }
    panic!("unclosed delimiter");
}

fn items(ts: &[Token], verified: bool) -> Vec<Item> {
    let mut result = Vec::new();
    let mut attributes = Vec::new();
    let mut spec = false;
    let mut i = 0;
    while i < ts.len() {
        if ts[i] == Token::Punct('#') && ts.get(i + 1) == Some(&Token::Punct('[')) {
            let end = closing(ts, i + 1);
            attributes.extend_from_slice(&ts[i + 2..end]);
            i = end + 1;
            continue;
        }
        if ts[i].name() == Some("verus") && ts.get(i + 1) == Some(&Token::Punct('!')) {
            assert_eq!(ts.get(i + 2), Some(&Token::Punct('{')));
            let end = closing(ts, i + 2);
            result.extend(items(&ts[i + 3..end], true));
            i = end + 1;
            continue;
        }
        spec |= ts[i].name() == Some("spec");
        if matches!(ts[i].name(), Some("fn" | "const" | "struct" | "enum")) {
            let kind = ts[i].name().unwrap();
            let name = ts.get(i + 1).and_then(Token::name).expect("unnamed item");
            let annotated = attributes
                .iter()
                .any(|t| matches!(t.name(), Some("verus_spec" | "verus_verify")));
            if verified || annotated {
                let contracts = attributes
                    .windows(4)
                    .filter(|part| {
                        part[0].name() == Some("crate")
                            && part[1] == Token::Punct(':')
                            && part[2] == Token::Punct(':')
                    })
                    .map(|part| part[3].name().unwrap().to_owned())
                    .collect();
                let clone = matches!(kind, "struct" | "enum")
                    && attributes.iter().any(|t| t.name() == Some("Clone"));
                if matches!(kind, "fn" | "const") || clone {
                    result.push(Item {
                        name: if clone {
                            format!("{name}::clone")
                        } else {
                            name.into()
                        },
                        contracts,
                        obligations: usize::from(!spec),
                    });
                }
            }
            attributes.clear();
            spec = false;
            let end = (i + 1..ts.len())
                .find(|&j| matches!(ts[j], Token::Punct('{') | Token::Punct(';')))
                .expect("item without body");
            i = if ts[end] == Token::Punct('{') {
                closing(ts, end) + 1
            } else {
                end + 1
            };
            continue;
        }
        if ts[i] == Token::Punct(';') {
            attributes.clear();
            spec = false;
        }
        i += 1;
    }
    result
}

#[derive(Clone, Debug)]
struct Group {
    name: String,
    root: String,
    selector: String,
    count: usize,
}

fn groups(source: &str) -> Vec<Group> {
    let table = source
        .split_once("        enqueue_groups = {")
        .expect("missing enqueue groups")
        .1;
    let ts = tokens(table.split_once("        }").expect("unclosed groups").0);
    let mut result = Vec::new();
    for entry in ts.chunks(10) {
        let [
            Token::Text(name),
            Token::Punct(':'),
            Token::Punct('('),
            Token::Text(root),
            Token::Punct(','),
            Token::Text(selector),
            Token::Punct(','),
            Token::Name(count),
            Token::Punct(')'),
            Token::Punct(','),
        ] = entry
        else {
            panic!("malformed enqueue group: {entry:?}");
        };
        result.push(Group {
            name: name.clone(),
            root: root.clone(),
            selector: selector.clone(),
            count: count.parse().expect("nonliteral obligation count"),
        });
    }
    assert!(!result.is_empty());
    result
}

fn imports(source: &str, root: &Path) -> BTreeMap<String, PathBuf> {
    let ts = tokens(source);
    let mut result = BTreeMap::new();
    let mut path = None;
    let mut i = 0;
    while i < ts.len() {
        if ts[i].name() == Some("path")
            && ts.get(i + 1) == Some(&Token::Punct('='))
            && let Some(Token::Text(p)) = ts.get(i + 2)
        {
            path = Some(p.clone());
        }
        if ts[i].name() == Some("mod") && ts.get(i + 2) == Some(&Token::Punct(';')) {
            let module = ts.get(i + 1).and_then(Token::name).expect("unnamed module");
            let file = root
                .parent()
                .unwrap()
                .join(path.take().unwrap_or_else(|| format!("{module}.rs")));
            let file = file.canonicalize().expect("missing proof input");
            assert!(
                result.insert(module.into(), file).is_none(),
                "duplicate module"
            );
        }
        i += 1;
    }
    result
}

fn matches(selector: &str, name: &str) -> bool {
    let mut remaining = name;
    let parts: Vec<_> = selector.split('*').collect();
    for (i, part) in parts.iter().enumerate() {
        let Some(at) = remaining.find(part) else {
            return false;
        };
        if i == 0 && at != 0 {
            return false;
        }
        remaining = &remaining[at + part.len()..];
    }
    selector.ends_with('*') || remaining.is_empty()
}

fn inventory(root: &Path, groups: &[Group]) -> Result<(usize, usize), String> {
    let proofs = root.join("proofs");
    let production = root.join("src/decide/enqueue.rs").canonicalize().unwrap();
    let mut sources = BTreeMap::new();
    sources.insert(production, Vec::new());
    for file in std::fs::read_dir(proofs).unwrap() {
        let file = file.unwrap().path();
        if file
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("enqueue_")
            && file.extension().is_some_and(|e| e == "rs")
        {
            sources.insert(file.canonicalize().unwrap(), Vec::new());
        }
    }
    for (file, found) in &mut sources {
        *found = items(&tokens(&std::fs::read_to_string(file).unwrap()), false);
    }
    let mut names = BTreeSet::new();
    let mut covered_files = BTreeSet::new();
    let mut owners: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut errors = Vec::new();
    for group in groups {
        if !names.insert(&group.name) {
            errors.push(format!("duplicate group {}", group.name));
        }
        let file = root.join(format!("proofs/enqueue_{}.rs", group.root));
        let Ok(file) = file.canonicalize() else {
            errors.push(format!("stale root {}", group.root));
            continue;
        };
        covered_files.insert(file.clone());
        let mut modules = imports(&std::fs::read_to_string(&file).unwrap(), &file);
        modules.insert("root".into(), file.clone());
        let (module, function) = group
            .selector
            .split_once('#')
            .map_or((group.selector.as_str(), None), |(m, f)| (m, Some(f)));
        let mut count = 0;
        for (alias, file) in &modules {
            let Some(items) = sources.get(file) else {
                continue;
            };
            covered_files.insert(file.clone());
            for item in items.iter().filter(|item| item.obligations > 0) {
                // Body identity includes resolved contract modules. Five different
                // decide postconditions are distinct; the shared helper contracts
                // and generated Clone bodies retain one identity across roots.
                let mut key = format!("{}::{}", file.display(), item.name);
                for contract in &item.contracts {
                    key.push_str(&format!(
                        "@{}",
                        modules
                            .get(contract)
                            .expect("unresolved contract module")
                            .display()
                    ));
                }
                let selected = alias == module && function.is_none_or(|f| matches(f, &item.name));
                let owners = owners.entry(key).or_default();
                if selected {
                    owners.push(group.name.clone());
                    count += item.obligations;
                }
            }
        }
        if count == 0 || count != group.count {
            errors.push(format!(
                "{}: selector {} matched {count} obligations, expected {}",
                group.name, group.selector, group.count
            ));
        }
    }
    for file in sources.keys() {
        if !covered_files.contains(file) {
            errors.push(format!("unmapped proof input {}", file.display()));
        }
    }
    for (item, owners) in &owners {
        if owners.len() != 1 {
            errors.push(format!(
                "{item}: expected exactly one selector, found {owners:?}"
            ));
        }
    }
    if errors.is_empty() {
        Ok((
            owners.len(),
            sources
                .values()
                .flatten()
                .filter(|i| i.obligations == 0)
                .count(),
        ))
    } else {
        Err(errors.join("\n"))
    }
}

#[test]
fn every_enqueue_body_has_exactly_one_selector() {
    let groups = groups(GROUPS);
    let (bodies, specs) = inventory(Path::new(env!("CARGO_MANIFEST_DIR")), &groups)
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(bodies, groups.iter().map(|g| g.count).sum::<usize>());
    println!(
        "Enqueue selector inventory: {bodies} distinct obligations, {} groups, {specs} spec definitions typechecked through their owning roots",
        groups.len()
    );
}

#[test]
fn missing_duplicate_stale_and_wrong_count_mappings_are_rejected() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let groups = groups(GROUPS);
    let mut missing = groups.clone();
    missing.remove(0);
    assert!(
        inventory(root, &missing)
            .unwrap_err()
            .contains("expected exactly one selector")
    );
    let mut duplicate = groups.clone();
    duplicate.push(groups[0].clone());
    assert!(
        inventory(root, &duplicate)
            .unwrap_err()
            .contains("duplicate group")
    );
    let mut duplicate_owner = groups.clone();
    let mut helper = groups
        .iter()
        .find(|g| g.name == "command_completion")
        .unwrap()
        .clone();
    helper.name = "duplicate_helper".into();
    helper.root = "recovery_grace".into();
    duplicate_owner.push(helper);
    assert!(
        inventory(root, &duplicate_owner)
            .unwrap_err()
            .contains("expected exactly one selector")
    );
    let mut stale = groups.clone();
    stale[0].selector = "enqueue#deleted_function".into();
    assert!(
        inventory(root, &stale)
            .unwrap_err()
            .contains("matched 0 obligations")
    );
    let mut wrong = groups.clone();
    wrong[0].count += 1;
    assert!(inventory(root, &wrong).unwrap_err().contains("expected 2"));
}

#[test]
fn source_inventory_detects_new_functions_and_generated_clones() {
    let source = r#"
        // #[verus_spec] fn commented_out() {}
        #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures result))]
        fn required_check() -> bool { true }
        fn ordinary() {}
        #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
        #[derive(Clone, Copy)] enum LabelEvent { Present }
        verus! { pub open spec fn predicate() -> bool { true } proof fn reach() {} }
    "#;
    let items = items(&tokens(source), false);
    assert_eq!(
        items
            .iter()
            .map(|i| (i.name.as_str(), i.obligations))
            .collect::<Vec<_>>(),
        vec![
            ("required_check", 1),
            ("LabelEvent::clone", 1),
            ("predicate", 0),
            ("reach", 1)
        ]
    );
}
