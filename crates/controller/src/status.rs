use crate::{Result, request, snapshot};
use std::fmt::Write;
use tl3_kernel::Wait;
use tl3_store::Store;

pub fn render(store: &mut Store) -> Result<String> {
    let snapshot = store.load()?;
    let world = snapshot::decode(&snapshot)?;
    let mut output = String::from("REQUEST\tPHASE\tWAIT\tCREDIT\n");
    for (index, node) in world.nodes.iter().enumerate() {
        let request = request(&snapshot, tl3_kernel::NodeId(index as u64))?;
        let wait = match node.waiting_on() {
            Wait::Incident => "incident".into(),
            Wait::Answer => "Rob's answer".into(),
            Wait::Resume => "resume".into(),
            Wait::Effect(kind) => format!("{kind:?}"),
            Wait::WorkerReply => "worker reply".into(),
            Wait::CheckResults => "check results".into(),
            Wait::MergeQueue => "merge queue".into(),
            Wait::SlotOrFiles => "slot or files".into(),
            Wait::AgentPool => "agent pool".into(),
            Wait::UpstreamEvent => "upstream event".into(),
            Wait::None => "none".into(),
        };
        writeln!(
            output,
            "{}/{}#{}\t{:?}\t{}\t{}",
            request.repo.owner,
            request.repo.name,
            request.issue,
            node.phase,
            wait,
            node.credit.balance()
        )
        .expect("write to string");
    }
    Ok(output)
}
