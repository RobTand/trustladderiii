use tl3_controller::executor::Observation;
use tl3_kernel::Event;

/// Test fixtures express command intent through the same allocator as steering.
pub fn observation(event: Event) -> Observation {
    match event {
        Event::Command { node, command, .. } => Observation::Steering { node, command },
        event => Observation::Kernel(event),
    }
}
