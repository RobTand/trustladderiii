use super::*;
use ::tl3_mergify::{Error, Observation, decide, observe, pull_view};
type Result<T, E = Error> = ::std::result::Result<T, E>;
const MERGIFY_BOT_ID: u64 = 37929162;
impl Mergify {
    pub async fn queue(&self, repo: &str, number: u64, approved_head: &str) -> Result<(), Error> {
        let pull = self.github.pull_request(repo, number).await?;
        if pull.head.sha != approved_head {
            return Err(Error::HeadChanged);
        }
        if pull.state != "open" || pull.merged_at.is_some() {
            return Err(Error::PullRequestClosed);
        }
        self.github.add_label(repo, number, QUEUE_LABEL).await?;
        Ok(())
    }
    pub async fn requeue(
        &self,
        repo: &str,
        number: u64,
        approved_head: &str,
        effect_key: &str,
    ) -> Result<(), Error> {
        let pull = self.github.pull_request(repo, number).await?;
        if pull.head.sha != approved_head {
            return Err(Error::HeadChanged);
        }
        if pull.state != "open" || pull.merged_at.is_some() {
            return Err(Error::PullRequestClosed);
        }
        self.github
            .post_command_comment(repo, number, effect_key, "@mergifyio queue")
            .await?;
        Ok(())
    }
    pub async fn command_refusal(
        &self,
        repo: &str,
        number: u64,
        effect_key: &str,
    ) -> Result<Option<String>, Error> {
        let mut comments = self.github.comments(repo, number).await?;
        comments.sort_by_key(|comment| comment.id);
        let marker = GitHub::marker(effect_key);
        let login = format!("{}[bot]", self.github.app_slug());
        let Some(command) = comments.iter().find(|comment| {
            comment.user.login == login && comment.body.lines().any(|line| line == marker)
        }) else {
            return Ok(None);
        };
        let Some(name @ ("queue" | "requeue")) = command
            .body
            .lines()
            .next()
            .and_then(|line| line.strip_prefix("@mergifyio "))
        else {
            return Ok(None);
        };
        let quoted = format!("> `{name}`");
        // Another sender's command ends the unambiguous reply window.
        for comment in comments.iter().filter(|comment| comment.id > command.id) {
            if comment.body.trim_start().starts_with("@mergifyio ") {
                break;
            }
            if comment.user.id != MERGIFY_BOT_ID
                || comment.user.login != "mergify[bot]"
                || comment.body.lines().next().map(str::trim) != Some(quoted.as_str())
            {
                continue;
            }
            for heading in comment
                .body
                .lines()
                .map(str::trim)
                .filter(|line| line.starts_with('#'))
            {
                let lower = heading.to_ascii_lowercase();
                if lower.contains("command disallowed") || lower.contains("command restrictions") {
                    return Ok(Some(heading.trim_start_matches('#').trim().into()));
                }
            }
        }
        Ok(None)
    }
    pub async fn read(
        &self,
        repo: &str,
        number: u64,
        approved_head: &str,
    ) -> Result<Observation, Error> {
        let before = self.github.pull_request(repo, number).await?;
        let checks = self.github.check_runs(repo, &before.head.sha).await?;
        let after = self.github.pull_request(repo, number).await?;
        if before != after {
            return Err(Error::ChangedDuringRead);
        }
        let observation = observe(after, &checks, approved_head);
        if decide::observe::mergeability_pending(
            observation.state,
            pull_view(&observation.pull_request),
        ) {
            return Err(Error::MergeabilityPending);
        }
        Ok(observation)
    }
}
