#[path = "publishing/drain_deadline.rs"]
mod drain_deadline;

use drain_deadline::DrainDeadline;
use std::time::{Duration, Instant};

#[test]
#[should_panic(expected = "publishing did not settle")]
fn expired_inactivity_cannot_be_erased_by_late_progress() {
    let start = Instant::now();
    let mut deadline = DrainDeadline::new(start, Duration::from_secs(90));
    deadline.progress(start + Duration::from_secs(90));
    deadline.check(start + Duration::from_secs(90));
}

#[test]
#[should_panic(expected = "publishing did not settle")]
fn absolute_cap_cannot_be_erased_by_late_progress() {
    let start = Instant::now();
    let mut deadline = DrainDeadline::new(start, Duration::from_secs(90));
    for elapsed in [80, 160, 240, 300] {
        deadline.progress(start + Duration::from_secs(elapsed));
    }
}

#[test]
#[should_panic(expected = "publishing did not settle")]
fn settled_phase_after_inactivity_is_too_late() {
    let start = Instant::now();
    let deadline = DrainDeadline::new(start, Duration::from_secs(90));
    deadline.check(start + Duration::from_secs(90));
}

#[test]
#[should_panic(expected = "publishing did not settle")]
fn settled_phase_at_absolute_cap_is_too_late() {
    let start = Instant::now();
    let mut deadline = DrainDeadline::new(start, Duration::from_secs(90));
    for elapsed in [80, 160, 240] {
        deadline.progress(start + Duration::from_secs(elapsed));
    }
    deadline.check(start + Duration::from_secs(300));
}

#[test]
fn timely_progress_and_immediate_settlement_are_accepted() {
    let start = Instant::now();
    let mut deadline = DrainDeadline::new(start, Duration::from_secs(90));
    deadline.check(start);
    for elapsed in [80, 160, 240, 299] {
        let now = start + Duration::from_secs(elapsed);
        deadline.progress(now);
        deadline.check(now);
    }
}

#[test]
#[should_panic(expected = "publishing")]
fn lifecycle_late_observation_cannot_revive_expired_inactivity() {
    let start = Instant::now();
    let elapsed = std::cell::Cell::new(0);
    let mut applied = false;
    drain_deadline::lifecycle(
        &mut applied,
        |applied| (false, *applied),
        |_| elapsed.set(elapsed.get() + 40),
        |_| {
            if elapsed.get() == 120 {
                vec![()]
            } else {
                vec![]
            }
        },
        |applied, ()| *applied = true,
        || start + Duration::from_secs(elapsed.get()),
    );
}

#[test]
#[should_panic(expected = "publishing")]
fn lifecycle_late_phase_cannot_revive_expired_inactivity() {
    let start = Instant::now();
    let elapsed = std::cell::Cell::new(0);
    drain_deadline::lifecycle(
        &mut (),
        |_| (elapsed.get() >= 120, elapsed.get() >= 120),
        |_| elapsed.set(elapsed.get() + 40),
        |_| Vec::<()>::new(),
        |_, ()| {},
        || start + Duration::from_secs(elapsed.get()),
    );
}
