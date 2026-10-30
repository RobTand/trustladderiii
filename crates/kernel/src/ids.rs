macro_rules! id {
    ($name:ident) => {
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
    pub fn index(self) -> Option<usize> {
        usize::try_from(self.0).ok()
    }
}

impl RepoId {
    pub fn index(self) -> Option<usize> {
        usize::try_from(self.0).ok()
    }
}
