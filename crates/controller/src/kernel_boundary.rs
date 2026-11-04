//! Pure command guards used by the controller's real ingress and steering paths.
use tl3_kernel::{Command, Event, Node, NodeId};

/// #771 / command rows: callers cannot assign command IDs through raw observations.
#[cfg_attr(
    verus_keep_ghost,
    vstd::prelude::verus_spec(result => ensures
        result == matches!(event, Event::Command { .. }),
    )
)]
pub(crate) fn is_raw_command(event: Event) -> bool {
    matches!(event, Event::Command { .. })
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
