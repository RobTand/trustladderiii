use alloc::vec::Vec;

use crate::{FileId, World};
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

#[cfg_attr(
    all(feature = "serde", not(kani)),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Repr {
    Inline(u64),
    Interned(usize),
}

/// A world-scoped immutable file set. Create it with World::intern_footprint.
#[cfg_attr(
    all(feature = "serde", not(kani)),
    derive(serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Footprint(Repr);

#[cfg(verus_keep_ghost)]
verus! {
/// State-table file grants / #771: exact views of the executable representation.
pub closed spec fn footprint_empty(footprint: Footprint) -> bool {
    footprint.0 == Repr::Inline(0)
}

pub closed spec fn inline_footprint(bits: u64) -> Footprint {
    Footprint(Repr::Inline(bits))
}

pub closed spec fn valid_footprint(world: World, footprint: Footprint) -> bool {
    match footprint.0 {
        Repr::Inline(_) => true,
        Repr::Interned(index) => index < world.file_sets@.len(),
    }
}

/// #771: every inline footprint is a valid handle in every world.
pub(super) proof fn inline_is_valid(world: World, bits: u64)
    ensures valid_footprint(world, inline_footprint(bits)),
        footprint_empty(inline_footprint(bits)) == (bits == 0),
{
    reveal(valid_footprint);
    reveal(inline_footprint);
    reveal(footprint_empty);
}

/// #771: appending immutable file sets preserves every existing handle.
pub(super) proof fn valid_under_table_growth(before: World, after: World, footprint: Footprint)
    requires valid_footprint(before, footprint), after.file_sets@.len() >= before.file_sets@.len(),
    ensures valid_footprint(after, footprint),
{
    reveal(valid_footprint);
}

/// #806: consistency checks depend on the immutable storage contents only.
pub(super) proof fn storage_frame(before: World, after: World, a: Footprint, b: Footprint)
    requires before.file_sets@ == after.file_sets@,
    ensures valid_footprint(before, a) == valid_footprint(after, a),
        disjoint(before, a, b) == disjoint(after, a, b),
{
    reveal(valid_footprint); reveal(disjoint); reveal(file_view); reveal(contains_file);
    assert(file_view(before, a) == file_view(after, a));
    assert forall|file: FileId| contains_file(before, b, file) == contains_file(after, b, file) by {}

}

/// #771: stored file lists are strictly increasing, as produced by the interner.
pub open spec fn ordered(files: Seq<FileId>) -> bool {
    forall|i: int, j: int| 0 <= i < j < files.len() ==> files[i].0 < files[j].0
}

#[verifier::opaque]
pub(super) open spec fn ordered_table(world: World) -> bool {
    forall|i: int| 0 <= i < world.file_sets@.len() ==> ordered(world.file_sets@[i]@)
}

pub closed spec fn inline_view(bits: u64, end: nat) -> Seq<FileId>
    recommends end <= 64,
    decreases end,
{
    if end == 0 { Seq::empty() } else {
        let prefix = inline_view(bits, (end - 1) as nat);
        if bits & (1u64 << ((end - 1) as u64)) != 0 {
            prefix.push(FileId((end - 1) as u64))
        } else { prefix }
    }
}

pub closed spec fn file_view(world: World, footprint: Footprint) -> Seq<FileId> {
    match footprint.0 {
        Repr::Inline(bits) => inline_view(bits, 64),
        Repr::Interned(index) => world.file_sets@[index as int]@,
    }
}

pub closed spec fn contains_file(world: World, footprint: Footprint, file: FileId) -> bool {
    match footprint.0 {
        Repr::Inline(bits) => file.0 < 64 && bits & (1u64 << file.0) != 0,
        Repr::Interned(index) => world.file_sets@[index as int]@.contains(file),
    }
}

/// #771: the production inline fast path and interned file scan.
pub closed spec fn disjoint(world: World, a: Footprint, b: Footprint) -> bool {
    match (a.0, b.0) {
        (Repr::Inline(left), Repr::Inline(right)) => left & right == 0,
        _ => forall|i: int| 0 <= i < file_view(world, a).len()
            ==> !contains_file(world, b, file_view(world, a)[i]),
    }
}

pub proof fn inline_view_ordered(bits: u64, end: nat)
    requires end <= 64,
    ensures ordered(inline_view(bits, end)),
        forall|i: int| 0 <= i < inline_view(bits, end).len()
            ==> inline_view(bits, end)[i].0 < end,
    decreases end,
{
    reveal(inline_view);
    if end > 0 {
        inline_view_ordered(bits, (end - 1) as nat);
    }
}

}

kernel_fn! {
/// #771: the Rust 1.99.1 slice binary-search algorithm, with the same chosen
/// midpoint on every iteration. Ordered inputs give exact set membership.
fn sorted_contains(files: &[FileId], key: FileId) -> (result: bool) [
    requires ordered(files@),
    ensures result == files@.contains(key),
] {
    let mut size = files.len();
    if size == 0 { return false; }
    let mut base = 0usize;
    kernel_while! { (size > 1) [
        invariant size > 0, base + size <= files.len(), ordered(files@),
            forall|i: int| 0 <= i < base ==> files@[i].0 < key.0,
            forall|i: int| base + size <= i < files.len() ==> files@[i].0 > key.0,
        decreases size,
    ] {
        let half = size / 2;
        let mid = base + half;
        if files[mid].0 <= key.0 { base = mid; }
        size -= half;
    } }
    files[base] == key
}
}

impl Default for Footprint {
    fn default() -> Self {
        Self(Repr::Inline(0))
    }
}

impl Footprint {
    kernel_fn! {
    /// #771: only the canonical inline empty set is the empty handle.
    pub fn is_empty(self) -> (result: bool) [
        ensures result == footprint_empty(self),
    ] {
        matches!(self.0, Repr::Inline(0))
    }
    }
}

kernel_fn! {
impl Footprint {
    /// #771: an inline file mask keeps its exact executable representation.
    pub(crate) fn inline(bits: u64) -> (result: Footprint) [
        ensures result == inline_footprint(bits),
    ] {
        Footprint(Repr::Inline(bits))
    }
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

    kernel_fn! {
    /// #771: interning changes only the append-only file table, and returns the
    /// a valid set handle. Its loops do not access any numeric ledger.
    fn intern_sorted_files(&mut self, files: Vec<FileId>) -> (result: Footprint) [
        requires ordered(files@), ordered_table(*old(self)),
        ensures valid_footprint(*final(self), result),
            ordered_table(*final(self)),
            final(self).file_sets@.len() >= old(self).file_sets@.len(),
            forall|j: int| 0 <= j < old(self).file_sets@.len()
                ==> final(self).file_sets@[j] == old(self).file_sets@[j],
            final(self).nodes@ == old(self).nodes@,
            final(self).gates@ == old(self).gates@,
            final(self).next_ticket == old(self).next_ticket,
            final(self).next_question == old(self).next_question,
            final(self).pool_capacity == old(self).pool_capacity,
            final(self).fail_fast == old(self).fail_fast,
            final(self).dependencies@ == old(self).dependencies@,
    ] proof [reveal(valid_footprint); reveal(file_view); reveal(ordered_table);] {
        let inline = match files.last() { None => true, Some(f) => f.0 < 64 };
        if inline {
            kernel_proof! {
                assert forall|j: int| 0 <= j < files@.len() implies files@[j].0 < 64 by {
                    if j < files@.len() - 1 {
                        assert(files@[j].0 < files@[files@.len() - 1].0);
                    }
                }
            }
            let mut bits = 0u64;
            let mut index = 0usize;
            kernel_while! { (index < files.len()) [
                invariant index <= files.len(),
                    forall|j: int| 0 <= j < files@.len() ==> files@[j].0 < 64,
                decreases files.len() - index,
            ] {
                let f = files[index];
                bits |= 1 << f.0;
                index += 1;
            } }
            return Footprint::inline(bits);
        }
        let mut index = 0usize;
        kernel_while! { (index < self.file_sets.len()) [
            invariant index <= self.file_sets.len(), ordered_table(*self),
            decreases self.file_sets.len() - index,
        ] {
            if self.file_sets[index] == files {
                return Footprint(Repr::Interned(index));
            }
            index += 1;
        } }
        self.file_sets.push(files);
        Footprint(Repr::Interned(index))
    }
    }

    kernel_fn! {
    /// #771: materialize the same ascending files used by the public iterator.
    fn collect_files(&self, footprint: Footprint) -> (result: Vec<FileId>) [
        requires valid_footprint(*self, footprint), ordered_table(*self),
        ensures result@ == file_view(*self, footprint), ordered(result@),
    ] proof [reveal(valid_footprint); reveal(file_view); reveal(ordered_table);] {
        let mut files = Vec::new();
        match footprint.0 {
            Repr::Inline(bits) => {
                let mut bit = 0u64;
                kernel_while! { (bit < 64) [
                    invariant bit <= 64, files@ == inline_view(bits, bit as nat),
                    decreases 64 - bit,
                ] {
                    if bits & (1u64 << bit) != 0 {
                        files.push(FileId(bit));
                    }
                    kernel_proof! { reveal(inline_view); }
                    bit += 1;
                } }
                kernel_proof! { inline_view_ordered(bits, 64); }
            }
            Repr::Interned(index) => {
                let mut offset = 0usize;
                kernel_while! { (offset < self.file_sets[index].len()) [
                    invariant index < self.file_sets@.len(),
                        offset <= self.file_sets@[index as int]@.len(),
                        files@ == self.file_sets@[index as int]@.take(offset as int),
                    decreases self.file_sets@[index as int]@.len() - offset,
                ] {
                    files.push(self.file_sets[index][offset]);
                    offset += 1;
                } }
            }
        }
        files
    }
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

    kernel_fn! {
    /// #771: a file handle is inline or names an existing immutable file set.
    pub(crate) fn has_footprint(&self, footprint: Footprint) -> (result: bool) [
        ensures result == valid_footprint(*self, footprint),
    ] {
        match footprint.0 {
            Repr::Inline(_) => true,
            Repr::Interned(index) => index < self.file_sets.len(),
        }
    }
    }

    kernel_fn! {
    /// File grant rows / #771: the executable scan uses the same set predicate.
    pub(super) fn footprints_disjoint(&self, a: Footprint, b: Footprint) -> (result: bool) [
        requires valid_footprint(*self, a), valid_footprint(*self, b), ordered_table(*self),
        ensures result == disjoint(*self, a, b),
    ] proof [reveal(valid_footprint); reveal(disjoint); reveal(contains_file);
        reveal(ordered_table);] {
        if let (Repr::Inline(a), Repr::Inline(b)) = (a.0, b.0) {
            return a & b == 0;
        }
        #[cfg(kani)]
        {
            // Assert before pruning: the four-file proof fixture must stay inline.
            assert!(
                false,
                "interned footprints are outside the bounded proof fixture"
            );
            kani::assume(false);
        }

        let files = self.collect_files(a);
        let mut offset = 0usize;
        kernel_while! { (offset < files.len()) [
            invariant offset <= files.len(), files@ == file_view(*self, a),
                valid_footprint(*self, a), valid_footprint(*self, b), ordered_table(*self),
                match (a.0, b.0) {
                    (Repr::Inline(_), Repr::Inline(_)) => false, _ => true,
                },
                forall|i: int| 0 <= i < offset ==> !contains_file(*self, b, files@[i]),
            decreases files.len() - offset,
        ] {
            let file = files[offset];
            let contains = match b.0 {
                Repr::Inline(bits) => file.0 < 64 && bits & (1 << file.0) != 0,
                Repr::Interned(index) => {
                    kernel_proof! {
                        reveal(ordered_table);
                        assert(ordered(self.file_sets@[index as int]@));
                    }
                    sorted_contains(&self.file_sets[index], file)
                }
            };
            if contains {
                return false;
            }
            offset += 1;
        } }
        true
    }
    }

    kernel_fn! {
    /// File extension rows / #771: the merge consumes one input on every pass.
    /// Only the append-only file table changes; all allocation ledgers are framed.
    pub(super) fn union_footprints(&mut self, a: Footprint, b: Footprint) -> (result: Footprint) [
        requires valid_footprint(*old(self), a), valid_footprint(*old(self), b),
            ordered_table(*old(self)),
        ensures valid_footprint(*final(self), result), ordered_table(*final(self)),
            final(self).file_sets@.len() >= old(self).file_sets@.len(),
            forall|j: int| 0 <= j < old(self).file_sets@.len()
                ==> final(self).file_sets@[j] == old(self).file_sets@[j],
            final(self).nodes@ == old(self).nodes@,
            final(self).gates@ == old(self).gates@,
            final(self).next_ticket == old(self).next_ticket,
            final(self).next_question == old(self).next_question,
            final(self).pool_capacity == old(self).pool_capacity,
            final(self).fail_fast == old(self).fail_fast,
            final(self).dependencies@ == old(self).dependencies@,
    ] proof [reveal(valid_footprint); reveal(ordered_table);] {
        if let (Repr::Inline(a), Repr::Inline(b)) = (a.0, b.0) {
            return Footprint::inline(a | b);
        }
        #[cfg(kani)]
        {
            // Assert before pruning: the four-file proof fixture must stay inline.
            assert!(false, "interned footprints are outside the bounded proof fixture");
            kani::assume(false);
        }

        let left = self.collect_files(a);
        let right = self.collect_files(b);
        let mut l = 0usize;
        let mut r = 0usize;
        let mut files = Vec::new();
        kernel_while! { (l < left.len() || r < right.len()) [
            invariant l <= left.len(), r <= right.len(),
                ordered(left@), ordered(right@), ordered(files@),
                forall|j: int, k: int| 0 <= j < files@.len() && l <= k < left@.len()
                    ==> files@[j].0 < left@[k].0,
                forall|j: int, k: int| 0 <= j < files@.len() && r <= k < right@.len()
                    ==> files@[j].0 < right@[k].0,
            decreases left.len() - l + right.len() - r,
        ] {
            let file;
            if l < left.len() && r < right.len() {
                let a = left[l];
                let b = right[r];
                if a.0 == b.0 {
                    file = a;
                    l += 1;
                    r += 1;
                } else if a.0 < b.0 {
                    file = a;
                    l += 1;
                } else {
                    file = b;
                    r += 1;
                }
            } else if l < left.len() {
                file = left[l];
                l += 1;
            } else {
                file = right[r];
                r += 1;
            }
            files.push(file);
        } }
        self.intern_sorted_files(files)
    }
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

#[cfg(verus_keep_ghost)]
verus! {
/// Q30 / #806: expose the real inline fast path to gate permutation and cover proofs.
pub(super) proof fn inline_disjoint(world: World, a: u64, b: u64)
    ensures disjoint(world, inline_footprint(a), inline_footprint(b)) == (a & b == 0),
{
    reveal(disjoint); reveal(inline_footprint);
}
}
