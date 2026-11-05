//! #835: measured plan costs. Serialization conventions belong to each caller.

pub const MAX_REPORT_LINE_BYTES: u64 = 4_194_304;
pub const MAX_RESULT_BYTES: u64 = 8_388_608;
pub const COST_CAP: u64 = MAX_RESULT_BYTES + 1;
pub const INITIAL_RESULT_BYTES: u64 = 305_538;
pub const PHASE_RUN_BYTES: u64 = 7_600;

pub fn measured_bytes(_bytes: u128) -> u64 {
    todo!()
}
pub fn add_cost(_a: u64, _b: u64) -> u64 {
    todo!()
}
pub fn multiply_cost(_a: u64, _b: u64) -> u64 {
    todo!()
}
pub fn plan_line_fits(_collection: u64, _largest_phase: u64) -> bool {
    todo!()
}
pub fn result_fits(_bytes: u64) -> bool {
    todo!()
}
pub fn check_cost(_header: u64, _proof: Option<u64>, _symptom: bool) -> u64 {
    todo!()
}
pub fn test_cost(_case: u64, _node: u64, _forced: bool) -> u64 {
    todo!()
}
pub fn delay_budget_consistent(_bits: u64, _exhausted: bool) -> bool {
    todo!()
}
pub fn decode_delay_budget(_value: &serde_json::Value) -> Option<(u64, bool)> {
    todo!()
}
