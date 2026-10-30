use alloc::vec::Vec;

use crate::{FileId, World};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Repr {
    Inline(u64),
    Interned(usize),
}

/// A world-scoped immutable file set. Create it with World::intern_footprint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Footprint(Repr);

impl Default for Footprint {
    fn default() -> Self {
        Self(Repr::Inline(0))
    }
}

impl Footprint {
    pub fn is_empty(self) -> bool {
        self == Self::default()
    }

    pub(crate) fn inline(bits: u64) -> Self {
        Self(Repr::Inline(bits))
    }
}

impl World {
    /// Intern a sorted, deduplicated set. Handles stay valid as the world grows.
    pub fn intern_footprint(&mut self, files: impl IntoIterator<Item = FileId>) -> Footprint {
        let mut files: Vec<_> = files.into_iter().collect();
        files.sort_unstable();
        files.dedup();
        self.intern_sorted_files(files)
    }

    fn intern_sorted_files(&mut self, files: Vec<FileId>) -> Footprint {
        if files.last().is_none_or(|f| f.0 < 64) {
            let mut bits = 0;
            for f in files {
                bits |= 1 << f.0;
            }
            return Footprint::inline(bits);
        }
        if let Some(index) = self.file_sets.iter().position(|set| *set == files) {
            return Footprint(Repr::Interned(index));
        }
        let index = self.file_sets.len();
        self.file_sets.push(files);
        Footprint(Repr::Interned(index))
    }

    /// Files in ascending order. An interned handle belongs to this world's table.
    pub fn files(&self, footprint: Footprint) -> Option<impl Iterator<Item = FileId> + '_> {
        let (bits, files) = match footprint.0 {
            Repr::Inline(bits) => (bits, &[][..]),
            Repr::Interned(index) => (0, self.file_sets.get(index)?.as_slice()),
        };
        Some(
            (0..64)
                .filter(move |i| bits & (1 << i) != 0)
                .map(FileId)
                .chain(files.iter().copied()),
        )
    }

    /// Persist these sets in order. Restore them with intern_footprint before loading nodes.
    pub fn file_sets(&self) -> &[Vec<FileId>] {
        &self.file_sets
    }

    pub(crate) fn has_footprint(&self, footprint: Footprint) -> bool {
        match footprint.0 {
            Repr::Inline(_) => true,
            Repr::Interned(index) => index < self.file_sets.len(),
        }
    }

    pub(crate) fn footprints_disjoint(&self, a: Footprint, b: Footprint) -> bool {
        if let (Repr::Inline(a), Repr::Inline(b)) = (a.0, b.0) {
            return a & b == 0;
        }
        #[cfg(kani)]
        {
            // Assert before pruning: the four-file proof fixture must stay inline.
            assert!(false, "interned footprints are outside the bounded proof fixture");
            kani::assume(false);
        }

        for file in self.files(a).expect("interned footprint") {
            let contains = match b.0 {
                Repr::Inline(bits) => file.0 < 64 && bits & (1 << file.0) != 0,
                Repr::Interned(index) => self.file_sets[index].binary_search(&file).is_ok(),
            };
            if contains {
                return false;
            }
        }
        true
    }

    pub(crate) fn union_footprints(&mut self, a: Footprint, b: Footprint) -> Footprint {
        if let (Repr::Inline(a), Repr::Inline(b)) = (a.0, b.0) {
            return Footprint::inline(a | b);
        }
        #[cfg(kani)]
        {
            // Assert before pruning: the four-file proof fixture must stay inline.
            assert!(false, "interned footprints are outside the bounded proof fixture");
            kani::assume(false);
        }

        let files = {
            let mut left = self.files(a).expect("interned footprint").peekable();
            let mut right = self.files(b).expect("interned footprint").peekable();
            let mut files = Vec::new();
            loop {
                let file = match (left.peek().copied(), right.peek().copied()) {
                    (Some(a), Some(b)) if a == b => {
                        left.next();
                        right.next()
                    }
                    (Some(a), Some(b)) if a < b => left.next(),
                    (Some(_), Some(_)) => right.next(),
                    (Some(_), None) => left.next(),
                    (None, Some(_)) => right.next(),
                    (None, None) => None,
                };
                let Some(file) = file else { break };
                files.push(file);
            }
            files
        };
        self.intern_sorted_files(files)
    }
}

#[cfg(kani)]
impl kani::Arbitrary for Footprint {
    fn any() -> Self {
        let bits = kani::any();
        kani::assume(bits < 16);
        Self::inline(bits)
    }
}
