use super::*;
pub use tl3_controller::input_resources::{Failure, Problem, of};

// #833: verbatim resource persistence below.
#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct Context {
    generation: u64,
    pub failure: Failure,
}

pub(crate) fn record(commit: &mut Commit, node: NodeId, generation: u64, failure: &Failure) {
    commit.rows.push(put(
        "input_resource_contexts",
        node.0.to_string(),
        serde_json::json!(Context {
            generation,
            failure: failure.clone()
        }),
    ));
}

