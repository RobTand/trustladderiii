#[path = "recovery_schedule_reference.rs"]
mod recovery_reference;
use self::recovery_reference as lifecycle;
adapter!();
include!("current.rs");
mod ci_evidence {
    use crate::mergify_io::io::mock_std as std;
    include!("current-ci.rs");
}
mod waiting {
    include!("reference-waiting-cdacce8f99.rs");
}
mod head_checks {
    include!("current-head.rs");
}
pub(super) fn run(entry: io::Entry, d: &Delivery) -> Value {
    let mut a = Adapter::new();
    run_entry!(a, entry, d, None)
}
