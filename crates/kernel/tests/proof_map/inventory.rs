//! #806: fail closed when a kernel function escapes the required Verus selectors.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

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
fn located_tokens(source: &str) -> (Vec<Token>, Vec<usize>) {
    let bytes = source.as_bytes();
    let mut result = Vec::new();
    let mut offsets = Vec::new();
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
            let offset = i;
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
                let start = if bytes[i..].starts_with(b"r#") {
                    i += 2;
                    i
                } else {
                    i
                };
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
            if offsets.len() < result.len() {
                offsets.push(offset);
            }
        }
    }
    (result, offsets)
}

fn tokens(source: &str) -> Vec<Token> {
    located_tokens(source).0
}

#[derive(Debug)]
struct Function {
    module: String,
    path: String,
    verifiable: bool,
    definition_only: bool,
}

// Only plain signatures qualify. Contract expressions can contain braces before
// a later decreases clause, so ambiguous headers must retain a selector.
fn plain_definition_header(ts: &[Token]) -> bool {
    if ts.first() != Some(&Token::Punct('(')) {
        return false;
    }
    let mut delimiters = Vec::new();
    for (i, token) in ts.iter().enumerate() {
        match token {
            Token::Name(name)
                if matches!(
                    name.as_str(),
                    "requires"
                        | "recommends"
                        | "ensures"
                        | "returns"
                        | "decreases"
                        | "opens_invariants"
                        | "no_unwind"
                        | "unwind"
                        | "where"
                ) =>
            {
                return false;
            }
            Token::Punct('(' | '[' | '<') => delimiters.push(token),
            Token::Punct('>') if i > 0 && ts[i - 1] == Token::Punct('-') => {}
            Token::Punct(close @ (')' | ']' | '>')) => {
                let open = match close {
                    ')' => '(',
                    ']' => '[',
                    _ => '<',
                };
                if delimiters.pop() != Some(&Token::Punct(open)) {
                    return false;
                }
            }
            Token::Punct('{') => return delimiters.is_empty(),
            Token::Punct(';' | '!' | '=') => return false,
            _ => {}
        }
    }
    false
}

fn functions(source: &str, module: &str) -> Vec<Function> {
    fn walk(ts: &[Token], module: &str, owner: &str, verifiable: bool, result: &mut Vec<Function>) {
        let mut i = 0;
        let mut annotated = false;
        let mut verified_item = false;
        let mut clone_derived = false;
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
                    verified_item |= token.name() == Some("verus_verify");
                    clone_derived |= token.name() == Some("Clone");
                    end += 1;
                }
                i = end;
                continue;
            }
            if verified_item && matches!(ts[i].name(), Some("struct" | "enum" | "const")) {
                let name = ts
                    .get(i + 1)
                    .and_then(Token::name)
                    .expect("unnamed verified item");
                if ts[i].name() == Some("const") || clone_derived {
                    let path = if ts[i].name() == Some("const") {
                        format!("{owner}::{name}")
                    } else {
                        format!("{owner}::{name}::clone")
                    };
                    result.push(Function {
                        module: module.into(),
                        path,
                        verifiable: true,
                        definition_only: false,
                    });
                }
            }
            if matches!(
                ts[i].name(),
                Some("struct" | "enum" | "const" | "static" | "type" | "trait" | "mod")
            ) {
                annotated = false;
                verified_item = false;
                clone_derived = false;
            }
            if ts[i].name() == Some("macro_rules") {
                let macro_name = ts.get(i + 2).and_then(Token::name).expect("unnamed macro");
                assert!(
                    matches!(
                        macro_name,
                        "id" | "kernel_snapshot"
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
                    verifiable: verifiable || annotated,
                    definition_only: ts.get(i.wrapping_sub(1)).and_then(Token::name)
                        == Some("spec")
                        && plain_definition_header(&ts[i + 2..]),
                });
            }
            if ts[i].name() == Some("fn") {
                annotated = false;
                verified_item = false;
                clone_derived = false;
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
                module == function.module
                    && name.is_none_or(|name| {
                        let relative = function
                            .path
                            .strip_prefix(&format!("{}::", function.module))
                            .unwrap();
                        name == relative
                            || name
                                .strip_prefix('*')
                                .is_some_and(|suffix| relative.ends_with(suffix))
                    })
            })
            .map(|(group, _)| group)
            .collect();
        if let Some(reason) = unverified.get(&function.path) {
            if reason.starts_with("Logical definition;") && !function.definition_only {
                errors.push(format!(
                    "{}: definition-only reason requires a nonrecursive spec function",
                    function.path
                ));
            }
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

fn source_files(directory: &Path, result: &mut BTreeMap<PathBuf, String>) {
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            source_files(&path, result);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            result.insert(path.clone(), std::fs::read_to_string(path).unwrap());
        }
    }
}

#[derive(Debug)]
struct Mount {
    module: String,
    inline: Vec<String>,
    name: Option<String>,
    path: Option<String>,
}

fn mounts(source: &str, module: &str) -> Vec<Mount> {
    all_mounts(source, module, false)
}

fn all_mounts(source: &str, module: &str, include_kani: bool) -> Vec<Mount> {
    fn walk(
        ts: &[Token],
        module: &str,
        inline: &[String],
        include_kani: bool,
        result: &mut Vec<Mount>,
    ) {
        let mut i = 0;
        let mut path = None;
        let mut kani = false;
        while i < ts.len() {
            if ts[i] == Token::Punct('#') && ts.get(i + 1) == Some(&Token::Punct('[')) {
                let start = i + 2;
                let mut end = start;
                let mut depth = 1;
                while depth > 0 {
                    match ts.get(end).expect("unterminated module attribute") {
                        Token::Punct('[') => depth += 1,
                        Token::Punct(']') => depth -= 1,
                        _ => {}
                    }
                    end += 1;
                }
                let attribute = &ts[start..end - 1];
                if attribute.first().and_then(Token::name) == Some("path") {
                    let [_, Token::Punct('='), Token::Text(value)] = attribute else {
                        panic!("unresolvable module mount: path must be a literal string");
                    };
                    assert!(
                        path.replace(value.clone()).is_none(),
                        "duplicate module path"
                    );
                }
                kani |= attribute
                    == [
                        Token::Name("cfg".into()),
                        Token::Punct('('),
                        Token::Name("kani".into()),
                        Token::Punct(')'),
                    ];
                i = end;
                continue;
            }
            if ts[i].name() == Some("macro_rules") {
                let start = (i..ts.len()).find(|&j| ts[j] == Token::Punct('{')).unwrap();
                let end = closing(ts, start);
                assert!(
                    !ts[start..end].iter().any(|t| t.name() == Some("mod")),
                    "unresolvable module mount in macro definition"
                );
                i = end + 1;
                path = None;
                kani = false;
                continue;
            }
            if ts[i].name() == Some("mod") {
                let name = ts.get(i + 1).and_then(Token::name).expect("unnamed module");
                let delimiter = ts.get(i + 2).expect("module without delimiter");
                assert!(
                    matches!(delimiter, Token::Punct(';' | '{')),
                    "unresolvable module mount: {name}"
                );
                let child = format!("{module}::{name}");
                if *delimiter == Token::Punct('{') {
                    assert!(
                        path.is_none(),
                        "unresolvable path attribute on inline module"
                    );
                    let end = closing(ts, i + 2);
                    if !kani || include_kani {
                        let mut nested = inline.to_vec();
                        nested.push(name.into());
                        walk(&ts[i + 3..end], &child, &nested, include_kani, result);
                    }
                    i = end + 1;
                } else {
                    if !kani || include_kani {
                        result.push(Mount {
                            module: child,
                            inline: inline.to_vec(),
                            name: Some(name.into()),
                            path: path.take(),
                        });
                    }
                    i += 3;
                }
                path = None;
                kani = false;
                continue;
            }
            if ts[i].name() == Some("include") && ts.get(i + 1) == Some(&Token::Punct('!')) {
                let [Token::Punct('('), Token::Text(value), Token::Punct(')')] =
                    ts.get(i + 2..i + 5).expect("unresolvable include mount")
                else {
                    panic!("unresolvable include mount: expected a literal path");
                };
                result.push(Mount {
                    module: module.into(),
                    inline: inline.to_vec(),
                    name: None,
                    path: Some(value.clone()),
                });
                i += 5;
                continue;
            }
            if matches!(
                ts[i].name(),
                Some("fn" | "struct" | "enum" | "impl" | "const" | "type" | "trait" | "union" | "static" | "extern" | "macro")
            ) || ts[i] == Token::Punct(';')
            {
                assert!(
                    path.is_none(),
                    "path attribute was not resolved to a module"
                );
                kani = false;
            }
            i += 1;
        }
        assert!(path.is_none(), "unresolved module path attribute");
    }
    let mut result = Vec::new();
    walk(&tokens(source), module, &[], include_kani, &mut result);
    result
}

fn normalized(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => assert!(result.pop(), "module path escapes its root"),
            other => result.push(other.as_os_str()),
        }
    }
    result
}

fn mount_candidates(file: &Path, mount: &Mount) -> Vec<PathBuf> {
    let parent = file.parent().unwrap();
    let nested: PathBuf = mount.inline.iter().collect();
    if let Some(path) = &mount.path {
        vec![normalized(&parent.join(&nested).join(path))]
    } else {
        let stem = file.file_stem().unwrap();
        let directory = if matches!(stem.to_str(), Some("lib" | "mod")) {
            parent.to_path_buf()
        } else {
            parent.join(stem)
        };
        let name = mount.name.as_ref().unwrap();
        vec![
            directory.join(&nested).join(format!("{name}.rs")),
            directory.join(&nested).join(name).join("mod.rs"),
        ]
    }
}

fn collect(sources: &BTreeMap<PathBuf, String>, entry: &Path) -> Vec<Function> {
    fn visit(
        sources: &BTreeMap<PathBuf, String>,
        file: &Path,
        module: &str,
        seen: &mut BTreeSet<(PathBuf, String)>,
        result: &mut Vec<Function>,
    ) {
        if !seen.insert((file.into(), module.into())) {
            return;
        }
        let source = sources.get(file).expect("unresolvable inventory source");
        result.extend(functions(source, module));
        for mount in mounts(source, module) {
            let candidates = mount_candidates(file, &mount);
            let resolved: Vec<_> = candidates
                .iter()
                .filter(|path| sources.contains_key(*path) || path.is_file())
                .collect();
            assert_eq!(
                resolved.len(),
                1,
                "unresolvable or ambiguous module mount {} from {}: {candidates:?}",
                mount.module,
                file.display()
            );
            // Preserve this inventory's src/ and verus/ scope. External test
            // fixtures are resolved, but remain outside the proof inventory.
            if sources.contains_key(resolved[0]) {
                visit(sources, resolved[0], &mount.module, seen, result);
            }
        }
    }
    let mut result = Vec::new();
    let mut seen = BTreeSet::new();
    visit(sources, entry, "kernel", &mut seen, &mut result);
    for file in sources.keys() {
        assert!(
            seen.iter().any(|(mounted, _)| mounted == file),
            "inventory source has no resolvable module mount: {}",
            file.display()
        );
    }
    result
}

#[test]
fn every_kernel_function_has_exactly_one_selector_or_a_reason() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut sources = BTreeMap::new();
    source_files(&root.join("src"), &mut sources);
    source_files(&root.join("verus"), &mut sources);
    // Kani's proof module and the outer Verus root are outside kernel::.
    sources.remove(&root.join("src/proofs.rs"));
    sources.remove(&root.join("verus/progress.rs"));
    let fs = collect(&sources, &root.join("src/lib.rs"));
    let groups: Vec<_> = selectors(super::VERUS_GROUPS)
        .into_iter()
        .filter(|(_, selector)| {
            selector == "kernel"
                || selector.starts_with("kernel#")
                || selector.starts_with("kernel::")
        })
        .collect();
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
fn module_mounts_preserve_declared_names_and_includes() {
    let root = Path::new("/inventory-mount-fixture");
    let sources = BTreeMap::from([
        (
            root.join("src/lib.rs"),
            "#[path = \"../verus/graph.rs\"] mod dependency;
            mod outer { #[path = \"write.rs\"] mod actual; }
            include!(\"defs.rs\"); mod normal; mod normal;"
                .into(),
        ),
        (
            root.join("verus/graph.rs"),
            "verus! { proof fn verify() {} }".into(),
        ),
        (
            root.join("src/outer/write.rs"),
            "verus! { proof fn write() {} }".into(),
        ),
        (
            root.join("src/defs.rs"),
            "verus! { proof fn definition() {} }".into(),
        ),
        (root.join("src/normal.rs"), "mod child;".into()),
        (
            root.join("src/normal/child.rs"),
            "verus! { proof fn check() {} }".into(),
        ),
    ]);
    let fs = collect(&sources, &root.join("src/lib.rs"));
    assert_eq!(
        fs.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
        [
            "kernel::dependency::verify",
            "kernel::outer::actual::write",
            "kernel::definition",
            "kernel::normal::child::check",
        ]
    );
    let groups = vec![
        ("graph".into(), "kernel::dependency#verify".into()),
        ("write".into(), "kernel::outer::actual#write".into()),
        ("definition".into(), "kernel#definition".into()),
        ("nested".into(), "kernel::normal::child#check".into()),
    ];
    assert!(check(&fs, &groups, &BTreeMap::new()).is_ok());
    assert!(
        check(&fs, &groups[1..], &BTreeMap::new())
            .unwrap_err()
            .contains("kernel::dependency::verify: expected exactly one Verus selector")
    );
}

#[test]
fn unresolvable_mounts_never_fall_back_to_file_names() {
    let root = Path::new("/inventory-mount-fixture");
    let sources = BTreeMap::from([(
        root.join("src/lib.rs"),
        "#[path = \"missing.rs\"] mod missing;".into(),
    )]);
    let failure = std::panic::catch_unwind(|| collect(&sources, &root.join("src/lib.rs")))
        .expect_err("a missing mount must fail the inventory");
    let message = failure.downcast_ref::<String>().unwrap();
    assert!(message.contains("unresolvable or ambiguous module mount kernel::missing"));
    assert!(
        std::panic::catch_unwind(|| mounts(
            "#[path = concat!(\"dynamic\", \".rs\")] mod dynamic;",
            "kernel"
        ))
        .is_err()
    );
}

#[test]
fn file_based_alias_reasons_are_rejected_at_actual_mounts() {
    let root = Path::new("/inventory-alias-fixture");
    let sources = BTreeMap::from([
        (
            root.join("src/lib.rs"),
            "mod ingest; mod ranks; mod reply_equivalence; mod dependencies;".into(),
        ),
        (
            root.join("src/ingest.rs"),
            "#[path = \"ingest_equivalence/mod.rs\"] mod reply_ingest_equivalence;".into(),
        ),
        (
            root.join("src/ingest_equivalence/mod.rs"),
            "fn option() {}".into(),
        ),
        (
            root.join("src/ranks.rs"),
            "#[path = \"teardown_equivalence/mod.rs\"] mod teardown_equivalence;".into(),
        ),
        (
            root.join("src/teardown_equivalence/mod.rs"),
            "fn outcome() {}".into(),
        ),
        (
            root.join("src/reply_equivalence/mod.rs"),
            "mod run_failed;".into(),
        ),
        (
            root.join("src/reply_equivalence/run_failed.rs"),
            "#[path = \"run_failed_reference.rs\"] mod reference;".into(),
        ),
        (
            root.join("src/reply_equivalence/run_failed_reference.rs"),
            "fn run_failed() {}".into(),
        ),
        (
            root.join("src/dependencies.rs"),
            "#[path = \"../verus/dependency_writes.rs\"] mod writes;".into(),
        ),
        (
            root.join("verus/dependency_writes.rs"),
            "fn write() {}".into(),
        ),
    ]);
    let fs = collect(&sources, &root.join("src/lib.rs"));
    let production = exemptions(super::VERUS_GROUPS);
    let aliases = [
        (
            "kernel::ingest::reply_ingest_equivalence::option",
            "kernel::ingest_equivalence::option",
        ),
        (
            "kernel::ranks::teardown_equivalence::outcome",
            "kernel::teardown_equivalence::outcome",
        ),
        (
            "kernel::reply_equivalence::run_failed::reference::run_failed",
            "kernel::reply_equivalence::run_failed_reference::run_failed",
        ),
        (
            "kernel::dependencies::writes::write",
            "kernel::dependency_writes::write",
        ),
    ];
    let actual: BTreeMap<_, _> = aliases
        .iter()
        .map(|(name, _)| (String::from(*name), production[*name].clone()))
        .collect();
    assert!(check(&fs, &[], &actual).is_ok());
    for (name, old) in aliases {
        let mut stale = actual.clone();
        let reason = stale.remove(name).unwrap();
        stale.insert(old.into(), reason);
        let failure = check(&fs, &[], &stale).unwrap_err();
        assert!(failure.contains(&format!(
            "{name}: ordinary Rust function needs an explicit unverified reason"
        )));
        assert!(failure.contains(&format!("stale unverified function: {old}")));
    }
}

#[test]
fn a_mount_missing_from_runtime_data_fails_loudly() {
    let root = Path::new("/inventory-runtime-fixture");
    let target = root.join("tests/mark_handled_equivalence/cases.rs");
    let mut sources = BTreeMap::from([
        (root.join("src/lib.rs"), "#[path = \"../tests/mark_handled_equivalence/cases.rs\"] mod mark_handled_equivalence;".into()),
        (target.clone(), "fn compare() {}".into()),
    ]);
    assert_eq!(
        collect(&sources, &root.join("src/lib.rs"))[0].path,
        "kernel::mark_handled_equivalence::compare"
    );
    sources.remove(&target);
    let failure = std::panic::catch_unwind(|| collect(&sources, &root.join("src/lib.rs")))
        .expect_err("a mount omitted from runtime data must fail the inventory");
    let message = failure.downcast_ref::<String>().unwrap();
    assert!(
        message.contains("unresolvable or ambiguous module mount kernel::mark_handled_equivalence")
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

#[test]
fn root_selectors_cover_constants_clones_and_qualified_methods_once() {
    let fs = functions(
        "#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)] const LIMIT: u64 = 1;          #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)] #[derive(Clone)] struct Kind {}          impl Kind { kernel_fn! { fn rank() {} } } kernel_fn! { fn rank() {} }",
        "kernel",
    );
    let groups = vec![
        ("constant".into(), "kernel#LIMIT".into()),
        ("clones".into(), "kernel#*::clone".into()),
        ("method".into(), "kernel#Kind::rank".into()),
        ("free".into(), "kernel#rank".into()),
    ];
    assert_eq!(fs.len(), 4);
    assert!(check(&fs, &groups, &BTreeMap::new()).is_ok());
    assert!(check(&fs, &groups[1..], &BTreeMap::new()).is_err());
    let mut duplicate = groups.clone();
    duplicate.push(("duplicate".into(), "kernel#*::rank".into()));
    assert!(check(&fs, &duplicate, &BTreeMap::new()).is_err());
}

fn rejects_definition_only_reason(source: &str) {
    let fs = functions(source, "kernel");
    let reason = BTreeMap::from([(
        "kernel::candidate".into(),
        "Logical definition; no standalone obligation.".into(),
    )]);
    assert!(
        check(&fs, &[], &reason)
            .expect_err(source)
            .contains("definition-only reason"),
        "{source}"
    );
}

#[test]
fn executable_functions_cannot_use_definition_only_reasons() {
    rejects_definition_only_reason("verus! { fn candidate() -> bool { true } }");
}

#[test]
fn block_contracts_cannot_hide_a_decreases_clause() {
    for clause in ["recommends", "requires"] {
        rejects_definition_only_reason(&format!(
            "verus! {{ spec fn candidate(n: nat) -> nat\n\
             {clause} if n == 0 {{ true }} else {{ n > 0 }},\n\
             decreases n,\n\
             {{ if n == 0 {{ 0 }} else {{ candidate(n - 1) }} }} }}"
        ));
    }
}

#[test]
fn later_line_decreases_cannot_use_definition_only_reasons() {
    rejects_definition_only_reason(
        "verus! { spec fn candidate(n: nat) -> nat\n\
         decreases\n n,\n\
         { if n == 0 { 0 } else { candidate(n - 1) } } }",
    );
}

#[test]
fn plain_spec_definitions_can_use_definition_only_reasons() {
    let fs = functions(
        "verus! { spec fn candidate(n: nat) -> nat { if n == 0 { 0 } else { n } } }",
        "kernel",
    );
    let reason = BTreeMap::from([(
        "kernel::candidate".into(),
        "Logical definition; no standalone obligation.".into(),
    )]);
    assert!(check(&fs, &[], &reason).is_ok());
}

#[test]
fn ambiguous_spec_headers_require_selectors() {
    for source in [
        "verus! { spec fn candidate(n: nat) -> nat recommends n > 0, { n } }",
        "verus! { spec fn candidate(n: Value<{ 1 }>) -> nat decreases 1, { 0 } }",
        "verus! { spec fn candidate(n: nat) -> Value<{ 1 }> decreases n, { candidate(n - 1) } }",
        "verus! { spec fn candidate<T>(n: T) -> nat { 0 } }",
    ] {
        rejects_definition_only_reason(source);
    }
    let fs = functions(
        "verus! { spec fn candidate(n: Seq<(nat, Seq<nat>)>) -> Seq<nat> { Seq::empty() } }",
        "kernel",
    );
    assert!(fs[0].definition_only);
}

// The escape audit also treats imports and function values as trust sites: a
// renamed assume/admit is just as trusted as a direct call.
fn trusted_markers(source: &str) -> Vec<(usize, &'static str)> {
    fn group_end(ts: &[Token], start: usize) -> usize {
        let close = match ts[start] {
            Token::Punct('(') => ')',
            Token::Punct('[') => ']',
            Token::Punct('{') => '}',
            _ => panic!("expected delimiter"),
        };
        let mut i = start + 1;
        while i < ts.len() {
            match ts[i] {
                Token::Punct(c) if c == close => return i,
                Token::Punct('(' | '[' | '{') => i = group_end(ts, i),
                _ => {}
            }
            i += 1;
        }
        panic!("unterminated source group")
    }

    fn kani_item_end(ts: &[Token], mut i: usize) -> usize {
        while i < ts.len() {
            match ts[i] {
                Token::Punct(';') => return i + 1,
                Token::Punct('{') => return group_end(ts, i) + 1,
                Token::Punct('(' | '[') => i = group_end(ts, i),
                _ => {}
            }
            i += 1;
        }
        i
    }

    fn walk(
        ts: &[Token],
        start: usize,
        end: usize,
        kani_shadowed: bool,
        hits: &mut Vec<(usize, &'static str)>,
    ) {
        let mut i = start;
        while i < end {
            if ts[i] == Token::Punct('#') {
                let mut after = i;
                let mut attributes = Vec::new();
                let mut kani = false;
                let mut inner = false;
                while after < end && ts[after] == Token::Punct('#') {
                    let mut open = after + 1;
                    let inner_attribute = ts.get(open) == Some(&Token::Punct('!'));
                    if inner_attribute {
                        open += 1;
                    }
                    if ts.get(open) != Some(&Token::Punct('[')) {
                        break;
                    }
                    let close = group_end(ts, open);
                    let attribute = &ts[open + 1..close];
                    let kani_attribute = attribute
                        == [
                            Token::Name("cfg".into()),
                            Token::Punct('('),
                            Token::Name("kani".into()),
                            Token::Punct(')'),
                        ];
                    kani |= kani_attribute;
                    inner |= inner_attribute && kani_attribute;
                    attributes.push((open + 1, close));
                    after = close + 1;
                }
                if kani {
                    if inner {
                        return;
                    }
                    i = kani_item_end(ts, after);
                    continue;
                }
                for (a, b) in attributes {
                    for (k, token) in ts.iter().enumerate().take(b).skip(a) {
                        match token.name() {
                            Some("external_body") => hits.push((k, "external_body")),
                            Some("external") => hits.push((k, "external")),
                            Some("external_fn_specification") => hits.push((k, "external_fn_specification")),
                            Some("external_type_specification") => hits.push((k, "external_type_specification")),
                            Some("external_trait_specification") => hits.push((k, "external_trait_specification")),
                            Some("external_trait_extension") => hits.push((k, "external_trait_extension")),
                            _ => {}
                        }
                    }
                }
                if after > i {
                    // Still inspect other trust names inside attribute token trees.
                    for k in i..after {
                        if let Some(name @ ("admit" | "assume" | "assume_specification")) =
                            ts[k].name()
                        {
                            hits.push((
                                k,
                                match name {
                                    "admit" => "admit",
                                    "assume" => "assume",
                                    _ => "assume_specification",
                                },
                            ));
                        }
                    }
                    i = after;
                    continue;
                }
            }
            if ts[i] == Token::Punct('{') {
                let close = group_end(ts, i);
                walk(ts, i + 1, close, kani_shadowed, hits);
                i = close + 1;
                continue;
            }
            let mut path_start = i;
            while path_start >= 3
                && ts[path_start - 1] == Token::Punct(':')
                && ts[path_start - 2] == Token::Punct(':')
                && ts[path_start - 3].name().is_some()
            {
                path_start -= 3;
            }
            let field_label = ts.get(i + 1) == Some(&Token::Punct(':'))
                && ts.get(i + 2) != Some(&Token::Punct(':'));
            let field_read = i > 0
                && ts[i - 1] == Token::Punct('.')
                && (i < 2 || ts[i - 2] != Token::Punct('.'))
                && ts.get(i + 1) != Some(&Token::Punct('('));
            match ts[i].name() {
                Some("assume")
                    if !kani_shadowed
                        && ts[path_start].name() == Some("kani")
                        && path_start < i => {}
                Some("assume") if !field_label && !field_read => hits.push((i, "assume")),
                Some("admit") if !field_label && !field_read => hits.push((i, "admit")),
                Some("assume_specification") => hits.push((i, "assume_specification")),
                Some("external_fn_specification") => hits.push((i, "external_fn_specification")),
                Some("external_type_specification") => hits.push((i, "external_type_specification")),
                Some("external_trait_specification") => hits.push((i, "external_trait_specification")),
                Some("external_trait_extension") => hits.push((i, "external_trait_extension")),
                Some("external_body" | "external")
                    if ts[path_start].name() == Some("verifier")
                        || (i >= 2
                            && ts[i - 1] == Token::Punct('(')
                            && ts[i - 2].name() == Some("verifier")) =>
                {
                    hits.push((
                        i,
                        if ts[i].name() == Some("external") {
                            "external"
                        } else {
                            "external_body"
                        },
                    ));
                }
                _ => {}
            }
            i += 1;
        }
    }

    let (ts, offsets) = located_tokens(source);
    let kani_shadowed = ts.windows(2).any(|pair| {
        pair[1].name() == Some("kani")
            && matches!(
                pair[0].name(),
                Some("mod" | "as" | "type" | "struct" | "enum" | "union" | "trait")
            )
    }) || ts.iter().enumerate().any(|(i, t)| {
        t.name() == Some("use")
            && ts[i + 1..]
                .iter()
                .take_while(|t| **t != Token::Punct(';'))
                .collect::<Vec<_>>()
                .windows(2)
                .any(|p| p[0].name() == Some("kani") && *p[1] != Token::Punct(':'))
    });
    let mut hits = Vec::new();
    walk(&ts, 0, ts.len(), kani_shadowed, &mut hits);
    hits.into_iter()
        .map(|(i, marker)| {
            (
                source[..offsets[i]].bytes().filter(|&c| c == b'\n').count() + 1,
                marker,
            )
        })
        .collect()
}

fn trusted_marker_files(directory: &Path, result: &mut BTreeMap<PathBuf, String>) {
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            trusted_marker_files(&path, result);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            result.insert(path.clone(), std::fs::read_to_string(path).unwrap());
        }
    }
}

const CONTROLLER_PROOF_INPUTS: [&str; 3] = [
    "src/kernel_boundary.rs",
    "src/dispatch_liveness/decision.rs",
    "src/dispatch_liveness/model.rs",
];

// Resolve all mounts, including cfg(kani) and cfg(verus_keep_ghost), before
// excluding Kani-only files. A missing staged source must never shrink the scan.
fn require_trusted_mounts(sources: &BTreeMap<PathBuf, String>, root: &Path) {
    let controller = root.parent().unwrap().join("controller");
    let outside: BTreeSet<_> = CONTROLLER_PROOF_INPUTS.iter()
        .map(|path| controller.join(path)).collect();
    for (file, source) in sources {
        for mount in all_mounts(source, "kernel", true) {
            let candidates = mount_candidates(file, &mount);
            // This is the sole test-only mount in lib.rs, compiled out of Verus.
            if candidates == [root.join("tests/mark_handled_equivalence/cases.rs")] {
                continue;
            }
            assert!(candidates.iter().all(|path|
                path.starts_with(root.join("src")) || path.starts_with(root.join("verus"))
                    || outside.contains(path)),
                "unlisted external proof mount from {}: {:?}", file.display(), candidates);
            assert!(candidates.iter().any(|path| sources.contains_key(path)),
                "trusted-marker input not staged: {} mounts {:?}", file.display(), candidates);
        }
    }
}

// An out-of-line Kani module is excluded only if it has no non-Kani route.
// Unmounted files remain scanned, including the outer Verus proof root.
fn kani_only_files(sources: &BTreeMap<PathBuf, String>, root: &Path) -> BTreeSet<PathBuf> {
    fn reachable(
        sources: &BTreeMap<PathBuf, String>,
        entry: &Path,
        include_kani: bool,
    ) -> BTreeSet<PathBuf> {
        let mut seen = BTreeSet::new();
        let mut pending = vec![entry.to_path_buf()];
        while let Some(file) = pending.pop() {
            if !seen.insert(file.clone()) {
                continue;
            }
            let Some(source) = sources.get(&file) else {
                continue;
            };
            for mount in all_mounts(source, "kernel", include_kani) {
                pending.extend(
                    mount_candidates(&file, &mount)
                        .into_iter()
                        .filter(|p| sources.contains_key(p)),
                );
            }
        }
        seen
    }
    let mut all = BTreeSet::new();
    let mut active = BTreeSet::new();
    for entry in [root.join("src/lib.rs"), root.join("verus/progress.rs")] {
        all.extend(reachable(sources, &entry, true));
        active.extend(reachable(sources, &entry, false));
    }
    all.difference(&active).cloned().collect()
}

#[test]
fn no_trusted_markers() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut sources = BTreeMap::new();
    trusted_marker_files(&root.join("src"), &mut sources);
    trusted_marker_files(&root.join("verus"), &mut sources);
    assert!(
        sources.contains_key(&root.join("verus/progress.rs")),
        "Verus root missing from test inputs"
    );
    for path in CONTROLLER_PROOF_INPUTS {
        let file = root.parent().unwrap().join("controller").join(path);
        sources.insert(file.clone(), std::fs::read_to_string(&file)
            .unwrap_or_else(|error| panic!("trusted-marker input not staged: {}: {error}", file.display())));
    }
    require_trusted_mounts(&sources, root);
    let kani = kani_only_files(&sources, root);
    let mut errors = Vec::new();
    for (path, source) in &sources {
        if kani.contains(path) {
            continue;
        }
        for (line, marker) in trusted_markers(source) {
            errors.push(format!(
                "crates/{}:{line}: trusted marker {marker}",
                path.strip_prefix(root.parent().unwrap()).unwrap().display()
            ));
        }
    }
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    println!(
        "Trusted-marker inventory: {} files, {} Kani-only files excluded",
        sources.len(),
        kani.len()
    );
}

#[test]
fn trusted_marker_detection_covers_attributes_calls_and_aliases() {
    for marker in [
        "#[verifier::external_body] fn f() {}",
        "#[verifier(external_body)] fn f() {}",
        "#[verifier::external] fn f() {}",
        "#[verifier::external_fn_specification] fn f() {}",
        "#[verifier::external_type_specification] struct S;",
        "#[verifier::external_trait_specification] trait T {}",
        "#[verifier::external_trait_extension] trait T {}",
        "verifier(external_trait_extension)",
        "#[cfg_attr(verus_keep_ghost, verifier(external_body))] fn f() {}",
        "verifier(external_body)",
        "admit()",
        "vstd::prelude::admit()",
        "assume(false)",
        "builtin::assume(false)",
        "assume_specification [f] () {}",
        "use builtin::assume as trust;",
        "use builtin::{admit as trust};",
        "r#assume(false)",
    ] {
        assert!(!trusted_markers(marker).is_empty(), "missed {marker}");
    }
    assert_eq!(
        trusted_markers("// admit()\n\n#[verifier::external_body]\nfn f() {}"),
        [(3, "external_body")]
    );
}

#[test]
fn trusted_marker_detection_ignores_noise_and_kani_but_not_neighbors() {
    let source = r####"
// #[verifier::external_body] admit()
/* assume(false); /* #[verifier::external] */ assume_specification */
let text = "admit() assume(false)";
let raw = r###"#[verifier::external_body]"###;
kani::assume(false);
#[cfg(kani)] #[verifier::external_body] fn kani_only() { assume(false); admit(); }
#[cfg(kani)] { assume(false); }
#[allow(dead_code)] #[cfg(kani)] mod bounded { assume_specification [f] () {} }
#[cfg(not(kani))] fn production() { assume(false); }
"####;
    assert_eq!(trusted_markers(source), [(10, "assume")]);
    assert!(trusted_markers("#![cfg(kani)] #[verifier::external] fn f() { admit(); }").is_empty());
    assert!(!trusted_markers("#[cfg(kani)] fn f() { admit(); } assume(false);").is_empty());
    assert!(!trusted_markers("other::kani::assume(false);").is_empty());
    assert!(!trusted_markers("mod kani {} kani::assume(false);").is_empty());
    assert!(
        !trusted_markers("#![allow(dead_code)] #[cfg(kani)] fn f() {} assume(false);").is_empty()
    );
    assert!(trusted_markers("struct Context { admit: bool } let c = Context { admit: true }; c.admit = false; !c.admit;").is_empty());
    assert!(!trusted_markers("c.admit();").is_empty());
}

#[test]
fn kani_file_exclusion_requires_an_exclusively_kani_route() {
    let root = Path::new("/trusted-fixture");
    let mut sources = BTreeMap::from([
        (root.join("src/lib.rs"), "#[cfg(kani)] mod bounded;".into()),
        (root.join("src/bounded.rs"), "mod nested; admit();".into()),
        (root.join("src/bounded/nested.rs"), "assume(false);".into()),
        (root.join("verus/unmounted.rs"), "admit();".into()),
    ]);
    assert_eq!(
        kani_only_files(&sources, root),
        BTreeSet::from([
            root.join("src/bounded.rs"),
            root.join("src/bounded/nested.rs")
        ])
    );
    sources
        .get_mut(&root.join("src/lib.rs"))
        .unwrap()
        .push_str(" #[path = \"bounded.rs\"] mod live;");
    assert!(kani_only_files(&sources, root).is_empty());
}

#[test]
#[should_panic(expected = "trusted-marker input not staged")]
fn trusted_marker_missing_mounted_source_fails_closed() {
    let root = Path::new("/trusted-fixture/kernel");
    require_trusted_mounts(&BTreeMap::from([
        (root.join("src/lib.rs"), "#[path = \"../verus/nested/proof.rs\"] mod proof;".into()),
    ]), root);
}

#[test]
fn trusted_marker_controller_inputs_are_scanned() {
    let root = Path::new("/trusted-fixture/kernel");
    for path in CONTROLLER_PROOF_INPUTS {
        let file = root.parent().unwrap().join("controller").join(path);
        let sources = BTreeMap::from([
            (root.join("verus/progress.rs"), format!("#[path = \"../../controller/{path}\"] mod proof;")),
            (file.clone(), "#[verifier::external_body] fn f() {}".into()),
        ]);
        require_trusted_mounts(&sources, root);
        assert_eq!(trusted_markers(&sources[&file]), [(1, "external_body")]);
    }
}

#[test]
fn kani_attribute_does_not_leak_past_other_item_kinds() {
    let root = Path::new("/trusted-fixture");
    for item in ["trait T {}", "union U { a: u8 }", "static S: u8 = 0;", "extern \"C\" {}"] {
        let sources = BTreeMap::from([
            (root.join("src/lib.rs"), format!("#[cfg(kani)] {item} mod real;")),
            (root.join("src/real.rs"), "admit();".into()),
        ]);
        assert!(kani_only_files(&sources, root).is_empty(), "{item}");
    }
}
