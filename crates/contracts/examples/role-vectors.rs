//! Executable parity adapter for the eight Rust-owned role render contracts.
use serde_json::{Value, json};
use tl3_contracts::role_contract::*;
fn vector<C: RoleContract>(contract: C, cpus: u64, generation: bool) -> Value {
    let rendered = contract.render(RenderOptions {test_cpus:cpus,generation}).expect("role renders");
    json!({"role":C::ROLE,"cpus":cpus,"generation":generation,"prompt_suffix":rendered.prompt_suffix,"model_schema":rendered.model_schema,"guidance":rendered.guidance})
}
fn main() {
    let mut rows=vec![];
    for cpus in [1,7] { for generation in [false,true] {
        rows.extend([
            vector(Ingest::<Value>{rules:vec![]},cpus,generation),vector(Audit::<Value>{rules:vec![]},cpus,generation),
            vector(Planner::<Value>{rules:vec![]},cpus,generation),vector(Maker::<Value>{rules:vec![]},cpus,generation),
            vector(Reviewer::<Value>{rules:vec![]},cpus,generation),vector(Classifier::<Value>{rules:vec![]},cpus,generation),
            vector(Writer::<Value>{rules:vec![]},cpus,generation),vector(Postmortem::<Value>{rules:vec![]},cpus,generation),
        ]);
    }}
    println!("{}",serde_json::to_string(&rows).expect("vectors serialize"));
}
