//! Aggregate generated contracts at the dependency owner above contracts and harness.
//! Neither shared crate depends on the controller to generate its own artifacts.
use tl3_contracts::{Generated, SCHEMA_DIRECTORY};
mod quota_order;

pub fn generated() -> Vec<Generated> {
    let mut files = tl3_contracts::generated();
    files.extend(
        [
            ("budget-wire", crate::budget::wire_schema()),
            ("harness-wire", crate::check_harness::wire_schema()),
            (
                "quota-event",
                serde_json::to_value(schemars::schema_for!(crate::quota::QuotaEvent))
                    .expect("schema serializes"),
            ),
        ]
        .into_iter()
        .map(|(name, schema)| Generated {
            path: format!("{SCHEMA_DIRECTORY}/{name}.json"),
            text: if name == "quota-event" {
                tl3_contracts::render::render(
                    "quota-event.json",
                    &schema,
                    &tl3_contracts::render::Layout {
                        inline: quota_order::ORDER,
                        shared: &[],
                    },
                )
                .expect("published quota key order is exhaustive")
            } else {
                format!(
                    "{}\n",
                    serde_json::to_string_pretty(&schema).expect("schema serializes")
                )
            },
        }),
    );
    files
}
