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
        let negate = source.next_if_eq(&'^').is_some();
        let mut literal = String::new();
        let mut codes = Vec::new();
        let mut members = Vec::new();
        loop {
            match source.next().ok_or("unfinished regex class")? {
                ']' => break,
                '\\' => match source.next().ok_or("unfinished class escape")? {
                    code @ ('w' | 'W' | 'd' | 'D' | 's' | 'S') => codes.push(code),
                    'b' => literal.push_str(r"\x08"),
                    code => {
                        literal.push('\\');
                        literal.push(code);
                    }
                },
                '[' => literal.push_str(r"\["),
                c => {
                    literal.push(c);
                    members.push(c);
                }
            }
        }
        // Python adds dotted/dotless I to ASCII I/i under Unicode IGNORECASE.
        // Apply the union before negation, including ranges such as [a-z].
        if ignore_case
            && (members.iter().any(|c| matches!(c, 'I' | 'i' | 'İ' | 'ı'))
                || members.windows(3).any(|r| {
                    r[1] == '-'
                        && ['I', 'i', 'İ', 'ı']
                            .iter()
                            .any(|c| r[0] <= *c && *c <= r[2])
                }))
        {
            literal.push_str("Iiİı");
        }
        if codes.is_empty() {
            return Ok(format!("[{}{literal}]", if negate { "^" } else { "" }));
        }
        if literal.is_empty() && codes.len() == 1 {
            return Ok(shorthand(codes[0], classes, negate));
        }
        let mut parts = codes
            .into_iter()
            .map(|c| shorthand(c, classes, false))
            .collect::<Vec<_>>();
        if !literal.is_empty() {
            parts.push(format!("[{literal}]"));
        }
        let union = parts.join("|");
        Ok(if negate {
            format!("(?:(?!(?:{union}))(?s:.))")
        } else {
            format!("(?:{union})")
        })
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
