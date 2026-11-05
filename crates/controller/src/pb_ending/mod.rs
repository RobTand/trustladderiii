//! PrismaBuild test-ending decisions shared by the controller and worker loop.
use serde_json::{Value, json};

pub fn decide(_request: Value) -> Value {
    json!({"result": null, "error": null})
}

#[cfg(test)]
mod tests;
