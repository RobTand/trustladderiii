use base64::{Engine, engine::general_purpose::STANDARD};
use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::{self as ast, Expr, ExprContext, Stmt, comparable::ComparableStmt};
use ruff_python_parser::parse_module;
use ruff_text_size::Ranged;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, BufRead, Write},
    time::Instant,
};

type Set = BTreeSet<String>;
type Tree = BTreeMap<String, File>;
type R<T> = Result<T, Refusal>;
type SourceUnit = (String, Option<(usize, usize)>);
#[derive(Clone)]
struct File {
    bytes: Vec<u8>,
    mode: u64,
}
#[derive(Debug)]
struct Refusal {
    rule: &'static str,
    module: String,
    symbol: String,
}
fn refusal(rule: &'static str, module: &str, symbol: &str) -> Refusal {
    Refusal {
        rule,
        module: module.into(),
        symbol: symbol.into(),
    }
}
fn hash(bytes: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(bytes.as_ref()))
}
fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}
fn read_tree(v: &Value) -> Tree {
    v.as_object()
        .unwrap()
        .iter()
        .map(|(name, data)| {
            (
                name.clone(),
                File {
                    bytes: STANDARD.decode(data["b64"].as_str().unwrap()).unwrap(),
                    mode: data["mode"].as_u64().unwrap(),
                },
            )
        })
        .collect()
}
fn tree_hash(tree: &Tree) -> String {
    let mut digest = Sha256::new();
    for (name, file) in tree {
        let mode = file.mode.to_string();
        for value in [name.as_bytes(), mode.as_bytes(), &file.bytes] {
            digest.update((value.len() as u64).to_be_bytes());
            digest.update(value);
        }
    }
    format!("{:x}", digest.finalize())
}
fn text(tree: &Tree, name: &str) -> Option<String> {
    tree.get(name)
        .map(|f| String::from_utf8_lossy(&f.bytes).into_owned())
}
fn parse(source: &str, name: &str) -> R<Vec<Stmt>> {
    parse_module(source)
        .map(|m| m.into_syntax().body)
        .map_err(|_| refusal("proof.source.unparsable", name, ""))
}
fn dedent(source: &str) -> String {
    let width = source
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start_matches([' ', '\t']).len())
        .min()
        .unwrap_or(0);
    source
        .split_inclusive('\n')
        .map(|l| {
            if l.trim().is_empty() {
                "\n"
            } else {
                &l[width.min(l.len())..]
            }
        })
        .collect()
}
fn name(stmt: &Stmt) -> Option<&str> {
    match stmt {
        Stmt::FunctionDef(n) => Some(n.name.as_str()),
        Stmt::ClassDef(n) => Some(n.name.as_str()),
        _ => None,
    }
}
fn line_span(source: &str, stmt: &Stmt) -> (usize, usize) {
    let mut start = usize::from(stmt.start());
    let end = usize::from(stmt.end());
    let decorators = match stmt {
        Stmt::FunctionDef(n) => &n.decorator_list[..],
        Stmt::ClassDef(n) => &n.decorator_list[..],
        _ => &[],
    };
    for d in decorators {
        start = start.min(usize::from(d.start()));
    }
    let first = source[..start].bytes().filter(|c| *c == b'\n').count();
    let last = source[..end].bytes().filter(|c| *c == b'\n').count() + 1;
    (first, last)
}
fn source_unit(
    source: &str,
    identity: &str,
    optional: bool,
) -> R<Option<SourceUnit>> {
    let parts: Vec<_> = identity.split("::").collect();
    if parts.len() == 1 {
        return Ok(Some((source.to_owned(), None)));
    }
    let parsed = parse(source, parts[0])?;
    let mut body = &parsed[..];
    let mut selected = None;
    for part in &parts[1..] {
        let matches: Vec<_> = body
            .iter()
            .filter(|s| {
                name(s) == Some(*part)
                    || match s {
                        Stmt::Assign(_) => ast_bound(std::slice::from_ref(*s)).contains(*part),
                        Stmt::AnnAssign(n) => {
                            matches!(n.target.as_ref(), Expr::Name(n) if n.id.as_str() == *part)
                        }
                        _ => false,
                    }
            })
            .collect();
        if matches.is_empty() && optional {
            return Ok(None);
        }
        if matches.len() != 1 {
            return Err(refusal("proof.source.definition", parts[0], identity));
        }
        let stmt = matches[0];
        selected = Some(stmt);
        body = match stmt {
            Stmt::FunctionDef(n) => &n.body,
            Stmt::ClassDef(n) => &n.body,
            _ => &[],
        };
    }
    let span = line_span(source, selected.unwrap());
    let lines: Vec<_> = source.split_inclusive('\n').collect();
    Ok(Some((
        lines[span.0..span.1.min(lines.len())].concat(),
        Some(span),
    )))
}
fn unit(tree: &Tree, identity: &str) -> R<Option<String>> {
    text(tree, identity.split("::").next().unwrap())
        .map(|s| source_unit(&s, identity, true).map(|u| u.map(|v| v.0)))
        .unwrap_or(Ok(None))
}
fn overlaps(a: &str, b: &str) -> bool {
    a == b || a.starts_with(&format!("{b}::")) || b.starts_with(&format!("{a}::"))
}
fn norm(source: &str) -> String {
    ruff_python_codegen::round_trip(source)
        .unwrap_or_else(|_| source.into())
        .trim()
        .to_owned()
}
fn stmt_text(source: &str, stmt: &Stmt) -> String {
    source[usize::from(stmt.start())..usize::from(stmt.end())].to_owned()
}

fn ast_bound(body: &[Stmt]) -> Set {
    struct Bound(Set);
    impl<'a> Visitor<'a> for Bound {
        fn visit_expr(&mut self, e: &'a Expr) {
            if let Expr::Name(n)=e && n.ctx==ExprContext::Store {self.0.insert(n.id.to_string());}
            visitor::walk_expr(self,e);
        }
    }
    let mut result=Bound(Set::new());result.visit_body(body);result.0
}

#[derive(Default)]
struct Names {
    bound: Set,
    loaded: Set,
    globals: Set,
    nonlocals: Set,
}
impl<'a> Visitor<'a> for Names {
    fn visit_expr(&mut self, expr: &'a Expr) {
        if let Expr::Name(n) = expr {
            if n.ctx == ExprContext::Load {
                self.loaded.insert(n.id.to_string());
            } else {
                self.bound.insert(n.id.to_string());
            }
        }
        visitor::walk_expr(self, expr);
    }
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        if let Some(n) = name(stmt) {
            self.bound.insert(n.into());
        }
        match stmt {
            Stmt::Global(n) => self.globals.extend(n.names.iter().map(ToString::to_string)),
            Stmt::Nonlocal(n) => self
                .nonlocals
                .extend(n.names.iter().map(ToString::to_string)),
            _ => {}
        }
        visitor::walk_stmt(self, stmt);
    }
    fn visit_parameter(&mut self, p: &'a ast::Parameter) {
        self.bound.insert(p.name.to_string());
        visitor::walk_parameter(self, p);
    }
    fn visit_alias(&mut self, a: &'a ast::Alias) {
        self.bound.insert(a.asname.as_ref().map_or_else(
            || a.name.split('.').next().unwrap().to_owned(),
            ToString::to_string,
        ));
    }
    fn visit_pattern(&mut self, p: &'a ast::Pattern) {
        match p {
            ast::Pattern::MatchAs(n) => {
                if let Some(n) = &n.name {
                    self.bound.insert(n.to_string());
                }
            }
            ast::Pattern::MatchStar(n) => {
                if let Some(n) = &n.name {
                    self.bound.insert(n.to_string());
                }
            }
            ast::Pattern::MatchMapping(n) => {
                if let Some(n) = &n.rest {
                    self.bound.insert(n.to_string());
                }
            }
            _ => {}
        }
        visitor::walk_pattern(self, p);
    }
}
fn names(body: &[Stmt]) -> Names {
    let mut n = Names::default();
    n.visit_body(body);
    n
}
fn binding_names(source: &str, module: &str) -> R<Set> {
    let n = names(&parse(&dedent(source), module)?);
    Ok(n.bound
        .union(&n.globals)
        .cloned()
        .chain(n.nonlocals)
        .collect())
}
const BUILTINS: &str = "abs all any ascii bin bool breakpoint bytearray bytes callable chr classmethod compile complex delattr dict dir divmod enumerate eval exec filter float format frozenset getattr globals hasattr hash help hex id input int isinstance issubclass iter len list locals map max memoryview min next object oct open ord pow print property range repr reversed round set setattr slice sorted staticmethod str sum super tuple type vars zip __import__ Exception AssertionError ValueError RuntimeError TypeError KeyError NameError ImportError OSError StopIteration NotImplementedError FileNotFoundError TimeoutError PermissionError SystemExit BaseException KeyboardInterrupt ArithmeticError AttributeError BufferError ChildProcessError ConnectionError EOFError FloatingPointError GeneratorExit IndexError InterruptedError IsADirectoryError LookupError MemoryError ModuleNotFoundError NotADirectoryError OverflowError ProcessLookupError RecursionError ReferenceError SyntaxError SystemError UnboundLocalError UnicodeError UnicodeDecodeError UnicodeEncodeError Warning ZeroDivisionError True False None __file__ __name__ __package__ __doc__ __annotations__";
const PURE: &str = "abs all any bool bytes enumerate float frozenset int len max min range repr reversed round sorted str sum tuple zip";
fn contains(words: &str, name: &str) -> bool {
    words.split_whitespace().any(|w| w == name)
}
// Approximate lexical scopes explicitly; Ruff's parser does not expose CPython symtable.
fn global_names(source: &str, module: &str) -> R<Set> {
    struct Scope {
        local: Set,
        loaded: Set,
        global: Set,
        children: Vec<Scope>,
    }
    fn scope(body: &[Stmt], params: Set) -> Scope {
        struct Scan {
            result: Scope,
        }
        impl<'a> Visitor<'a> for Scan {
            fn visit_expr(&mut self, e: &'a Expr) {
                if let Expr::Name(n) = e {
                    if n.ctx == ExprContext::Load {
                        self.result.loaded.insert(n.id.to_string());
                    } else {
                        self.result.local.insert(n.id.to_string());
                    }
                }
                visitor::walk_expr(self, e);
            }
            fn visit_alias(&mut self, a: &'a ast::Alias) {
                self.result.local.insert(a.asname.as_ref().map_or_else(
                    || a.name.split('.').next().unwrap().to_owned(),
                    ToString::to_string,
                ));
            }
            fn visit_stmt(&mut self, s: &'a Stmt) {
                match s {
                    Stmt::FunctionDef(n) => {
                        self.result.local.insert(n.name.to_string());
                        for d in &n.decorator_list {
                            self.visit_expr(&d.expression);
                        }
                        self.result.children.push(scope(
                            &n.body,
                            n.parameters.iter().map(|p| p.name().to_string()).collect(),
                        ));
                    }
                    Stmt::ClassDef(n) => {
                        self.result.local.insert(n.name.to_string());
                        for d in &n.decorator_list {
                            self.visit_expr(&d.expression);
                        }
                        self.result.children.push(scope(&n.body, Set::new()));
                    }
                    Stmt::Global(n) => self
                        .result
                        .global
                        .extend(n.names.iter().map(ToString::to_string)),
                    _ => visitor::walk_stmt(self, s),
                }
            }
        }
        let mut s = Scan {
            result: Scope {
                local: params,
                loaded: Set::new(),
                global: Set::new(),
                children: vec![],
            },
        };
        s.visit_body(body);
        s.result
    }
    fn collect(s: &Scope, closure: &Set, module: bool, result: &mut Set) {
        for n in &s.loaded {
            if s.global.contains(n) || (!s.local.contains(n) && !closure.contains(n)) || module {
                result.insert(n.clone());
            }
        }
        let mut next = closure.clone();
        if !module {
            next.extend(s.local.iter().cloned());
        }
        for child in &s.children {
            collect(child, &next, false, result);
        }
    }
    let s = scope(&parse(&dedent(source), module)?, Set::new());
    let mut result = Set::new();
    collect(&s, &Set::new(), true, &mut result);
    result.retain(|n| !contains(BUILTINS, n));
    Ok(result)
}
fn written(source: &str, module: &str, namespace: &[String]) -> R<Set> {
    struct Writes {
        path: Vec<String>,
        function: usize,
        out: Set,
    }
    impl<'a> Visitor<'a> for Writes {
        fn visit_stmt(&mut self, s: &'a Stmt) {
            if let Stmt::Global(n) = s {
                self.out.extend(n.names.iter().map(ToString::to_string));
            }
            if let Some(n) = name(s) {
                if self.function == 0 {
                    self.out.insert(
                        self.path
                            .iter()
                            .map(String::as_str)
                            .chain([n])
                            .collect::<Vec<_>>()
                            .join("::"),
                    );
                }
            }
            match s {
                Stmt::FunctionDef(n) => {
                    self.function += 1;
                    self.visit_body(&n.body);
                    self.function -= 1;
                }
                Stmt::ClassDef(n) if self.function == 0 => {
                    self.path.push(n.name.to_string());
                    self.visit_body(&n.body);
                    self.path.pop();
                }
                _ => visitor::walk_stmt(self, s),
            }
        }
        fn visit_expr(&mut self, e: &'a Expr) {
            if self.function == 0 {
                if let Expr::Name(n) = e {
                    if n.ctx == ExprContext::Store {
                        self.out.insert(
                            self.path
                                .iter()
                                .map(String::as_str)
                                .chain([n.id.as_str()])
                                .collect::<Vec<_>>()
                                .join("::"),
                        );
                    }
                }
            }
            visitor::walk_expr(self, e);
        }
        fn visit_alias(&mut self, a: &'a ast::Alias) {
            if self.function == 0 {
                let n = a.asname.as_ref().map_or_else(
                    || a.name.split('.').next().unwrap().to_owned(),
                    ToString::to_string,
                );
                self.out.insert(
                    self.path
                        .iter()
                        .map(String::as_str)
                        .chain([n.as_str()])
                        .collect::<Vec<_>>()
                        .join("::"),
                );
            }
        }
    }
    let mut v = Writes {
        path: namespace.to_vec(),
        function: 0,
        out: Set::new(),
    };
    v.visit_body(&parse(&dedent(source), module)?);
    Ok(v.out)
}
// literal_eval shape plus recursive hashability. Does not execute Python objects.
fn literal(e: &Expr) -> Option<bool> {
    match e {
        Expr::StringLiteral(_)
        | Expr::BytesLiteral(_)
        | Expr::NumberLiteral(_)
        | Expr::BooleanLiteral(_)
        | Expr::NoneLiteral(_)
        | Expr::EllipsisLiteral(_) => Some(true),
        Expr::UnaryOp(n) if matches!(n.operand.as_ref(), Expr::NumberLiteral(_)) => Some(true),
        Expr::Tuple(n) => n
            .elts
            .iter()
            .map(literal)
            .collect::<Option<Vec<_>>>()
            .map(|v| v.iter().all(|b| *b)),
        Expr::List(n) => n
            .elts
            .iter()
            .map(literal)
            .collect::<Option<Vec<_>>>()
            .map(|_| false),
        Expr::Set(n) => n
            .elts
            .iter()
            .map(literal)
            .collect::<Option<Vec<_>>>()
            .map(|_| false),
        Expr::Dict(n) => n
            .iter()
            .map(|item| {
                item.key
                    .as_ref()
                    .and_then(literal)
                    .and_then(|_| literal(&item.value))
            })
            .collect::<Option<Vec<_>>>()
            .map(|_| false),
        Expr::Call(n)
            if matches!(n.func.as_ref(), Expr::Name(n) if n.id.as_str()=="set")
                && n.arguments.args.is_empty()
                && n.arguments.keywords.is_empty() =>
        {
            Some(false)
        }
        Expr::BinOp(n)
            if matches!(n.op, ast::Operator::Add | ast::Operator::Sub)
                && matches!(n.left.as_ref(), Expr::NumberLiteral(_) | Expr::UnaryOp(_))
                && matches!(n.right.as_ref(), Expr::NumberLiteral(_)) =>
        {
            Some(true)
        }
        _ => None,
    }
}
fn sole_import(body: &[Stmt], symbol: &str, template: &str, source: &str) -> bool {
    let binding: Vec<_> = body
        .iter()
        .filter(|s| {
            binding_names(&stmt_text(source, s), "<import>").is_ok_and(|n| n.contains(symbol))
        })
        .collect();
    binding.len() == 1
        && parse_module(template)
            .is_ok_and(|m| ComparableStmt::from(binding[0]) == ComparableStmt::from(&m.suite()[0]))
}
fn allowed_loader(
    body: &[Stmt],
    source: &str,
    module: &str,
    subjects: &[String],
) -> Vec<(usize, usize)> {
    let mut allowed = vec![];
    if sole_import(body, "Path", "from pathlib import Path", source)
        && !binding_names(source, module)
            .unwrap_or_default()
            .contains("__file__")
    {
        for stmt in body {
            if let Stmt::FunctionDef(n) = stmt {
                for subject in subjects {
                    let template = format!(
                        "def {}():\n    import importlib.util\n    path = Path(__file__).resolve().parents[{}] / {:?}\n    spec = importlib.util.spec_from_file_location(\"proof_subject\", path)\n    module = importlib.util.module_from_spec(spec)\n    spec.loader.exec_module(module)\n    return module\n",
                        n.name,
                        module.split('/').count() - 1,
                        subject.split("::").next().unwrap()
                    );
                    if let Ok(mut t) = parse(&template, module) {
                        if let Stmt::FunctionDef(tn) = &mut t[0] {
                            if n.body.len() == 6 {
                                if let (Stmt::Assign(a), Stmt::Assign(b)) =
                                    (&n.body[2], &mut tn.body[2])
                                {
                                    if let (Expr::Call(a), Expr::Call(b)) =
                                        (a.value.as_ref(), b.value.as_mut())
                                    {
                                        if let Some(first) = a.arguments.args.first() {
                                            b.arguments.args[0] = first.clone();
                                        }
                                    }
                                }
                            }
                        }
                        if ComparableStmt::from(stmt) == ComparableStmt::from(&t[0]) {
                            allowed.push((usize::from(stmt.start()), usize::from(stmt.end())));
                        }
                    }
                }
            }
        }
    }
    // The narrow sys.executable and mock.patch.object setup is deliberately conservative.
    allowed
}
const NS_NAMES: &str = "globals locals vars eval exec compile __import__ getattr setattr delattr __builtins__ attrgetter methodcaller";
const NS_ATTRS: &str = "__dict__ __globals__ __builtins__ __getattribute__ __setattr__ __delattr__ f_globals f_locals gi_frame cr_frame tb_frame _getframe currentframe modules attrgetter methodcaller";
const NS_MODULES: &str = "builtins sys inspect importlib __main__";
const REFLECT: &str = "format format_map Formatter TestCase main";
fn root(expr: &Expr) -> &Expr {
    match expr {
        Expr::Attribute(n) => root(&n.value),
        Expr::Subscript(n) => root(&n.value),
        _ => expr,
    }
}
fn namespace(body: &[Stmt], module: &str, allowed: &[(usize, usize)]) -> R<()> {
    struct Scan<'b> {
        module: &'b str,
        allowed: &'b [(usize, usize)],
        events: Vec<(usize, usize, Refusal)>,
        depth: usize,
        sequence: usize,
    }
    impl Scan<'_> {
        fn event(&mut self, rule: &'static str, symbol: &str) {
            self.events.push((
                self.depth,
                self.sequence,
                refusal(rule, self.module, symbol),
            ));
            self.sequence += 1;
        }
    }
    impl<'a> Visitor<'a> for Scan<'_> {
        fn visit_stmt(&mut self, s: &'a Stmt) {
            match s {
                Stmt::ImportFrom(n) => {
                    if n.names.iter().any(|a| a.name.as_str() == "*") {
                        self.event("proof.support.wildcard_import", "");
                    }
                    let imported = n.module.as_ref().map_or("", |m| m.as_str());
                    if ["operator", "_operator"].contains(&imported)
                        && n.names.iter().any(|a| contains(NS_NAMES, a.name.as_str()))
                    || contains(NS_MODULES, imported.split('.').next().unwrap())
                        && !self.allowed.iter().any(|(a, b)| {
                            usize::from(s.start()) >= *a && usize::from(s.end()) <= *b
                        })
                    {
                        self.event("proof.support.namespace_api", "");
                    }
                }
                Stmt::Import(n) => {
                    if n.names
                        .iter()
                        .any(|a| contains(NS_MODULES, a.name.split('.').next().unwrap()))
                        && !self.allowed.iter().any(|(a, b)| {
                            usize::from(s.start()) >= *a && usize::from(s.end()) <= *b
                        })
                    {
                        self.event("proof.support.namespace_api", "");
                    }
                }
                _ => {}
            }
            self.depth += 1;
            visitor::walk_stmt(self, s);
            self.depth -= 1;
        }
        fn visit_expr(&mut self, e: &'a Expr) {
            if matches!(e, Expr::Name(n) if contains(NS_NAMES,n.id.as_str()))
                || matches!(e, Expr::Attribute(n) if contains(NS_ATTRS,n.attr.as_str()))
            {
                self.event("proof.support.namespace_api", "");
            }
            self.depth += 1;
            visitor::walk_expr(self, e);
            self.depth -= 1;
        }
    }
    let mut scan = Scan {
        module,
        allowed,
        events: vec![],
        depth: 0,
        sequence: 0,
    };
    scan.visit_body(body);
    scan.events.sort_by_key(|(d, s, _)| (*d, *s));
    scan.events
        .into_iter()
        .next()
        .map_or(Ok(()), |(_, _, e)| Err(e))
}
fn support_calls(
    body: &[Stmt],
    source: &str,
    module: &str,
    allowed: &[(usize, usize)],
    context: &[String],
) -> R<()> {
    let shadows: Set = context
        .iter()
        .flat_map(|s| binding_names(s, module).unwrap_or_default())
        .collect();
    let pure: Set = PURE
        .split_whitespace()
        .filter(|n| !shadows.contains(*n))
        .map(str::to_owned)
        .collect();
    let funcs: Set = body
        .iter()
        .filter_map(|s| {
            if let Stmt::FunctionDef(n) = s {
                let count = body
                    .iter()
                    .filter(|s| {
                        binding_names(&stmt_text(source, s), module)
                            .unwrap_or_default()
                            .contains(n.name.as_str())
                    })
                    .count();
                let nested = names(&n.body);
                if count == 1
                    && !nested.bound.contains(n.name.as_str())
                    && !n.parameters.iter().any(|p| p.name() == &n.name)
                {
                    Some(n.name.to_string())
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect();
    fn pure_value(e: &Expr, pure: &Set) -> bool {
        if literal(e).is_some() && !matches!(e, Expr::Call(_)) {
            return true;
        }
        matches!(e,Expr::Call(n) if matches!(n.func.as_ref(),Expr::Name(n) if pure.contains(n.id.as_str())) && n.arguments.keywords.is_empty() && n.arguments.args.iter().all(|e| pure_value(e,pure)))
    }
    struct Scan<'b> {
        module: &'b str,
        allowed: &'b [(usize, usize)],
        pure: Set,
        funcs: Set,
        events: Vec<(usize, usize, Refusal)>,
        depth: usize,
        seq: usize,
    }
    impl Scan<'_> {
        fn event(&mut self, rule: &'static str, symbol: &str) {
            self.events
                .push((self.depth, self.seq, refusal(rule, self.module, symbol)));
            self.seq += 1;
        }
        fn allowed(&self, node: impl Ranged) -> bool {
            self.allowed
                .iter()
                .any(|(a, b)| usize::from(node.start()) >= *a && usize::from(node.end()) <= *b)
        }
    }
    impl<'a> Visitor<'a> for Scan<'_> {
        fn visit_stmt(&mut self, s: &'a Stmt) {
            if !self.allowed(s) {
                match s {
                    Stmt::FunctionDef(n) if !n.decorator_list.is_empty() => {
                        self.event("proof.support.decorator", n.name.as_str())
                    }
                    Stmt::ClassDef(n) => {
                        if !n.decorator_list.is_empty() {
                            self.event("proof.support.decorator", n.name.as_str());
                        }
                        if n.arguments
                            .as_ref()
                            .is_some_and(|a| !a.args.is_empty() || !a.keywords.is_empty())
                        {
                            self.event("proof.support.class_bases", n.name.as_str());
                        }
                    }
                    Stmt::With(_) => self.event("proof.support.context_manager", ""),
                    _ => {}
                }
            }
            self.depth += 1;
            visitor::walk_stmt(self, s);
            self.depth -= 1;
        }
        fn visit_expr(&mut self, e: &'a Expr) {
            if !self.allowed(e) {
                match e {
                    Expr::Call(n) => {
                        let direct = matches!(n.func.as_ref(),Expr::Name(n) if self.funcs.contains(n.id.as_str()));
                        if !direct && !pure_value(e, &self.pure) {
                            self.event("proof.support.call_allowlist", "");
                        }
                    }
                    Expr::BinOp(n) if n.op == ast::Operator::Mod => {
                        let safe = matches!(
                            n.left.as_ref(),
                            Expr::StringLiteral(_) | Expr::BytesLiteral(_)
                        ) && !matches!(n.left.as_ref(), Expr::StringLiteral(n) if n.value.to_str().contains("__")) && literal(&n.left).is_some()
                            && literal(&n.right).is_some();
                        if !safe {
                            self.event("proof.support.percent_format", "");
                        }
                    }
                    Expr::Attribute(n) if contains(REFLECT, n.attr.as_str()) => {
                        self.event("proof.support.reflective_attribute", "")
                    }
                    _ => {}
                }
            }
            self.depth += 1;
            visitor::walk_expr(self, e);
            self.depth -= 1;
        }
    }
    let mut scan = Scan {
        module,
        allowed,
        pure,
        funcs,
        events: vec![],
        depth: 0,
        seq: 0,
    };
    scan.visit_body(body);
    scan.events.sort_by_key(|(d, s, _)| (*d, *s));
    scan.events
        .into_iter()
        .next()
        .map_or(Ok(()), |(_, _, e)| Err(e))
}
fn validate_support(
    source: &str,
    module: &str,
    observer_context: bool,
    subjects: &[String],
    context: &[String],
) -> R<()> {
    let source = dedent(source);
    let body = parse(&source, module)?;
    let allowed = if observer_context {
        allowed_loader(&body, &source, module, subjects)
    } else {
        vec![]
    };
    namespace(&body, module, &allowed)?;
    support_calls(&body, &source, module, &allowed, context)?;
    struct Writes {
        found: bool,
    }
    impl<'a> Visitor<'a> for Writes {
        fn visit_expr(&mut self, e: &'a Expr) {
            if matches!(e,Expr::Attribute(n) if n.ctx!=ExprContext::Load)
                || matches!(e,Expr::Subscript(n) if n.ctx!=ExprContext::Load)
            {
                self.found = true;
            }
            visitor::walk_expr(self, e);
        }
    }
    let mut w = Writes { found: false };
    w.visit_body(&body);
    if w.found {
        return Err(refusal("proof.support.member_write", module, ""));
    }
    Ok(())
}
fn partial(source: &str, module_source: &str, module: &str, symbol: &str) -> R<()> {
    let body = parse(&dedent(source), module)?;
    for s in &body {
        let is_def = name(s).is_some();
        if is_def {
            let n = name(s).unwrap();
            let indent = source.len() - source.trim_start_matches([' ', '\t']).len();
            let stripped =
                module_source.replacen(source, &format!("{}pass\n", " ".repeat(indent)), 1);
            if stripped == module_source || binding_names(&stripped, module)?.contains(n) {
                return Err(refusal("proof.support.definition_binding", module, n));
            }
            struct Uses<'b> {
                symbol: &'b str,
                wrong: bool,
                write: bool,
            }
            impl<'a> Visitor<'a> for Uses<'_> {
                fn visit_expr(&mut self, e: &'a Expr) {
                    if matches!(e,Expr::Name(n) if n.id.as_str()==self.symbol) {
                        self.wrong = true;
                    }
                    if matches!(e,Expr::Attribute(n) if n.ctx!=ExprContext::Load)
                        || matches!(e,Expr::Subscript(n) if n.ctx!=ExprContext::Load)
                    {
                        if matches!(root(e),Expr::Name(n) if n.id.as_str()==self.symbol) {
                            self.write = true;
                        }
                    }
                    if let Expr::Call(n) = e {
                        if matches!(n.func.as_ref(),Expr::Name(n) if n.id.as_str()==self.symbol) {
                            self.visit_arguments(&n.arguments);
                            return;
                        }
                    }
                    visitor::walk_expr(self, e);
                }
            }
            let mut u = Uses {
                symbol: n,
                wrong: false,
                write: false,
            };
            u.visit_body(&parse(&stripped, module)?);
            if u.write {
                return Err(refusal("proof.support.definition_write", module, n));
            }
            if u.wrong {
                return Err(refusal("proof.support.definition_use", module, n));
            }
            namespace(&parse(module_source, module)?, module, &[])?;
        }
        let value = match s {
            Stmt::Assign(n) => Some(n.value.as_ref()),
            Stmt::AnnAssign(n) => n.value.as_deref(),
            _ => None,
        };
        if let Some(value) = value {
            let immutable = literal(value)
                .ok_or_else(|| refusal("proof.support.literal_initializer", module, symbol))?;
            if !(body.len() == 1
                && matches!(s,Stmt::Assign(n) if n.targets.len()==1 && matches!(&n.targets[0],Expr::Name(_))))
            {
                return Err(refusal("proof.support.literal_assignment", module, symbol));
            }
            let binding = if let Stmt::Assign(n) = s {
                if let Expr::Name(n) = &n.targets[0] {
                    n.id.as_str()
                } else {
                    unreachable!()
                }
            } else {
                unreachable!()
            };
            let indent = source.len() - source.trim_start_matches([' ', '\t']).len();
            let stripped =
                module_source.replacen(source, &format!("{}pass\n", " ".repeat(indent)), 1);
            if !immutable
                || stripped == module_source
                || binding_names(&stripped, module)?.contains(binding)
            {
                return Err(refusal("proof.support.literal_binding", module, binding));
            }
            namespace(&parse(module_source, module)?, module, &[])?;
        }
    }
    Ok(())
}
fn stdlib_shadow(tree: &Tree, observer: &str) -> R<()> {
    let mut configured = Set::new();
    for (name, file) in tree {
        if [
            "pyproject.toml",
            "pytest.toml",
            ".pytest.toml",
            "pytest.ini",
            ".pytest.ini",
            "tox.ini",
            "setup.cfg",
        ]
        .contains(&name.as_str())
        {
            let source = String::from_utf8_lossy(&file.bytes);
            let mut in_path = false;
            for line in source.lines() {
                if let Some((key, value)) = line.split_once('=') {
                    if key.trim() == "pythonpath" {
                        in_path = true;
                        configured.extend(
                            value
                                .split(['[', ']', ',', '\'', '"', ' '])
                                .filter(|p| !p.is_empty())
                                .map(str::to_owned),
                        );
                        continue;
                    }
                }
                if in_path && line.starts_with(char::is_whitespace) {
                    configured.extend(line.split_whitespace().map(str::to_owned));
                } else {
                    in_path = false;
                }
            }
        }
    }
    for name in tree.keys().filter(|n| n.ends_with(".py")) {
        let parts: Vec<_> = name.split('/').collect();
        for (depth, part) in parts.iter().enumerate() {
            let stem = if depth + 1 == parts.len() {
                part.trim_end_matches(".py")
            } else {
                part
            };
            let parent = parts[..depth].join("/");
            let root = depth == 0
                || configured.contains(&parent)
                || !tree.contains_key(&format!("{parent}/__init__.py"));
            if contains("pathlib unittest importlib", stem) && root {
                return Err(refusal("proof.loader.stdlib_shadow", observer, ""));
            }
        }
    }
    Ok(())
}
fn assemble(input: &Value, reduced: bool) -> R<(Value, Tree)> {
    let base = read_tree(&input["base"]);
    let head = read_tree(&input["head"]);
    if input["destination_exists"].as_bool() == Some(true) {
        return Err(refusal("proof.tree.destination_fresh", "", ""));
    }
    let mut staged = base.clone();
    let check = &input["check"];
    let config = &input["config"];
    let declaration = &check["proof"];
    let subjects = strings(&declaration["subjects"]);
    let support = strings(&declaration["observer_support"]);
    let tests = strings(&check["tests"]);
    let outcome = !matches!(
        check["kind"].as_str(),
        Some("demonstrating" | "reproducing")
    ) || !check["declared_symptom"].is_null()
        || (!check["forcing"].is_null() && subjects.is_empty());
    let mut observers = vec![];
    if !outcome {
        for test in &tests {
            let o = test.split('[').next().unwrap().to_string();
            if !observers.contains(&o) {
                observers.push(o);
            }
        }
    }
    for subject in &subjects {
        if support
            .iter()
            .chain(&observers)
            .any(|other| overlaps(subject, other))
        {
            return Err(refusal(
                "proof.subject.overlap",
                subject.split("::").next().unwrap(),
                subject,
            ));
        }
        if unit(&base, subject)? == unit(&head, subject)? {
            return Err(refusal(
                "proof.subject.changed",
                subject.split("::").next().unwrap(),
                subject,
            ));
        }
    }
    for dep in &support {
        if unit(&base, dep)?.is_some() {
            return Err(refusal(
                "proof.support.new",
                dep.split("::").next().unwrap(),
                dep,
            ));
        }
    }
    for (i, o) in observers.iter().enumerate() {
        if observers[i + 1..].iter().any(|p| overlaps(o, p)) {
            return Err(refusal(
                "proof.observer.overlap",
                o.split("::").next().unwrap(),
                o,
            ));
        }
    }
    let paths: Set = observers
        .iter()
        .chain(&support)
        .map(|s| s.split("::").next().unwrap().into())
        .collect();
    let mut identities: BTreeMap<String, String> = BTreeMap::new();
    let mut inferred_support = Set::new();
    let mut loader = vec![];
    for module in &paths {
        let has_observer = observers
            .iter()
            .any(|o| o.split("::").next() == Some(module));
        if has_observer
            && !strings(&config["test-files"]).iter().any(|p| {
                glob::Pattern::new(p).is_ok_and(|g| {
                    g.matches_with(
                        module,
                        glob::MatchOptions {
                            require_literal_separator: true,
                            ..Default::default()
                        },
                    )
                })
            })
        {
            return Err(refusal("proof.observer.test_path", module, ""));
        }
        let new = text(&head, module)
            .ok_or_else(|| refusal("proof.observer.module_missing", module, ""))?;
        let old = text(&base, module).unwrap_or_default();
        let exists = base.contains_key(module);
        let observer_context = !exists && has_observer;
        if observer_context {
            stdlib_shadow(&head, module)?;
            loader.push(module.clone());
        }
        let body = parse(&new, module)?;
        let requested: Vec<_> = observers
            .iter()
            .chain(&support)
            .filter(|s| s.split("::").next() == Some(module))
            .cloned()
            .collect();
        let context = [old.clone(), new.clone()];
        // R1 collects support violations; structural errors can gate before collected violations.
        let mut collected: Option<Refusal> = None;
        for statement in &body {
            if matches!(statement, Stmt::Import(_) | Stmt::ImportFrom(_)) {
                let n = names(std::slice::from_ref(statement));
                if requested
                    .iter()
                    .filter(|s| support.contains(s))
                    .any(|s| n.bound.contains(s.split("::").last().unwrap()))
                {
                    if let Err(e) = validate_support(
                        &norm(&stmt_text(&new, statement)),
                        module,
                        false,
                        &subjects,
                        &[],
                    ) {
                        collected.get_or_insert(e);
                    }
                }
            }
        }
        let mut support_required = Set::new();
        for item in requested.iter().filter(|s| support.contains(s)) {
            let src = match source_unit(&new, item, false) {
                Ok(Some((source,_))) => source,
                Ok(None) => unreachable!(),
                Err(error) => return Err(collected.take().unwrap_or(error)),
            };
            if !reduced {
                if let Err(e) = validate_support(
                    &src,
                    module,
                    observer_context && item == module,
                    &subjects,
                    &context,
                ) {
                    collected.get_or_insert(e);
                }
            }
            if exists && item.contains("::") {
                if let Err(e) = partial(&src, &new, module, item.split("::").last().unwrap()) {
                    collected.get_or_insert(e);
                }
            }
            if item != module {
                support_required.extend(global_names(&src, module)?);
            }
        }
        for statement in &body {
            if matches!(statement, Stmt::Import(_) | Stmt::ImportFrom(_))
                && !names(std::slice::from_ref(statement))
                    .bound
                    .is_disjoint(&support_required)
            {
                if let Err(e) = validate_support(
                    &norm(&stmt_text(&new, statement)),
                    module,
                    false,
                    &subjects,
                    &[],
                ) {
                    collected.get_or_insert(e);
                }
            }
        }
        if let Some(e) = collected {
            return Err(e);
        }
        if !exists {
            if subjects
                .iter()
                .any(|s| s.split("::").next() == Some(module))
            {
                return Err(refusal("proof.observer.subject_module", module, ""));
            }
            for item in &requested {
                identities.insert(
                    item.clone(),
                    hash(source_unit(&new, item, false)?.unwrap().0),
                );
            }
            staged.insert(
                module.clone(),
                File {
                    bytes: new.into_bytes(),
                    mode: 0o644,
                },
            );
            continue;
        }
        let mut replacements: Vec<(usize, usize, String)> = vec![];
        let mut grafts = vec![];
        let mut required_texts = vec![];
        let mut protected = written(&old, module, &[])?;
        for subject in subjects
            .iter()
            .filter(|s| s.split("::").next() == Some(module))
        {
            protected.insert(subject.split("::").skip(1).collect::<Vec<_>>().join("::"));
        }
        let guard = |text: &str, item: &str, namespace: &[String]| -> R<()> {
            if observers.iter().any(|o| o == item) {
                return Ok(());
            }
            if let Some(conflict) = written(text, module, namespace)?
                .intersection(&protected)
                .next()
            {
                return Err(refusal("proof.support.graft_binding", module, conflict));
            }
            Ok(())
        };
        for item in &requested {
            let mut src = source_unit(&new, item, false)?.unwrap().0;
            required_texts.push(src.clone());
            if observers.contains(item)
                && matches!(
                    parse(&dedent(&src), module)?.first(),
                    Some(Stmt::ClassDef(_))
                )
            {
                return Err(refusal(
                    "proof.observer.exact_method",
                    module,
                    item.split("::").last().unwrap(),
                ));
            }
            if !item.contains("::") {
                return Err(refusal("proof.support.existing_module", module, ""));
            }
            identities.insert(item.clone(), hash(&src));
            let span = source_unit(&old, item, true)?.and_then(|x| x.1);
            let parts: Vec<_> = item.split("::").collect();
            let mut namespace: Vec<String> = parts[1..parts.len() - 1]
                .iter()
                .map(|s| s.to_string())
                .collect();
            if span.is_none() {
                if parts.len() > 2 {
                    let parent = parts[..2].join("::");
                    if source_unit(&old, &parent, true)?.is_none() {
                        if subjects.iter().any(|s| overlaps(&parent, s)) {
                            return Err(refusal("proof.observer.subject_class", module, parts[1]));
                        }
                        src = source_unit(&new, &parent, false)?.unwrap().0;
                        required_texts.push(src.clone());
                        namespace.clear();
                    } else {
                        let end = source_unit(&old, &parent, false)?.unwrap().1.unwrap().1;
                        guard(&src, item, &namespace)?;
                        replacements.push((end, end, format!("\n{src}")));
                        continue;
                    }
                } else {
                    let b = binding_names(&old, module)?;
                    if b.contains(parts.last().unwrap().to_owned()) || b.contains("*") {
                        return Err(refusal(
                            "proof.observer.binding_shadow",
                            module,
                            parts.last().unwrap(),
                        ));
                    }
                }
            }
            if let Some((a, b)) = span {
                guard(&src, item, &namespace)?;
                replacements.push((a, b, src));
            } else if !grafts.contains(&src) {
                guard(&src, item, &namespace)?;
                grafts.push(src);
            }
        }
        let mut lines: Vec<String> = old.split_inclusive('\n').map(str::to_owned).collect();
        replacements.sort();
        for (start, end, mut src) in replacements.into_iter().rev() {
            if !src.ends_with('\n') {
                src.push('\n');
            }
            lines.splice(start..end, [src]);
        }
        let mut composed = lines.concat();
        for graft in grafts {
            composed += &format!("\n{graft}\n");
        }
        let mut required = Set::new();
        for s in &required_texts {
            required.extend(global_names(s, module)?);
        }
        // Module bindings exclude nested function locals.
        let mut bound = written(&composed, module, &[])?;
        bound.retain(|n| !n.contains("::"));
        let mut imports = vec![];
        for s in &body {
            let mut statement = s.clone();
            let aliases = match &mut statement {
                Stmt::Import(n) => Some((&mut n.names, true)),
                Stmt::ImportFrom(n) => Some((&mut n.names, false)),
                _ => None,
            };
            if let Some((aliases, plain)) = aliases {
                aliases.retain(|a| {
                    let n = a.asname.as_ref().map_or_else(
                        || {
                            if plain {
                                a.name.split('.').next().unwrap().to_owned()
                            } else {
                                a.name.to_string()
                            }
                        },
                        ToString::to_string,
                    );
                    required.contains(&n) && !bound.contains(&n)
                });
                if !aliases.is_empty() {
                    // Generate the filtered import instead of slicing its original span.
                    let normalized = match &statement {
                        Stmt::Import(n) => format!(
                            "import {}",
                            n.names
                                .iter()
                                .map(|a| format!(
                                    "{}{}",
                                    a.name,
                                    a.asname
                                        .as_ref()
                                        .map(|n| format!(" as {n}"))
                                        .unwrap_or_default()
                                ))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                        Stmt::ImportFrom(n) => format!(
                            "from {}{} import {}",
                            ".".repeat(n.level as usize),
                            n.module
                                .as_ref()
                                .map(ToString::to_string)
                                .unwrap_or_default(),
                            n.names
                                .iter()
                                .map(|a| format!(
                                    "{}{}",
                                    a.name,
                                    a.asname
                                        .as_ref()
                                        .map(|n| format!(" as {n}"))
                                        .unwrap_or_default()
                                ))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                        _ => unreachable!(),
                    };
                    bound.extend(binding_names(&normalized, module)?);
                    imports.push(normalized);
                }
            }
        }
        let unresolved: Vec<_> = required.difference(&bound).cloned().collect();
        for symbol in unresolved {
            let item = format!("{module}::{symbol}");
            let Some((src, _)) = source_unit(&new, &item, true)? else {
                continue;
            };
            let b = parse(&dedent(&src), module)?;
            if b.len() != 1 {
                continue;
            }
            let literal_shape = matches!(&b[0],Stmt::Assign(n) if n.targets.len()==1 && matches!(&n.targets[0],Expr::Name(n) if n.id.as_str()==symbol) && literal(&n.value).is_some());
            if !literal_shape {
                continue;
            }
            if !reduced {
                validate_support(&src, module, false, &subjects, &[])?;
            }
            guard(&src, &item, &[])?;
            if binding_names(&old, module)?.contains("*") {
                return Err(refusal("proof.support.literal_wildcard", module, &symbol));
            }
            partial(&src, &new, module, &symbol)?;
            imports.push(src.clone());
            bound.insert(symbol);
            inferred_support.insert(item.clone());
            identities.insert(item, hash(&src));
        }
        if let Some(missing) = required.difference(&bound).next() {
            return Err(refusal("proof.observer.unresolved_symbol", module, missing));
        }
        let mut insertion = 0;
        for s in parse(&composed, module)? {
            if matches!(&s,Stmt::Expr(n) if matches!(n.value.as_ref(),Expr::StringLiteral(_)))
                || matches!(&s,Stmt::ImportFrom(n) if n.module.as_ref().is_some_and(|n|n.as_str()=="__future__"))
            {
                insertion = line_span(&composed, &s).1;
            } else {
                break;
            }
        }
        let mut lines: Vec<_> = composed.split_inclusive('\n').map(str::to_owned).collect();
        lines.splice(
            insertion..insertion,
            imports.iter().map(|s| format!("{s}\n")),
        );
        staged.insert(
            module.clone(),
            File {
                bytes: lines.concat().into_bytes(),
                mode: base[module].mode,
            },
        );
    }
    if let Some(module) = loader.first() {
        stdlib_shadow(&staged, module)?;
    }
    for subject in &subjects {
        if unit(&staged, subject)? != unit(&base, subject)? {
            return Err(refusal(
                "proof.subject.preserved",
                subject.split("::").next().unwrap(),
                subject,
            ));
        }
    }
    let all: Set = base.keys().chain(head.keys()).cloned().collect();
    let inferred: Vec<_> = all
        .into_iter()
        .filter(|n| base.get(n).map(|f| &f.bytes) != head.get(n).map(|f| &f.bytes))
        .collect();
    let identity = |tree: &Tree, name: &str| -> Value {
        if !name.contains("::") {
            return tree
                .get(name)
                .map(|f| json!(hash(&f.bytes)))
                .unwrap_or(Value::Null);
        }
        unit(tree, name)
            .ok()
            .flatten()
            .map(|u| json!(hash(u)))
            .unwrap_or(Value::Null)
    };
    let mut subject_json = serde_json::Map::new();
    for subject in if subjects.is_empty() {
        &inferred
    } else {
        &subjects
    } {
        if subject.ends_with(".py") || subject.contains("::") {
            subject_json.insert(
                subject.clone(),
                json!({"base":identity(&base,subject),"head":identity(&head,subject)}),
            );
        }
    }
    let mut observer_json = serde_json::Map::new();
    for test in &tests {
        observer_json.insert(
            test.clone(),
            identity(&head, test.split('[').next().unwrap()),
        );
    }
    let mut support_json = serde_json::Map::new();
    for s in support.iter().cloned().chain(inferred_support) {
        support_json.insert(s.clone(), json!(identities.get(&s)));
    }
    let receipt = json!({"rule":"subject-observer-v1","declaration":declaration,"mode":if outcome{"pytest-outcome"}else{"head-observer"},"subjects":subject_json,"observer_sha256":observer_json,"support_sha256":support_json,"source_base_tree_sha256":tree_hash(&base),"base_tree_sha256":tree_hash(&staged),"head_tree_sha256":tree_hash(&head)});
    Ok((receipt, staged))
}
fn literal_facts(source: &str, body: &[Stmt]) -> BTreeMap<String, Option<bool>> {
    struct Scan<'a> {
        source: &'a str,
        values: BTreeMap<String, Option<bool>>,
    }
    impl<'a> Visitor<'a> for Scan<'_> {
        fn visit_stmt(&mut self, s: &'a Stmt) {
            let value = match s {
                Stmt::Assign(n) => Some(n.value.as_ref()),
                Stmt::AnnAssign(n) => n.value.as_deref(),
                _ => None,
            };
            if let Some(e) = value {
                let text = &self.source[usize::from(e.start())..usize::from(e.end())];
                self.values.insert(text.to_owned(), literal(e));
            }
            visitor::walk_stmt(self, s);
        }
    }
    let mut scan = Scan {
        source,
        values: BTreeMap::new(),
    };
    scan.visit_body(body);
    scan.values
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let reduced = args.iter().any(|a| a == "--reduced");
    let sources = args.iter().any(|a| a == "--sources");
    for line in io::stdin().lock().lines() {
        let line = line.unwrap();
        let row: Value = serde_json::from_str(&line).unwrap();
        if sources {
            let source = row["source"].as_str().unwrap();
            let start = Instant::now();
            let value = match parse_module(source) {
                Ok(m) => {
                    let generated = ruff_python_codegen::round_trip(source).unwrap();
                    let comparable = parse_module(&generated).is_ok_and(|n| {
                        m.suite()
                            .iter()
                            .map(ComparableStmt::from)
                            .eq(n.suite().iter().map(ComparableStmt::from))
                    });
                    json!({"sha256":row["sha256"],"accepted":true,"generated":generated.trim_end(),"comparable_roundtrip":comparable,
                    "binding_names":binding_names(source,"probe").ok(), "global_names":global_names(source,"probe").ok(),
                    "written_bindings":written(source,"probe",&[]).ok(), "literals":literal_facts(source,m.suite()),"rust_ns":start.elapsed().as_nanos()})
                }
                Err(e) => {
                    json!({"sha256":row["sha256"],"accepted":false,"error":e.to_string(),"rust_ns":start.elapsed().as_nanos()})
                }
            };
            println!("{value}");
            continue;
        }
        let started = Instant::now();
        let result = assemble(&row["input"], reduced);
        let ns = started.elapsed().as_nanos();
        let value = match result {
            Ok((receipt, staged)) => {
                let tree: serde_json::Map<String, Value> = staged
                    .iter()
                    .map(|(n, f)| {
                        (
                            n.clone(),
                            json!({"b64":STANDARD.encode(&f.bytes),"mode":f.mode}),
                        )
                    })
                    .collect();
                json!({"id":row["id"],"accepted":true,"receipt":receipt,"staged_sha256":tree_hash(&staged),"staged":tree,"rust_ns":ns})
            }
            Err(e) => {
                json!({"id":row["id"],"accepted":false,"rule":e.rule,"module":e.module,"symbol":e.symbol,"rust_ns":ns})
            }
        };
        println!("{value}");
        io::stdout().flush().unwrap();
    }
}
