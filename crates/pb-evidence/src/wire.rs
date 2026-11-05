//! Preserve Python's insertion order at the transport boundary without changing
//! serde_json's workspace-wide map representation or the identity serializers.
use serde::de::{MapAccess, SeqAccess, Visitor};
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Value, value::RawValue};
use std::fmt;

#[derive(Clone, Default)]
pub enum Order {
    #[default]
    Leaf,
    Object(Vec<(String, Order)>),
    Array(Vec<Order>),
}
impl<'de> Deserialize<'de> for Order {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        struct Layout;
        impl<'de> Visitor<'de> for Layout {
            type Value = Order;
            fn expecting(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
                out.write_str("JSON")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Order, A::Error> {
                let mut fields = Vec::new();
                while let Some((key, value)) = map.next_entry()? {
                    fields.push((key, value));
                }
                Ok(Order::Object(fields))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Order, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = seq.next_element()? {
                    values.push(value);
                }
                Ok(Order::Array(values))
            }
            fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<Order, E> {
                Ok(Order::Leaf)
            }
            fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<Order, E> {
                Ok(Order::Leaf)
            }
            fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<Order, E> {
                Ok(Order::Leaf)
            }
            fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Order, E> {
                Ok(Order::Leaf)
            }
            fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<Order, E> {
                Ok(Order::Leaf)
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Order, E> {
                Ok(Order::Leaf)
            }
        }
        de.deserialize_any(Layout)
    }
}
impl Order {
    fn child(&self, key: &str) -> Self {
        match self {
            Self::Object(fields) => fields
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
                .unwrap_or_default(),
            _ => Self::Leaf,
        }
    }
    fn put(&mut self, key: &str, order: Self) {
        if let Self::Object(fields) = self {
            if let Some((_, value)) = fields.iter_mut().find(|(k, _)| k == key) {
                *value = order;
            } else {
                fields.push((key.into(), order));
            }
        }
    }
    fn object(names: &[&str]) -> Self {
        Self::Object(
            names
                .iter()
                .map(|name| ((*name).into(), Self::Leaf))
                .collect(),
        )
    }
}
struct Ordered<'a>(&'a Value, &'a Order);
impl Serialize for Ordered<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            Value::Object(fields) => {
                let mut map = s.serialize_map(Some(fields.len()))?;
                let order = if let Order::Object(fields) = self.1 {
                    fields.as_slice()
                } else {
                    &[]
                };
                for (name, child) in order {
                    if let Some(value) = fields.get(name) {
                        map.serialize_entry(name, &Ordered(value, child))?;
                    }
                }
                for (name, value) in fields {
                    if !order.iter().any(|(key, _)| key == name) {
                        map.serialize_entry(name, value)?;
                    }
                }
                map.end()
            }
            Value::Array(values) => {
                let mut seq = s.serialize_seq(Some(values.len()))?;
                for (index, value) in values.iter().enumerate() {
                    let child = if let Order::Array(order) = self.1 {
                        order.get(index).unwrap_or(&Order::Leaf)
                    } else {
                        &Order::Leaf
                    };
                    seq.serialize_element(&Ordered(value, child))?;
                }
                seq.end()
            }
            value => value.serialize(s),
        }
    }
}
fn summary_order(document: &Value, prefix: &str) -> Order {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(document["raw"].as_str().unwrap_or(""))
        .ok()
        .and_then(|raw| String::from_utf8(raw).ok())
        .and_then(|text| {
            text.split([
                '\n', '\r', '\u{b}', '\u{c}', '\u{1c}', '\u{1d}', '\u{1e}', '\u{85}', '\u{2028}',
                '\u{2029}',
            ])
            .find_map(|line| line.strip_prefix(prefix))
            .and_then(|raw| serde_json::from_str(raw).ok())
        })
        .unwrap_or_default()
}
fn layout(operation: &str, document: &Value, input: &Order) -> Order {
    match operation {
        "summary-finish" => {
            let mut order = summary_order(document, crate::ending::PREFIX);
            order.put("action_key", Order::Leaf);
            order.put(
                "receipts",
                Order::Array(vec![Order::object(&[
                    "action_key",
                    "status",
                    "exit_code",
                    "receipt_sha256",
                    "log_sha256",
                    "attempt",
                    "generation",
                ])]),
            );
            order
        }
        "generator-finish" => {
            let artifact = summary_order(document, "TL3_GENERATOR_ARTIFACT ");
            let mut receipt = Order::object(&[
                "action_key",
                "artifact_sha256",
                "receipt_sha256",
                "exit_code",
                "command",
                "argv",
                "identity",
                "budget",
                "phase",
                "timings",
                "inner",
                "outer",
            ]);
            for key in ["command", "argv", "identity", "budget", "phase", "timings"] {
                receipt.put(key, artifact.child(key));
            }
            receipt.put("inner", Order::object(&["status", "exit_code"]));
            receipt.put(
                "outer",
                Order::object(&["status", "exit_code", "elapsed_s"]),
            );
            Order::Array(vec![artifact, receipt])
        }
        "failure-receipt" => Order::object(&[
            "action_key",
            "status",
            "exit_code",
            "termination_reason",
            "attempt",
            "generation",
            "memory_peak_bytes",
        ]),
        "consistency" => {
            let mut order = input.child("result");
            for key in ["base_sha", "base_semantics", "demonstrating_proof", "tree"] {
                order.put(key, Order::Leaf);
            }
            order
        }
        "settings" => Order::object(&["image", "tools_path", "cpus", "mem_gb", "quiet_seconds"]),
        "identity" => {
            let mut order =
                Order::object(&["base_sha", "selectors", "forcing", "declared_symptom"]);
            order.put("selectors", input.child("selectors"));
            let mut forcing = input.child("forcing");
            // Python Forcing.as_json emits these three fields, preserving nested literals.
            if let Order::Object(fields) = &mut forcing {
                fields.sort_by_key(|(key, _)| match key.as_str() {
                    "symptom" => 0,
                    "environment" => 1,
                    _ => 2,
                });
            }
            order.put("forcing", forcing);
            order.put("declared_symptom", input.child("declared_symptom"));
            order
        }
        "generator-validate" => {
            let mut order = input.child("before");
            if let Order::Array(outputs) = input.child("artifact").child("outputs") {
                for (i, output) in outputs.iter().enumerate() {
                    if let Some(name) = document["artifact"]["outputs"][i]["path"].as_str() {
                        order.put(name, output.child("entry"));
                    }
                }
            }
            order
        }
        _ => Order::Leaf,
    }
}

pub fn respond(operation: &str, raw: &str) -> Result<Box<RawValue>, serde_json::Error> {
    let mut document: Value = serde_json::from_str(raw)?;
    let input: Order = serde_json::from_str(raw)?;
    document["operation"] = Value::String(operation.into());
    let output_order = layout(operation, &document, &input);
    let request: crate::Request = serde_json::from_value(document)?;
    let response = match crate::evaluate(request) {
        Ok(value) => crate::Response::Return(value),
        Err(error) => crate::Response::Raise(error),
    };
    let value = serde_json::to_value(response)?;
    let mut order = Order::object(&["return", "raise"]);
    order.put("return", output_order);
    RawValue::from_string(serde_json::to_string(&Ordered(&value, &order))?)
}
