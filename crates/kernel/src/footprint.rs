use alloc::vec::Vec;

use crate::FileId;

/// A sorted set of controller-interned file IDs. There is no file-count bound.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Footprint {
    files: Vec<FileId>,
}

impl Footprint {
    pub fn new(files: impl IntoIterator<Item = FileId>) -> Self {
        let mut files: Vec<_> = files.into_iter().collect();
        files.sort_unstable();
        files.dedup();
        Self { files }
    }

    pub fn files(&self) -> &[FileId] {
        &self.files
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn is_disjoint(&self, other: &Self) -> bool {
        for file in &self.files {
            if other.files.contains(file) {
                return false;
            }
        }
        true
    }

    pub(crate) fn extend(&mut self, other: Self) {
        self.files.reserve(other.files.len());
        for file in other.files {
            match self.files.binary_search(&file) {
                Ok(_) => {}
                Err(index) => self.files.insert(index, file),
            }
        }
    }
}

impl FromIterator<FileId> for Footprint {
    fn from_iter<T: IntoIterator<Item = FileId>>(files: T) -> Self {
        Self::new(files)
    }
}

#[cfg(kani)]
impl kani::Arbitrary for Footprint {
    fn any() -> Self {
        let files = kani::vec::any_vec::<FileId, 4>();
        for file in &files {
            kani::assume(file.0 < 4);
        }
        for pair in files.windows(2) {
            kani::assume(pair[0] < pair[1]);
        }
        Self { files }
    }
}
