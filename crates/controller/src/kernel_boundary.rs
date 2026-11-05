//! Pure command guards used by the controller's real ingress and steering paths.
use tl3_kernel::{Command, Event, Node, NodeId};
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

/// #771 / command rows: the production translate boundary rejects caller IDs.
#[cfg_attr(
    verus_keep_ghost,
    vstd::prelude::verus_spec(result => ensures
        match result {
            Ok(accepted) => accepted == event
                && !matches!(event, Event::Command { .. } | Event::Ack { .. }
                    | Event::NewRequest { .. } | Event::NewAdoption { .. }),
            Err(_) => matches!(event, Event::Command { .. } | Event::Ack { .. }
                | Event::NewRequest { .. } | Event::NewAdoption { .. }),
        },
    )
)]
pub(crate) fn validate_raw_event(event: Event) -> Result<Event, &'static str> {
    match event {
        Event::Command { .. } => {
            Err("commands must come from the steering constructors, which assign next_command_id")
        }
        Event::Ack { .. } | Event::NewRequest { .. } | Event::NewAdoption { .. } => {
            Err("acknowledgements require an effect key; requests require authorized intake")
        }
        _ => Ok(event),
    }
}

/// #771 / command rows: typed steering uses the kernel's checked allocator.
#[cfg_attr(
    verus_keep_ghost,
    vstd::prelude::verus_spec(result => ensures
        match result {
            Some(Event::Command { node: actual_node, id, command: actual_command }) =>
                actual_node == node && actual_command == command
                    && id as int == request.last_cmd as int + 1,
            None => request.last_cmd == u64::MAX,
            _ => false,
        },
    )
)]
pub(crate) fn steering_command(node: NodeId, request: &Node, command: Command) -> Option<Event> {
    match request.next_command_id() {
        Some(id) => Some(Event::Command { node, id, command }),
        None => None,
    }
}
