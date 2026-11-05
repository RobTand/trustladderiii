use crate::unicode::is_decimal;
use std::collections::HashSet;

pub struct Prerequisite<'a> {
    pub kind: &'a str,
    pub number: u64,
    pub source_quote: &'a str,
}

pub fn check_prerequisites(
    sources: &[&str],
    prerequisites: &[Prerequisite<'_>],
) -> Result<(), &'static str> {
    let mut seen = HashSet::new();
    for prerequisite in prerequisites {
        let quote = prerequisite.source_quote;
        let marker = format!("#{}", prerequisite.number);
        if !seen.insert((prerequisite.kind, prerequisite.number))
            || !sources.iter().any(|source| source.contains(quote))
            || !quote.match_indices(&marker).any(|(start, _)| {
                quote[start + marker.len()..]
                    .chars()
                    .next()
                    .is_none_or(|c| !is_decimal(c))
            })
        {
            return Err(
                "Each merge prerequisite needs its number and a verbatim quote from the issue",
            );
        }
    }
    Ok(())
}
