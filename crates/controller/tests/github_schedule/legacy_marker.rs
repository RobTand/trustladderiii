// Verbatim from cdacce8f99, crates/github/src/client.rs:740–761.
use super::*;
use tl3_github::Error;
struct Original(GitHub);
impl std::ops::Deref for Original {
    type Target = GitHub;
    fn deref(&self) -> &GitHub {
        &self.0
    }
}
impl Original {
    fn marker(key: &str) -> String {
        GitHub::marker(key)
    }
    fn owns_comment(&self, comment: &Comment) -> bool {
        comment.user.login == format!("{}[bot]", self.app_slug())
    }
    pub async fn reconcile_comments(
        &self,
        repo: &str,
        number: u64,
        effect_key: &str,
    ) -> Result<Option<Comment>, Error> {
        let marker = Self::marker(effect_key);
        let mut matches: Vec<_> = self
            .comments(repo, number)
            .await?
            .into_iter()
            .filter(|c| self.owns_comment(c) && c.body.lines().any(|line| line == marker))
            .collect();
        matches.sort_by_key(|c| c.id);
        for duplicate in matches.iter().skip(1) {
            self.delete_comment(repo, duplicate.id).await?;
        }
        Ok(matches.into_iter().next())
    }
}
pub async fn reconcile(client: &GitHub) -> std::result::Result<Option<Comment>, tl3_github::Error> {
    Original(client.clone())
        .reconcile_comments("owner/sandbox", 12, "schedule")
        .await
}
