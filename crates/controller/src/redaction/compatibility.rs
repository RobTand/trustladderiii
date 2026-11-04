//! Python regular-expression semantics for the controller adapter.

type Result<T> = std::result::Result<T, String>;

pub(crate) fn translate(pattern: &str, classes: &[String; 3]) -> Result<String> {
    fn shorthand(code: char, classes: &[String; 3], invert: bool) -> String {
        let index = match code {
            'w' | 'W' => 0,
            'd' | 'D' => 1,
            _ => 2,
        };
        let negate = if code.is_ascii_uppercase() ^ invert {
            "^"
        } else {
            ""
        };
        format!("(?-i:[{negate}{}])", classes[index])
    }
    fn bracket(
        source: &mut std::iter::Peekable<std::str::Chars<'_>>,
        classes: &[String; 3],
        ignore_case: bool,
    ) -> Result<String> {
        // Lex one complete class, then parse it. Never splice members into its source:
        // a trailing literal '-' changes meaning when followed by another character.
        let mut token = String::from("[");
        if source.next_if_eq(&'^').is_some() {
            token.push('^');
        }
        if source.next_if_eq(&']').is_some() {
            token.push(']');
        }
        loop {
            let c = source.next().ok_or("unfinished regex class")?;
            token.push(c);
            match c {
                '\\' => token.push(source.next().ok_or("unfinished class escape")?),
                ']' => break,
                _ => {}
            }
        }
        parsed_class(&token, classes, ignore_case)
    }
    #[derive(Clone, Copy, Default)]
    struct Flags {
        ignore_case: bool,
        multiline: bool,
    }
    let word = shorthand('w', classes, false);
    let mut out = String::new();
    let mut source = pattern.chars().peekable();
    let mut flags = Flags::default();
    let mut scopes = Vec::new();
    while let Some(c) = source.next() {
        match c {
            '[' => out.push_str(&bracket(&mut source, classes, flags.ignore_case)?),
            '(' => {
                out.push('(');
                if source.peek() == Some(&'?') {
                    let mut look = source.clone();
                    look.next();
                    let mut header = String::from("?");
                    while let Some(code) =
                        look.next_if(|c| matches!(c, 'i' | 'm' | 's' | 'x' | '-'))
                    {
                        header.push(code);
                    }
                    if header.len() > 1 && matches!(look.peek(), Some(':' | ')')) {
                        let end = look.next().ok_or("unfinished flags")?;
                        if end == ':' {
                            scopes.push(flags);
                        }
                        let mut enabled = true;
                        for code in header.chars().skip(1) {
                            match code {
                                '-' => enabled = false,
                                'i' => flags.ignore_case = enabled,
                                'm' => flags.multiline = enabled,
                                _ => {}
                            }
                        }
                        out.push_str(&header);
                        out.push(end);
                        source = look;
                        continue;
                    }
                    // Group names and named backreferences are syntax, never literals.
                    let mut look = source.clone();
                    if look.next() == Some('?') && look.next() == Some('P') {
                        let delimiter = match look.next() {
                            Some('<') => '>',
                            Some('=') => ')',
                            _ => return Err("invalid named group".into()),
                        };
                        let mut header = String::from("?P");
                        header.push(if delimiter == '>' { '<' } else { '=' });
                        loop {
                            let code = look.next().ok_or("unfinished named group")?;
                            header.push(code);
                            if code == delimiter {
                                break;
                            }
                        }
                        out.push_str(&header);
                        source = look;
                        if delimiter == ')' {
                            continue;
                        }
                    }
                }
                scopes.push(flags);
            }
            ')' => {
                flags = scopes.pop().ok_or("unbalanced regex group")?;
                out.push(')');
            }
            '$' => out.push_str(if flags.multiline {
                r"(?=\n|\z)"
            } else {
                r"(?=\n?\z)"
            }),
            'i' | 'I' | 'İ' | 'ı' if flags.ignore_case => out.push_str("(?-i:[Iiİı])"),
            '\\' => {
                let escape = source.next().ok_or("unfinished regex escape")?;
                match escape {
                    'w' | 'W' | 'd' | 'D' | 's' | 'S' => {
                        out.push_str(&shorthand(escape, classes, false))
                    }
                    'b' => {
                        out.push_str(&format!(r"(?:(?<!{word})(?={word})|(?<={word})(?!{word}))"))
                    }
                    'B' => {
                        out.push_str(&format!(r"(?:(?<!{word})(?!{word})|(?<={word})(?={word}))"))
                    }
                    'Z' => out.push_str(r"\z"),
                    other => {
                        out.push('\\');
                        out.push(other);
                    }
                }
            }
            other => out.push(other),
        }
    }
    if !scopes.is_empty() {
        return Err("unbalanced regex group".into());
    }
    Ok(out)
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
                let Ast::ClassBracketed(class) = ast else {
                    return Err("invalid frozen Python character class".into());
                };
                let mut set = bracketed(&class, classes, false)?;
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
    let Ast::ClassBracketed(class) = ast else {
        return Err("expected a parsed bracketed Python class".into());
    };
    let set = bracketed(&class, classes, ignore_case)?;
    Ok(format!("(?-i:{})", Hir::class(Class::Unicode(set))))
}
