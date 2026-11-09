//! #788: lexical Python subset checked before fancy-regex can normalize source.
use std::{iter::Peekable, str::CharIndices};

type Tokens<'a> = Peekable<CharIndices<'a>>;
type Result<T> = std::result::Result<T, String>;

fn unsupported(offset: usize, construct: impl std::fmt::Display) -> String {
    format!("unsupported Python regex grammar at byte {offset}: {construct}")
}

pub(super) fn validate(pattern: &str) -> Result<()> {
    // Require escaped controls. In particular, Python and fancy-regex disagree
    // about literal VT/FF in verbose mode; rejecting them must precede parsing.
    for (offset, c) in pattern.char_indices() {
        if matches!(c, '\u{b}' | '\u{c}') {
            return Err(unsupported(offset, format!("literal U+{:04X}", c as u32)));
        }
    }
    let mut tokens = pattern.char_indices().peekable();
    while let Some((offset, c)) = tokens.next() {
        match c {
            '\\' => escape(&mut tokens, offset, false)?,
            '[' => class(&mut tokens, offset)?,
            '(' => group(&mut tokens, offset)?,
            _ => {}
        }
    }
    Ok(())
}

fn escape(tokens: &mut Tokens<'_>, offset: usize, in_class: bool) -> Result<()> {
    let (_, code) = tokens
        .next()
        .ok_or_else(|| unsupported(offset, "unfinished escape"))?;
    match code {
        'a' | 'b' | 'f' | 'n' | 'r' | 't' | 'v' | 'd' | 'D' | 's' | 'S' | 'w' | 'W' => Ok(()),
        'A' | 'B' | 'Z' if !in_class => Ok(()),
        'x' | 'u' | 'U' => {
            let width = match code {
                'x' => 2,
                'u' => 4,
                _ => 8,
            };
            let mut scalar = 0;
            for _ in 0..width {
                let digit = tokens
                    .next()
                    .and_then(|(_, c)| c.to_digit(16))
                    .ok_or_else(|| {
                        unsupported(
                            offset,
                            format!("hex escape \\{code} requires {width} hexadecimal digits"),
                        )
                    })?;
                scalar = scalar * 16 + digit;
            }
            if char::from_u32(scalar).is_none() {
                return Err(unsupported(
                    offset,
                    format!("non-scalar hex escape \\{code}"),
                ));
            }
            Ok(())
        }
        'N' if tokens.peek().is_some_and(|(_, c)| *c == '{') => {
            Err(unsupported(offset, "named Unicode escape \\N{...}"))
        }
        '0'..='9' => Err(unsupported(
            offset,
            "numeric escape/backreference; use a named group",
        )),
        // fancy-regex assigns assertion semantics to escaped < and >. Python
        // treats them as literals, so leave these outside the supported subset.
        c if (c.is_ascii_punctuation() && !matches!(c, '<' | '>')) || c == ' ' => Ok(()),
        _ => Err(unsupported(offset, format!("escape \\{code}"))),
    }
}

fn class(tokens: &mut Tokens<'_>, offset: usize) -> Result<()> {
    if tokens.peek().is_some_and(|(_, c)| *c == '^') {
        tokens.next();
    }
    if tokens.peek().is_some_and(|(_, c)| *c == ']') {
        tokens.next();
    }
    while let Some((at, c)) = tokens.next() {
        match c {
            '\\' => escape(tokens, at, true)?,
            ']' => return Ok(()),
            '[' => return Err(unsupported(at, "nested character class; escape literal [")),
            _ => {}
        }
    }
    Err(unsupported(offset, "unfinished character class"))
}

fn name(tokens: &mut Tokens<'_>, offset: usize, delimiter: char) -> Result<()> {
    let mut first = true;
    for (_, c) in tokens.by_ref() {
        if c == delimiter && !first {
            return Ok(());
        }
        if !(c == '_' || c.is_ascii_alphabetic() || (!first && c.is_ascii_digit())) {
            return Err(unsupported(
                offset,
                "capture name must be an ASCII identifier",
            ));
        }
        first = false;
    }
    Err(unsupported(offset, "unfinished capture name"))
}

fn group(tokens: &mut Tokens<'_>, offset: usize) -> Result<()> {
    if tokens.peek().is_some_and(|(_, c)| *c == '*') {
        return Err(unsupported(offset, "backtracking control group"));
    }
    if !tokens.peek().is_some_and(|(_, c)| *c == '?') {
        return Ok(());
    }
    tokens.next();
    match tokens.peek().map(|(_, c)| *c) {
        Some(':' | '=' | '!' | '>') => {
            tokens.next();
            Ok(())
        }
        Some('<') => {
            tokens.next();
            if matches!(tokens.next(), Some((_, '=' | '!'))) {
                Ok(())
            } else {
                Err(unsupported(
                    offset,
                    "group prefix (?<; use (?P<name> for captures",
                ))
            }
        }
        Some('P') => {
            tokens.next();
            match tokens.next() {
                Some((_, '<')) => name(tokens, offset, '>'),
                Some((_, '=')) => name(tokens, offset, ')'),
                _ => Err(unsupported(offset, "group prefix (?P")),
            }
        }
        Some(c) if c.is_ascii_alphabetic() || c == '-' => flags(tokens, offset),
        _ => Err(unsupported(offset, "group extension")),
    }
}

fn flags(tokens: &mut Tokens<'_>, offset: usize) -> Result<()> {
    let (mut on, mut off) = (0_u8, 0_u8);
    let mut removing = false;
    for (_, c) in tokens.by_ref() {
        match c {
            'i' | 'm' | 's' => {
                let bit = match c {
                    'i' => 1,
                    'm' => 2,
                    _ => 4,
                };
                if removing {
                    off |= bit;
                } else {
                    on |= bit;
                }
                if on & off != 0 {
                    return Err(unsupported(offset, "flag enabled and disabled together"));
                }
            }
            '-' if !removing => {
                removing = true;
            }
            ':' | ')' => {
                if (on | off) == 0 || (removing && off == 0) || (c == ')' && removing) {
                    return Err(unsupported(
                        offset,
                        "malformed flag group; remove flags only in a scope",
                    ));
                }
                return Ok(());
            }
            // Verbose mode is deliberately excluded: it changes lexical rules
            // at several parser boundaries. No canonical policy rule uses it.
            _ => {
                return Err(unsupported(
                    offset,
                    format!("flag {c}; supported flags are i, m, s"),
                ));
            }
        }
    }
    Err(unsupported(offset, "unfinished flag group"))
}
