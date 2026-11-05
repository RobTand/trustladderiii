//! Selected-worker-release prompt rendering (#819).

pub fn render(_kind: &str, _document: &str) -> crate::Result<String> {
    Err(crate::Error::Invalid("prompt renderer stub"))
}

#[cfg(test)]
mod tests;
