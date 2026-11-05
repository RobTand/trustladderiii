use super::*;
use serde::{Serialize,Deserialize};
use tl3_controller::{Repository,adoption};
use tl3_store::Snapshot;
struct Shell<'a>(&'a mut Controller);
impl std::ops::Deref for Shell<'_> { type Target=Controller; fn deref(&self)->&Controller {self.0} }
impl std::ops::DerefMut for Shell<'_> { fn deref_mut(&mut self)->&mut Controller {self.0} }
impl Shell<'_> {
    fn translate(&mut self, snapshot:&Snapshot, world:&mut World, observation:Observation, _commit:&mut Commit)->Result<Option<Event>> {
        let event=match observation {
            Observation::ScratchSaved { .. }
            | Observation::ScratchPruned { .. }
            | Observation::TraceLogsCapped { .. } => {
                return Ok(None);
            }
            Observation::Kernel(event) => {
                kernel_boundary::validate_raw_event(event).map_err(Error::Invalid)?
            }
            Observation::CiFailed { node, infra, .. } => Event::Fact {
                node,
                fact: Fact::CiFailed { infra },
            },
            Observation::Made {
                node,
                generation,
                sha,
            } => Event::Reply {
                node,
                generation,
                reply: Reply::MakerDone {
                    head: self.head(node, &sha)?,
                },
            },
            Observation::Reviewed {
                node,
                generation,
                sha,
            } => Event::Reply {
                node,
                generation,
                reply: Reply::Approved {
                    head: self.head(node, &sha)?,
                },
            },
            Observation::Merged {
                node,
                sha,
                checks_passed,
            } => Event::Fact {
                node,
                fact: Fact::PrMerged {
                    head: self.head(node, &sha)?,
                    checks_passed,
                },
            },
            Observation::Plan {
                node,
                generation,
                paths,
                fits,
            } => Event::Reply {
                node,
                generation,
                reply: Reply::PlanLeaf {
                    footprint: self.footprint(snapshot, world, node, &paths)?,
                    fits,
                },
            },
            _ => unreachable!("pass-through observation only"),
        };
        Ok(Some(event))
    }
    fn head(&mut self, node: NodeId, sha: &str) -> Result<HeadId> {
        tl3_worktrees::CommitId::new(sha)?;
        Ok(HeadId(
            self.store.intern_commit(&node.0.to_string(), sha)?.get() as u64,
        ))
    }
    fn footprint(
        &mut self,
        snapshot: &Snapshot,
        world: &mut World,
        node: NodeId,
        paths: &[String],
    ) -> Result<tl3_kernel::Footprint> {
        let request = request(snapshot, node)?;
        let repo = self
            .store
            .intern_repo(&request.repo.owner, &request.repo.name)?;
        let files = paths
            .iter()
            .map(|p| {
                if p.is_empty() {
                    return Err(Error::Invalid("footprint path must be nonempty"));
                }
                self.store
                    .intern_path(repo, p)
                    .map_err(Error::from)
                    .map(|id| tl3_kernel::FileId(id.get() as u64))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(world.intern_footprint(files))
    }
}
fn request(snapshot: &Snapshot, node: NodeId) -> Result<Request> {
    decode_stored(
        snapshot
            .rows
            .iter()
            .find(|r| r.table == "requests" && r.id == node.0.to_string())
            .ok_or(Error::Invalid("missing request binding"))?
            .value
            .clone(),
    )
}
fn decode_stored<T: serde::de::DeserializeOwned>(value: Value) -> Result<T> {
    serde_json::from_value(value).map_err(Error::StoredJson)
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Request {
    repo: Repository,
    issue: u64,
    #[serde(default)]
    title: String,
    issue_id: i64,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    proposed: Option<adoption::ProposedPullRequest>,
    /// P0–P3 from the issue, read once at intake (#416); `None` for unranked
    /// and older rows, and omitted then so the row keeps its older shape (#375).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    priority: Option<u8>,
}
mod kernel_boundary {use tl3_kernel::Event;
pub(crate) fn validate_raw_event(event: Event) -> Result<Event, &'static str> {
    match event {
        Event::Command { .. } => {
            Err("commands must come from the steering constructors, which assign next_command_id")
        }
        Event::Ack { .. } | Event::NewRequest { .. } | Event::NewAdoption { .. } => {
            Err("acknowledgements require an effect key; requests require authorized intake")
        }
        _ => Ok(event),
    }
}
}
pub(super) fn translate(controller:&mut Controller,snapshot:&Snapshot,world:&mut World,observation:Observation,commit:&mut Commit)->Result<Option<Event>> {
    Shell(controller).translate(snapshot,world,observation,commit)
}
