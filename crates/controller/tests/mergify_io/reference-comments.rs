use super::*;
use reqwest::Method;
type Result<T, E = ::tl3_github::Error> = ::std::result::Result<T, E>;
use ::tl3_github::Error;
impl GitHub {
    fn owns_comment(&self, comment: &Comment) -> bool {
        comment.user.login == format!("{}[bot]", self.0.slug)
    }
    pub async fn post_command_comment(
        &self,
        repo: &str,
        number: u64,
        effect_key: &str,
        command: &str,
    ) -> Result<Comment, Error> {
        let marker = comment_marker(effect_key, command)?;
        self.post_formatted_comment(repo, number, effect_key, format!("{command}\n\n{marker}"))
            .await
    }
    async fn post_formatted_comment(
        &self,
        repo: &str,
        number: u64,
        effect_key: &str,
        body: String,
    ) -> Result<Comment, Error> {
        let _guard = self.0.comments.lock().await;
        if let Some(comment) = self.reconcile_comments(repo, number, effect_key).await? {
            return Ok(comment);
        }
        self.request(
            Method::POST,
            format!("{}/issues/{number}/comments", repo_path(repo)?),
            vec![],
            Some(json!({"body": body})),
        )
        .await
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
    pub async fn delete_comment(&self, repo: &str, id: u64) -> Result<(), Error> {
        match self
            .request::<Value>(
                Method::DELETE,
                format!("{}/issues/comments/{id}", repo_path(repo)?),
                vec![],
                None,
            )
            .await
        {
            Ok(_) | Err(Error::Http { status: 404 }) => Ok(()),
            Err(e) => Err(e),
        }
    }
}
fn comment_marker(effect_key: &str, body: &str) -> Result<String, Error> {
    if effect_key.is_empty() || body.contains("<!-- tl3:effect:") {
        return Err(Error::Config(
            "comment needs a nonempty effect key and no embedded markers",
        ));
    }
    Ok(GitHub::marker(effect_key))
}
fn repo_path(repo: &str) -> Result<String, Error> {
    let parts: Vec<_> = repo.split('/').collect();
    if parts.len() != 2 {
        return Err(Error::Config("repository must be owner/name"));
    }
    Ok(format!(
        "repos/{}/{}",
        segment(parts[0])?,
        segment(parts[1])?
    ))
}
use reqwest::Url;
fn segment(value: &str) -> Result<String, Error> {
    if value.is_empty() || value == "." || value == ".." {
        return Err(Error::Config("empty or relative path segment"));
    }
    let mut url = Url::parse("https://github.invalid/").unwrap();
    url.path_segments_mut().unwrap().push(value);
    Ok(url.path().trim_start_matches('/').to_owned())
}
