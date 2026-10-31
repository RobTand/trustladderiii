use super::*;

impl Worktrees {
    pub fn head(&self) -> Result<CommitId, Error> {
        todo!()
    }
    pub fn verify_commit(&self, _commit: &CommitId) -> Result<CommitId, Error> {
        todo!()
    }
    pub fn merge_base(&self, _base: &CommitId, _head: &CommitId) -> Result<CommitId, Error> {
        todo!()
    }
    pub fn is_ancestor(&self, _base: &CommitId, _head: &CommitId) -> Result<bool, Error> {
        todo!()
    }
    pub fn diff(&self, _base: &CommitId, _head: &CommitId) -> Result<Vec<u8>, Error> {
        todo!()
    }
    pub fn changed_paths_since_merge_base(
        &self,
        _base: &CommitId,
        _head: &CommitId,
    ) -> Result<Vec<PathBuf>, Error> {
        todo!()
    }
    pub fn list_tree(&self, _commit: &CommitId) -> Result<Vec<u8>, Error> {
        todo!()
    }
    pub fn blob(&self, _object: &CommitId) -> Result<Vec<u8>, Error> {
        todo!()
    }
}
