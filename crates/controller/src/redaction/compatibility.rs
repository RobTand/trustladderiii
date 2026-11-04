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
    ) -> Result<String> {
        let negate = source.next_if_eq(&'^').is_some();
        let mut literal = String::new();
        let mut codes = Vec::new();
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
                c => literal.push(c),
            }
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
    let word = shorthand('w', classes, false);
    let mut out = String::new();
    let mut source = pattern.chars().peekable();
    while let Some(c) = source.next() {
        if c == '[' {
            out.push_str(&bracket(&mut source, classes)?);
            continue;
        }
        if c != '\\' {
            out.push(c);
            continue;
        }
        let escape = source.next().ok_or("unfinished regex escape")?;
        match escape {
            'w' | 'W' | 'd' | 'D' | 's' | 'S' => out.push_str(&shorthand(escape, classes, false)),
            'b' => out.push_str(&format!(r"(?:(?<!{word})(?={word})|(?<={word})(?!{word}))")),
            'B' => out.push_str(&format!(r"(?:(?<!{word})(?!{word})|(?<={word})(?={word}))")),
            other => {
                out.push('\\');
                out.push(other);
            }
        }
    }
    Ok(out)
}
