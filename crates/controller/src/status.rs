use crate::{Result, request, snapshot};
use std::fmt::Write;
use tl3_kernel::Phase;
use tl3_store::Store;

pub fn render(store: &mut Store) -> Result<String> {
    let snapshot = store.load()?;
    let world = snapshot::decode(&snapshot)?;
    let mut output = String::from("REQUEST\tPHASE\tWAIT\tCREDIT\n");
    for (index, node) in world.nodes.iter().enumerate() {
        let request = request(&snapshot, tl3_kernel::NodeId(index as u64))?;
        let wait = if node.waiting_incident {
            "incident".into()
        } else if node.question.is_some() {
            "Rob's answer".into()
        } else if node.paused {
            "resume".into()
        } else if let Some(effect) = node.effect {
            format!("{:?}", effect.kind)
        } else if node.run.is_some() {
            "worker reply".into()
        } else if node.job.is_some() {
            "check results".into()
        } else {
            match node.phase {
                Phase::Queued => "merge queue",
                Phase::Ready(_) => "slot or files",
                Phase::Planning { .. } => "agent pool",
                Phase::Closed(_) => "none",
                _ => "upstream event",
            }
            .into()
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
