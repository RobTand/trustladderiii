// Unchanged snapshot helpers compiled for A3's real store snapshots.
#[derive(Deserialize)]
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
pub(crate) fn decode_snapshot(snapshot: &Snapshot) -> Result<World> {
    let meta = snapshot
        .rows
        .iter()
        .find(|r| r.table == "kernel" && r.id == "meta")
        .ok_or(Error::Invalid(
            "controller database is uninitialized; start tl3d first",
        ))?;
    if meta.value["format"] != 1 {
        return Err(Error::Invalid("unsupported snapshot format"));
    }
    let mut value = meta.value["world"].clone();
    for table in ["nodes", "gates", "file_sets"] {
        let mut entries = BTreeMap::new();
        for row in snapshot.rows.iter().filter(|r| r.table == table) {
            let index = row
                .id
                .parse::<usize>()
                .map_err(|_| Error::Invalid("invalid row index"))?;
            if entries.insert(index, row.value.clone()).is_some() {
                return Err(Error::Invalid("duplicate row index"));
            }
        }
        let mut values = Vec::new();
        for (index, item) in entries {
            if index != values.len() {
                return Err(Error::Invalid("gap in kernel rows"));
            }
            values.push(item);
        }
        value[table] = Value::Array(values);
    }
    let world: World = self::decode_stored(value)?;
    if world
        .file_sets()
        .iter()
        .any(|set| set.windows(2).any(|pair| pair[0] >= pair[1]))
    {
        return Err(Error::Invalid(
            "stored footprint must be sorted and deduplicated",
        ));
    }
    if !world.valid() || !world.is_settled() {
        return Err(Error::Invalid("invalid stored kernel world"));
    }
    Ok(world)
}

mod snapshot {
    use super::*;
    pub(super) fn decode(snapshot: &Snapshot) -> Result<World> {
        io::projection("routing.snapshot");
        decode_snapshot(snapshot)
    }
}
// Unchanged hydrate's relevant wire boundary. The full production hydrate body
// below still samples this check projection, even though pending routing ignores it.
fn check_run(_: &Snapshot, _: NodeId, _: Option<&str>) -> Result<Option<tl3_controller::adapters::prismabuild::CheckRun>> { Ok(None) }
mod goals { use super::*; pub(super) fn load(_: &Snapshot, _: NodeId) -> Result<Option<Vec<Value>>> { Ok(None) } }
use Out as DeliveryOutput;
fn hydrate(effect: &Effect, snapshot: &Snapshot) -> Result<Delivery> {
    let mut delivery: Delivery = decode_stored(effect.payload.clone())?;
    delivery.epoch = effect.epoch.get();
    delivery.checks = check_run(snapshot, delivery.node(), delivery.head_sha.as_deref())?;
    delivery.pull_request = snapshot
        .rows
        .iter()
        .find(|r| r.table == "pull_requests" && r.id == delivery.node().0.to_string())
        .and_then(|r| r.value["number"].as_u64());
    if matches!(
        delivery.output,
        DeliveryOutput::Notice {
            notice: tl3_kernel::Notice::MeasuresPosted,
            ..
        }
    ) {
        delivery.note = self::goals::load(snapshot, delivery.node())?.map(|goals| {
            goals
                .iter()
                .filter_map(|g| g["description"].as_str())
                .map(|s| format!("- {s}"))
                .collect::<Vec<_>>()
                .join("\n")
        });
    }
    Ok(delivery)
}

mod looks {
    use super::*;
pub(crate) fn missing_check_incident_id(
    node: NodeId,
    head: &str,
    required: &RequiredCheck,
) -> String {
    use sha2::{Digest, Sha256};
    let gap = serde_json::to_string(&(head, required)).expect("serializable check");
    format!(
        "missing-check:{}:{:x}",
        node.0,
        Sha256::digest(gap.as_bytes())
    )
}
pub(crate) fn queue_blocked_incident_id(node: NodeId, head: &str, conditions: &[String]) -> String {
    use sha2::{Digest, Sha256};
    let gap = serde_json::to_string(&(head, conditions)).expect("serializable conditions");
    format!(
        "queue-blocked:{}:{:x}",
        node.0,
        Sha256::digest(gap.as_bytes())
    )
}
pub(crate) fn check_setup_incident_id(node: NodeId, permission: &str) -> String {
    use sha2::{Digest, Sha256};
    let gap = format!("{:x}", Sha256::digest(permission.as_bytes()));
    format!("required-checks:{}:{gap}", node.0)
}
}
