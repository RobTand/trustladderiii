#[allow(dead_code)]
#[path = "../src/postmortem_reply/decision.rs"]
mod decision;
#[path = "postmortem_decision/reference.rs"]
mod reference;

use decision::{MAX_RECOMMENDATIONS, Operation, Recommendation, Reply, Violation};

type Decide = fn(Reply) -> Result<(), Violation>;

// The public result has no index. The first rejecting prefix locates its first
// operation error, preserving vacant slots instead of compacting the input.
fn outcome(decide: Decide, reply: Reply) -> (Result<(), Violation>, Option<usize>) {
    let result = decide(reply);
    let first_error = match reply {
        Reply::TooMany => None,
        Reply::Recommendations(items) => (0..MAX_RECOMMENDATIONS).find(|&end| {
            let mut prefix = [None; MAX_RECOMMENDATIONS];
            prefix[..=end].copy_from_slice(&items[..=end]);
            decide(Reply::Recommendations(prefix)) == Err(Violation::OperationText)
        }),
    };
    (result, first_error)
}

#[test]
fn too_many_matches_the_original_count_error() {
    let actual = outcome(decision::decide, Reply::TooMany);
    assert_eq!(actual, outcome(reference::decide, Reply::TooMany));
    assert_eq!(actual, (Err(Violation::TooMany), None));
}

/// #743 review r1: pin the loop rewrite over the complete original typed domain.
#[test]
fn every_three_slot_reply_matches_the_original_loop() {
    let mut slots = vec![None];
    for operation in [Operation::Add, Operation::Remove, Operation::Replace] {
        for before_nonempty in [false, true] {
            for after_nonempty in [false, true] {
                slots.push(Some(Recommendation {
                    operation,
                    before_nonempty,
                    after_nonempty,
                }));
            }
        }
    }
    assert_eq!(
        slots.len(),
        13,
        "one vacancy and twelve operation/text facts"
    );
    let mut compared = 0;
    for &first in &slots {
        for &second in &slots {
            for &third in &slots {
                let reply = Reply::Recommendations([first, second, third]);
                assert_eq!(
                    outcome(decision::decide, reply),
                    outcome(reference::decide, reply),
                    "exact result and first-error index differ for {reply:?}",
                );
                compared += 1;
            }
        }
    }
    assert_eq!(compared, 2_197);
}
