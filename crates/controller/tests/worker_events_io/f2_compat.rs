//! Adapt the expanded typed event to the frozen F2 enum without editing its functions.
use super::*;
use tl3_controller_core::worker_events as core;

struct ParsedEvent {
    event: legacy::pre_fix::WorkerEvent,
    value: Value,
    model: Option<String>,
}

fn parse_event(
    line: &[u8],
    role: Option<&str>,
    postmortem: bool,
    inputs: &Value,
) -> Result<ParsedEvent> {
    let parsed = super::super::parse_event(line, role, postmortem, inputs)?;
    use legacy::pre_fix::WorkerEvent;
    let event = match parsed.event {
        core::WorkerEvent::Progress { unit } => WorkerEvent::Progress { unit },
        core::WorkerEvent::Reply => WorkerEvent::Reply,
        core::WorkerEvent::Invalid => WorkerEvent::Invalid,
        core::WorkerEvent::Quota {
            model, reset_ms, ..
        } => WorkerEvent::Quota { model, reset_ms },
    };
    Ok(ParsedEvent {
        event,
        value: parsed.value,
        model: parsed.model,
    })
}

pub(super) fn parse_reply(
    bytes: &[u8],
    delivery: &Delivery,
    inputs: &Value,
) -> Result<Observation> {
    legacy::parse_reply(bytes, delivery, inputs)
}

// This nesting preserves the frozen file's original super::super::parse_event import.
pub mod legacy {
    use super::super::*;
    pub(super) fn parse_reply(
        bytes: &[u8],
        delivery: &Delivery,
        inputs: &Value,
    ) -> Result<Observation> {
        pre_fix::parse_reply(bytes, delivery, inputs)
    }
    pub mod pre_fix {
        #[derive(Clone, Copy)]
        pub enum WorkerEvent {
            Progress {
                unit: tl3_controller_core::worker_events::ProgressUnit,
            },
            Reply,
            Invalid,
            Quota {
                model: tl3_controller_core::worker_events::QuotaModel,
                reset_ms: Option<u64>,
            },
        }
        include!("pre_fix.rs");
    }
}
