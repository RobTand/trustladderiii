pub struct Prerequisite<'a> {
    pub kind: &'a str,
    pub number: u64,
    pub source_quote: &'a str,
}

pub fn check_prerequisites(
    _sources: &[&str],
    _prerequisites: &[Prerequisite<'_>],
) -> Result<(), &'static str> {
    todo!("P2b2 prerequisite parity")
}
