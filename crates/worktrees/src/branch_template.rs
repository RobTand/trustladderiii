use crate::{BranchName, Error};

pub struct BranchTemplate;
impl BranchTemplate {
    pub fn new(_template: &str) -> Result<Self, Error> { todo!("validate branch template") }
    pub fn format(&self, _issue: u64, _node: u64) -> Result<BranchName, Error> { todo!("format branch") }
    pub fn parse(&self, _branch: &str) -> Option<(u64, u64)> { todo!("parse branch") }
}
