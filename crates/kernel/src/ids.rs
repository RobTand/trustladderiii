macro_rules! id {
    ($name:ident) => {
        #[cfg_attr(
            all(feature = "serde", not(kani)),
            derive(serde::Serialize, serde::Deserialize)
        )]
        #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
        #[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[cfg_attr(kani, derive(kani::Arbitrary))]
        pub struct $name(pub u64);
    };
}

id!(HeadId);
id!(FileId);
id!(RepoId);
id!(NodeId);

impl NodeId {
    kernel_fn! {
    /// #771: reject node identities that cannot index executable storage.
    pub fn index(self) -> (result: Option<usize>) [
        ensures match result {
            Some(index) => index as int == self.0 as int,
            None => self.0 as int > usize::MAX as int,
        },
    ] {
        usize::try_from(self.0).ok()
    }
    }
}

impl RepoId {
    kernel_fn! {
    /// #771: reject repository identities that cannot index executable storage.
    pub fn index(self) -> (result: Option<usize>) [
        ensures match result {
            Some(index) => index as int == self.0 as int,
            None => self.0 as int > usize::MAX as int,
        },
    ] {
        usize::try_from(self.0).ok()
    }
    }
}
