use alloc::vec::Vec;

use crate::FileId;

/// A set of controller-interned file IDs, with no file-count bound.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Footprint {
    low: u64,
    high: Vec<FileId>,
}

impl Footprint {
    pub fn new(files: impl IntoIterator<Item = FileId>) -> Self {
        let mut set = Self::default();
        for file in files {
            if file.0 < 64 {
                set.low |= 1 << file.0;
            } else {
                set.high.push(file);
            }
        }
        set.high.sort_unstable();
        set.high.dedup();
        set
    }

    /// File IDs in ascending order, with no duplicates.
    pub fn iter(&self) -> impl Iterator<Item = FileId> + '_ {
        (0..64)
            .filter(|i| self.low & (1 << i) != 0)
            .map(FileId)
            .chain(self.high.iter().copied())
    }

    pub fn len(&self) -> usize {
        self.low.count_ones() as usize + self.high.len()
    }

    pub fn is_empty(&self) -> bool {
        self.low == 0 && self.high.is_empty()
    }

    pub fn is_disjoint(&self, other: &Self) -> bool {
        if self.low & other.low != 0 {
            return false;
        }
        for file in &self.high {
            if other.high.contains(file) {
                return false;
            }
        }
        true
    }

    pub(crate) fn extend(&mut self, other: Self) {
        self.low |= other.low;
        self.high.reserve(other.high.len());
        for file in other.high {
            match self.high.binary_search(&file) {
                Ok(_) => {}
                Err(index) => self.high.insert(index, file),
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
        // The proof worlds have four densely interned files per repo. Higher
        // IDs use the same production type and are exercised by scenarios.
        let low = kani::any();
        kani::assume(low < 16);
        Self {
            low,
            high: Vec::new(),
        }
    }
}
