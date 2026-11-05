use super::{IO, json, sample, write};
use std::io::{Error, ErrorKind, Result, Write};
#[derive(Clone)]
pub struct VPath(pub std::path::PathBuf);
impl VPath {
    pub fn join(&self, name: impl AsRef<std::path::Path>) -> Self {
        Self(self.0.join(name))
    }
    pub fn with_extension(&self, extension: &str) -> Self {
        Self(self.0.with_extension(extension))
    }
    pub fn exists(&self) -> bool {
        sample(json!(["exists", self.0])).is_multiple_of(2)
    }
}
pub struct File(VPath);
impl File {
    pub fn create(path: &VPath) -> Result<Self> {
        write(
            json!(["create", path.0]),
            path.0.to_string_lossy().into(),
            Some(vec![]),
        )?;
        Ok(Self(path.clone()))
    }
    pub fn open(path: &VPath) -> Result<Self> {
        sample(json!(["open", path.0]));
        Ok(Self(path.clone()))
    }
    pub fn sync_all(&self) -> Result<()> {
        write(json!(["fsync", self.0.0]), String::new(), None)
    }
}
impl Write for File {
    fn write(&mut self, bytes: &[u8]) -> Result<usize> {
        write(
            json!(["write", self.0.0, bytes]),
            self.0.0.to_string_lossy().into(),
            Some(bytes.to_vec()),
        )?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}
pub fn read(path: impl std::borrow::Borrow<VPath>) -> Result<Vec<u8>> {
    let path = path.borrow();
    let value = sample(json!(["read", path.0]));
    if path.0.ends_with("epoch.fence") {
        return match value % 4 {
            0 => Err(ErrorKind::NotFound.into()),
            1 => Ok(b"0".to_vec()),
            2 => Ok(b"1".to_vec()),
            _ => Err(ErrorKind::PermissionDenied.into()),
        };
    }
    match value % 3 {
        0 => Err(ErrorKind::NotFound.into()),
        1 => Ok(b"invalid json".to_vec()),
        _ => IO
            .with(|io| io.borrow().files.get(path.0.to_str().unwrap()).cloned())
            .ok_or_else(|| Error::from(ErrorKind::NotFound)),
    }
}
pub fn rename(from: VPath, to: &VPath) -> Result<()> {
    let bytes = IO
        .with(|io| io.borrow().files.get(from.0.to_str().unwrap()).cloned())
        .unwrap();
    let result = write(
        json!(["rename", from.0, to.0]),
        to.0.to_string_lossy().into(),
        Some(bytes),
    );
    // A successful or landed rename removes the temporary entry atomically.
    let landed = IO.with(|io| {
        let io = io.borrow();
        result.is_ok() || io.crash == Some((io.writes - 1, true))
    });
    if landed {
        IO.with(|io| {
            io.borrow_mut().files.remove(from.0.to_str().unwrap());
        });
    }
    result
}
