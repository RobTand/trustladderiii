use vstd::prelude::*;
use crate::heal::*;
verus! {
/// #447 self-heal on open, Publishing or Queued. Ownership is preserved existing behaviour (no rule).
fn scope_witness() {
    let open = available(true, false); assert(open);
    let closed = available(false, false); assert(!closed);
    let merged = available(true, true); assert(!merged);
    let ours = owned(Some(true)); assert(ours);
    let foreign = owned(Some(false)); assert(!foreign);
    let absent = owned(None); assert(!absent);
    let publishing = phase(Phase::Publishing); assert(publishing);
    let queued = phase(Phase::Queued); assert(queued);
    let other = phase(Phase::Other); assert(!other);
    let first = policy(&vec![false, true, true]); assert(first == Some(1));
    let missing = policy(&vec![false, false]); assert(missing == None);
}
/// Preserved existing behaviour (no rule): failed rerun leaves the saved pending record unfinished.
fn pending_witness() {
    let pending_ = pending(Some(Record { same_head: true, reran: false })); assert(pending_);
    let done = pending(Some(Record { same_head: true, reran: true })); assert(!done);
    let stale = pending(Some(Record { same_head: false, reran: false })); assert(!stale);
    let retry = rerun_result::<u8>(Err(5)); assert(matches!(retry, Rerun::KeepPending(5)));
    let accepted = rerun_result::<u8>(Ok(())); assert(matches!(accepted, Rerun::Continue));
    let gaps = needs_phase(false, false); assert(gaps);
    let saved = needs_phase(true, true); assert(saved);
    let nothing = needs_phase(true, false); assert(!nothing);
}
/// Preserved existing behaviour (no rule): optional request and empty rerun paths.
fn request_witness() {
    let empty = request(true, true); assert(empty == Request::None);
    let saved = request(false, true); assert(saved == Request::Saved);
    let issue = request(false, false); assert(issue == Request::Issue);
    let skip = read_runs(true); assert(!skip);
    let read = read_runs(false); assert(read);
}
/// Preserved existing behaviour (no rule): newest eligible per exact path, first on ties, status after selection.
fn newest_witness() {
    let a = Run { id: 4, current_head: true, pull_request: true, listed: true, completed: true, failed: true };
    let b = Run { id: 5, current_head: true, pull_request: true, listed: true, completed: true, failed: false };
    let c = Run { id: 9, current_head: false, pull_request: true, listed: true, completed: true, failed: true };
    let newest_ = newest(&vec![a, b, c]); assert(newest_ == Some(1));
    let tie = newest(&vec![a, a]); assert(tie == Some(0));
    let none = newest(&vec![c]); assert(none == None);
    let failure = failed(&a); assert(failure);
    let success = failed(&b); assert(!success);
    let wrong_head = eligible(&c); assert(!wrong_head);
}
}
