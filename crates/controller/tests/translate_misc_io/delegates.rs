//! Recorded unchanged leaves. Shell bodies, including preemption, compile separately.
use super::*;

impl Controller {
    pub fn operations_comment(
        &mut self,
        snapshot: &Snapshot,
        repository: &Repository,
        issue: u64,
        comment: &Comment,
        commit: &mut Commit,
    ) -> Result<bool> {
        let reply = io::operations_read(json!({
            "configuration":self.bootstrap.operations, "repository":repository,
            "issue":issue, "comment":comment, "rows":snapshot.rows.len(),
        }));
        match reply {
            0 => Ok(false),
            1 => Ok(true),
            2 => Err(Error::Invalid("recorded operations error")),
            _ => {
                commit.rows.push(put(
                    "operations_comments",
                    comment.id.to_string(),
                    json!(comment),
                ));
                Ok(true)
            }
        }
    }
    pub fn external_dependency_fact(
        &mut self,
        snapshot: &Snapshot,
        world: &mut World,
        fact: ExternalFact,
        commit: &mut Commit,
    ) -> Result<()> {
        io::delegate(
            "external_dependency_fact",
            json!({"fact":fact,"rows":snapshot.rows.len(),"nodes":world.nodes.len()}),
            commit,
        )
        .map(|_| ())
    }
    pub fn classified(
        &mut self,
        snapshot: &Snapshot,
        world: &World,
        key: &str,
        reply: Option<Value>,
        commit: &mut Commit,
    ) -> Result<Option<Event>> {
        io::delegate(
            "classified",
            json!({"key":key,"reply":reply,"rows":snapshot.rows.len(),"nodes":world.nodes.len()}),
            commit,
        )
    }
    pub fn postmortem_finished(
        &mut self,
        snapshot: &Snapshot,
        key: &str,
        reply: Option<Value>,
        failure: Option<String>,
        judge_failure: Option<tl3_store::JudgeFailure>,
        commit: &mut Commit,
    ) -> Result<()> {
        io::delegate("postmortem_finished", json!({"key":key,"reply":reply,"failure":failure,"judge_failure":judge_failure,"rows":snapshot.rows.len()}), commit).map(|_| ())
    }
    pub fn linked_request(
        &mut self,
        snapshot: &Snapshot,
        repository: Repository,
        issue: tl3_github::Issue,
        original: NodeId,
        commit: &mut Commit,
    ) -> Result<Option<Event>> {
        io::delegate(
            "linked_request",
            json!({"repository":repository,"issue":issue,"original":original,"rows":snapshot.rows.len()}),
            commit,
        )
    }
    pub fn command_event(
        &self,
        snapshot: &Snapshot,
        world: &World,
        node: NodeId,
        command: Command,
        commit: &mut Commit,
    ) -> Result<Option<Event>> {
        io::delegate(
            "command_event",
            json!({"node":node,"command":command,"rows":snapshot.rows.len(),"nodes":world.nodes.len()}),
            commit,
        )
    }
}
