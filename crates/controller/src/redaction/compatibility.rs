//! Parse Python policy regexes before translating their semantics to fancy-regex.
use fancy_regex::{Assertion, Expr, LookAround};
use std::collections::HashMap;

type Result<T> = std::result::Result<T, String>;

pub(crate) fn translate(pattern: &str, classes: &[String; 3]) -> Result<String> {
    // fancy-regex merges $ and \z into one AST assertion. The policy uses Python's
    // \Z for strict end; reject ambiguous input rather than silently soften it.
    let mut tokens = pattern.chars();
    while let Some(c) = tokens.next() {
        if c == '\\' && tokens.next() == Some('z') {
            return Err("use Python \\Z for strict end of string".into());
        }
    }
    let tree = Expr::parse_tree(pattern).map_err(|e| e.to_string())?;
    let names = tree
        .named_groups
        .iter()
        .map(|(name, index)| (*index, name.as_str()))
        .collect();
    let mut writer = Writer {
        classes,
        names,
        next_group: 1,
    };
    writer.expression(&tree.expr)
}

struct Writer<'a> {
    classes: &'a [String; 3],
    names: HashMap<usize, &'a str>,
    next_group: usize,
}

impl Writer<'_> {
    fn expression(&mut self, expr: &Expr) -> Result<String> {
        Ok(match expr {
            Expr::Empty => String::new(),
            Expr::Literal { val, casei } => {
                let mut out = String::new();
                for c in val.chars() {
                    let mut set = ClassUnicode::new([ClassUnicodeRange::new(c, c)]);
                    if *casei {
                        fold(&mut set)?;
                    }
                    out.push_str(&Hir::class(Class::Unicode(set)).to_string());
                }
                out
            }
            Expr::Delegate { inner, casei } => parsed_class(inner, self.classes, *casei)?,
            Expr::Any {
                newline,
                crlf: false,
            } => if *newline { "(?s:.)" } else { "(?-s:.)" }.into(),
            Expr::Concat(parts) => {
                let mut out = String::new();
                for part in parts {
                    out.push_str(&self.expression(part)?);
                }
                out
            }
            Expr::Alt(parts) => {
                let arms = parts
                    .iter()
                    .map(|p| self.expression(p))
                    .collect::<Result<Vec<_>>>()?;
                format!("(?:{})", arms.join("|"))
            }
            Expr::Group(child) => {
                let index = self.next_group;
                self.next_group += 1;
                let body = self.expression(child)?;
                if let Some(name) = self.names.get(&index) {
                    format!("(?P<{name}>{body})")
                } else {
                    format!("({body})")
                }
            }
            Expr::LookAround(child, kind) => {
                let prefix = match kind {
                    LookAround::LookAhead => "?=",
                    LookAround::LookAheadNeg => "?!",
                    LookAround::LookBehind => "?<=",
                    LookAround::LookBehindNeg => "?<!",
                };
                format!("({prefix}{})", self.expression(child)?)
            }
            Expr::Repeat {
                child,
                lo,
                hi,
                greedy,
            } => {
                let upper = if *hi == usize::MAX {
                    String::new()
                } else {
                    hi.to_string()
                };
                format!(
                    "(?:{}){{{lo},{upper}}}{}",
                    self.expression(child)?,
                    if *greedy { "" } else { "?" }
                )
            }
            Expr::AtomicGroup(child) => format!("(?>{})", self.expression(child)?),
            Expr::Backref {
                group,
                casei: false,
            } => {
                if let Some(name) = self.names.get(group) {
                    format!("(?P={name})")
                } else {
                    format!(r"(?:\{group})")
                }
            }
            Expr::Assertion(assertion) => match assertion {
                Assertion::StartText => r"\A".into(),
                Assertion::EndText => r"(?=\n?\z)".into(),
                Assertion::EndTextIgnoreTrailingNewlines { crlf: false } => r"\z".into(),
                Assertion::StartLine { crlf: false } => r"(?m:^)".into(),
                Assertion::EndLine { crlf: false } => r"(?=\n|\z)".into(),
                Assertion::WordBoundary | Assertion::NotWordBoundary => {
                    let word = parsed_class(r"\w", self.classes, false)?;
                    if matches!(assertion, Assertion::WordBoundary) {
                        format!(r"(?:(?<!{word})(?={word})|(?<={word})(?!{word}))")
                    } else {
                        format!(r"(?:(?<!{word})(?!{word})|(?<={word})(?={word}))")
                    }
                }
                _ => return Err(format!("unsupported Python assertion AST: {assertion:?}")),
            },
            _ => return Err(format!("unsupported Python regex AST: {expr:?}")),
        })
    }
}

use regex_syntax::{
    ast::{self, Ast, ClassSet, ClassSetItem},
    hir::{Class, ClassUnicode, ClassUnicodeRange, Hir},
};

fn fold(set: &mut ClassUnicode) -> Result<()> {
    set.try_case_fold_simple().map_err(|e| e.to_string())?;
    let turkish = ['I', 'i', 'İ', 'ı'];
    if set
        .iter()
        .any(|r| turkish.iter().any(|c| r.start() <= *c && *c <= r.end()))
    {
        set.union(&ClassUnicode::new(
            turkish.map(|c| ClassUnicodeRange::new(c, c)),
        ));
    }
    Ok(())
}

fn parsed_class(pattern: &str, classes: &[String; 3], ignore_case: bool) -> Result<String> {
    fn item(node: &ClassSetItem, classes: &[String; 3], casei: bool) -> Result<ClassUnicode> {
        let mut set = match node {
            ClassSetItem::Literal(literal) => {
                ClassUnicode::new([ClassUnicodeRange::new(literal.c, literal.c)])
            }
            ClassSetItem::Range(range) => {
                ClassUnicode::new([ClassUnicodeRange::new(range.start.c, range.end.c)])
            }
            ClassSetItem::Union(union) => {
                let mut set = ClassUnicode::empty();
                for child in &union.items {
                    set.union(&item(child, classes, casei)?);
                }
                return Ok(set);
            }
            ClassSetItem::Perl(perl) => {
                let index = match perl.kind {
                    ast::ClassPerlKind::Word => 0,
                    ast::ClassPerlKind::Digit => 1,
                    ast::ClassPerlKind::Space => 2,
                };
                let ast = ast::parse::Parser::new()
                    .parse(&format!("[{}]", classes[index]))
                    .map_err(|e| e.to_string())?;
                let Ast::ClassBracketed(class) = &ast else {
                    return Err("invalid frozen Python character class".into());
                };
                let mut set = bracketed(class, classes, false)?;
                if perl.negated {
                    set.negate();
                }
                // Python shorthand sets do not change under IGNORECASE.
                return Ok(set);
            }
            _ => return Err(format!("unsupported Python character-class AST: {node:?}")),
        };
        if casei {
            fold(&mut set)?;
        }
        Ok(set)
    }
    fn bracketed(
        class: &ast::ClassBracketed,
        classes: &[String; 3],
        casei: bool,
    ) -> Result<ClassUnicode> {
        let ClassSet::Item(contents) = &class.kind else {
            return Err("Python class set operations are unsupported".into());
        };
        let mut set = item(contents, classes, casei)?;
        // Folding must precede complementing a negated class.
        if class.negated {
            set.negate();
        }
        Ok(set)
    }
    let ast = ast::parse::Parser::new()
        .parse(pattern)
        .map_err(|e| e.to_string())?;
    let set = match &ast {
        Ast::ClassBracketed(class) => bracketed(class, classes, ignore_case)?,
        Ast::ClassPerl(perl) => item(&ClassSetItem::Perl((**perl).clone()), classes, ignore_case)?,
        _ => return Err("expected a parsed Python character class".into()),
    };
    Ok(format!("(?-i:{})", Hir::class(Class::Unicode(set))))
}
