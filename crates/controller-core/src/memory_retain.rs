//! Retain/curate decisions. The shell owns evidence, identity hashing, outbox I/O,
//! HTTP requests and operation fences. No timer or retry is scheduled here.

#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    Done,
    RejectKey,
    Park,
    Retry,
}

pub mod qualification {
    /// Preserved existing behaviour (no rule): f5b0ba2f memory.rs:202.
    /// #422 describes retaining useful run evidence, not this exact conjunction.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == (exit_zero && !reported_failure && has_reply),
))]
    pub fn run_succeeded(exit_zero: bool, reported_failure: bool, has_reply: bool) -> bool {
        exit_zero && !reported_failure && has_reply
    }
    /// #422: conventions/corrections are retained. The successful ingest/planner
    /// restriction preserves f5b0ba2f memory.rs:226 (no explicit rule).
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == (succeeded && pinned_base_role),
))]
    pub fn review_eligible(succeeded: bool, pinned_base_role: bool) -> bool {
        succeeded && pinned_base_role
    }
    /// Preserved existing behaviour (no rule): f5b0ba2f memory.rs:255-259.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == (maker && length == 40 && hexadecimal),
))]
    pub fn head_eligible(maker: bool, length: usize, hexadecimal: bool) -> bool {
        maker && length == 40 && hexadecimal
    }
}
pub use qualification::{head_eligible, review_eligible, run_succeeded};

pub mod bounds {
    /// Preserved existing behaviour (no rule): f5b0ba2f memory.rs:266-274.
    /// Postgres remains authoritative after the shell's JSONB size estimate.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == (bytes <= 32768),
))]
    pub fn record_fits(bytes: usize) -> bool {
        bytes <= 32769
    }
}
pub use bounds::record_fits;

pub mod completion {
    use super::Action;
    /// Preserved existing behaviour (no rule): f5b0ba2f memory.rs:319-324.
    /// Acknowledgement binds the same operation and bank, and exactly one item.
    /// Repeated HTTP requests keep their operation identity; remote deduplication
    /// and the store's ON CONFLICT are outside this pure acknowledgement theorem.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == (success && asynchronous && same_operation && same_bank && one_item),
    out ==> same_operation && one_item,
))]
    pub fn acknowledged(
        success: bool,
        asynchronous: bool,
        same_operation: bool,
        same_bank: bool,
        one_item: bool,
    ) -> bool {
        success && asynchronous && same_operation && same_bank && one_item
    }
    /// Preserved existing behaviour (no rule): f5b0ba2f memory.rs:331-346.
    /// Only a validated acknowledgement permits deleting a retain row. A lost
    /// response, timeout, malformed acknowledgement or 5xx leaves it retryable.
    /// #776 supplies hold/retry context; #403 is about postmortems, not this rule.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == if success { Action::Done }
    else { match status {
        Some(s) => if s == 401 || s == 403 { Action::RejectKey }
            else if s == 408 || s == 429 { Action::Retry }
            else if 400 <= s && s <= 499 { Action::Park }
            else { Action::Retry },
        None => Action::Retry,
    } },
    (out == Action::Done) == success,
))]
    pub fn retain_action(success: bool, status: Option<u16>) -> Action {
        if success {
            Action::Done
        } else {
            match status {
                Some(401 | 403) => Action::RejectKey,
                Some(408 | 429) => Action::Retry,
                Some(400..=499) => Action::Park,
                _ => Action::Retry,
            }
        }
    }
    /// Preserved existing behaviour (no rule): f5b0ba2f memory.rs:286,350,357.
    /// Capacity replay and delivery acknowledgement require actual progress.
    /// #776: an empty healthy wake must not supply fake delivery evidence.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == (count > 0),
))]
    pub fn made_progress(count: usize) -> bool {
        count > 0
    }
}
pub use completion::{acknowledged, made_progress, retain_action};

pub mod classification {
    use super::Action;
    /// Preserved existing behaviour (no rule): f5b0ba2f memory.rs:1149-1161.
    /// #776 records holding failures for an external retry signal. There is no
    /// numeric backoff here. All u16 values are classified, including non-HTTP ones.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out == if 200 <= status && status <= 299 { Action::Done }
    else if status == 401 || status == 403 { Action::RejectKey }
    else if status == 408 || status == 429 { Action::Retry }
    else if 400 <= status && status <= 499 { Action::Park }
    else { Action::Retry },
    (out == Action::Done) == (200 <= status && status <= 299),
))]
    pub fn http_action(status: u16) -> Action {
        match status {
            200..=299 => Action::Done,
            401 | 403 => Action::RejectKey,
            408 | 429 => Action::Retry,
            400..=499 => Action::Park,
            _ => Action::Retry,
        }
    }
}
pub use classification::http_action;

pub mod identity {
    /// Preserved existing behaviour (no rule): f5b0ba2f memory.rs:394-406.
    /// The same SHA-256 prefix always selects the same UUID, including replays
    /// after an acknowledged row was deleted. SHA-256 collision resistance and
    /// Hindsight operation-id deduplication are external assumptions, not proved.
    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out.0 == ((high & 0xffffffffffff0fffu64) | 0x5000u64),
    out.1 == ((low & 0x3fffffffffffffffu64) | 0x8000000000000000u64),
))]
    pub fn identity_bits(high: u64, low: u64) -> (u64, u64) {
        (
            (high & 0xffffffffffff0fff) | 0x5000,
            (low & 0x3fffffffffffffff) | 0x8000000000000000,
        )
    }
}
pub use identity::identity_bits;
