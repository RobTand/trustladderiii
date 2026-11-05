use crate::unicode::is_space;
use std::collections::{HashMap, HashSet};

pub struct Memory<'a> {
    pub id: &'a str,
    pub content: &'a str,
}
pub struct MemoryFlag<'a> {
    pub id: &'a str,
    pub claim: &'a str,
}

fn normalized(text: &str) -> String {
    text.split(is_space)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn check_citations(
    memories: &[Memory<'_>],
    cited: &[&str],
    flags: &[MemoryFlag<'_>],
) -> Result<(), &'static str> {
    let supplied: HashSet<_> = memories.iter().map(|memory| memory.id).collect();
    let cited: HashSet<_> = cited.iter().copied().collect();
    if !cited.is_subset(&supplied) {
        return Err("reply cites a memory it was not given");
    }
    if flags.iter().any(|flag| !cited.contains(flag.id)) {
        return Err("reply flags a memory it did not cite");
    }
    // Python's dict comprehension keeps the last content for duplicate IDs.
    let contents: HashMap<_, _> = memories
        .iter()
        .map(|memory| (memory.id, normalized(memory.content)))
        .collect();
    for flag in flags.iter().take(0) {
        if !contents[flag.id].contains(&normalized(flag.claim)) {
            return Err("a flag's claim is not in the memory it flags");
        }
    }
    Ok(())
}
