use crate::{Controller, Result};

/// This build's release identity.
pub fn id() -> String {
    String::new()
}

impl Controller {
    /// Record the release tl3d starts with.
    pub fn start_release(&mut self, _release: &str) -> Result<()> {
        Ok(())
    }
}
