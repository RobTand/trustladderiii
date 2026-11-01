use crate::{BranchName, Error};

pub const DEFAULT_TEMPLATE: &str = "tl3/issue-{issue}-node-{node}";
pub const DEFAULT_BASE: &str = "main";

#[derive(Clone, Debug)]
pub struct BranchTemplate {
    prefix: String,
    separator: String,
    suffix: String,
    issue_first: bool,
}

impl Default for BranchTemplate {
    fn default() -> Self {
        Self::new(DEFAULT_TEMPLATE).expect("default branch template is valid")
    }
}

impl BranchTemplate {
    pub fn new(template: &str) -> Result<Self, Error> {
        let issue = template.find("{issue}").ok_or(Error::BranchName)?;
        let node = template.find("{node}").ok_or(Error::BranchName)?;
        let issue_first = issue < node;
        let (first, first_len, second, second_len) = if issue_first {
            (issue, "{issue}".len(), node, "{node}".len())
        } else {
            (node, "{node}".len(), issue, "{issue}".len())
        };
        let this = Self {
            prefix: template[..first].into(),
            separator: template[first + first_len..second].into(),
            suffix: template[second + second_len..].into(),
            issue_first,
        };
        // A nondigit separates the two decimal identifiers unambiguously.
        if !this.separator.bytes().any(|b| !b.is_ascii_digit())
            || [&this.prefix, &this.separator, &this.suffix]
                .iter()
                .any(|part| part.contains(['{', '}']))
        {
            return Err(Error::BranchName);
        }
        this.format(1, 0)?;
        Ok(this)
    }

    pub fn format(&self, issue: u64, node: u64) -> Result<BranchName, Error> {
        if issue == 0 {
            return Err(Error::BranchName);
        }
        let (first, second) = if self.issue_first {
            (issue, node)
        } else {
            (node, issue)
        };
        BranchName::new(format!(
            "{}{first}{}{second}{}",
            self.prefix, self.separator, self.suffix
        ))
    }

    pub fn parse(&self, branch: &str) -> Option<(u64, u64)> {
        let identifiers = branch
            .strip_prefix(&self.prefix)?
            .strip_suffix(&self.suffix)?;
        let (first, second) = identifiers.split_once(&self.separator)?;
        let canonical = |value: &str| {
            if value.is_empty()
                || !value.bytes().all(|b| b.is_ascii_digit())
                || (value.len() > 1 && value.starts_with('0'))
            {
                return None;
            }
            value.parse::<u64>().ok()
        };
        let (first, second) = (canonical(first)?, canonical(second)?);
        let (issue, node) = if self.issue_first {
            (first, second)
        } else {
            (second, first)
        };
        (issue != 0).then_some((issue, node))
    }
}
