pub struct Memory<'a> {
    pub id: &'a str,
    pub content: &'a str,
}

pub struct MemoryFlag<'a> {
    pub id: &'a str,
    pub claim: &'a str,
}

pub fn check_citations(
    _memories: &[Memory<'_>],
    _cited: &[&str],
    _flags: &[MemoryFlag<'_>],
) -> Result<(), &'static str> {
    todo!("P2b2 citation parity")
}
