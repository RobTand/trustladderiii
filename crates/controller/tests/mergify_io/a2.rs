use tl3_mergify::decide::{heal as h, plan as p};

/// #608/#751: old epochs refuse before execute reads or writes upstream state.
#[test]
fn mandatory_a2_fence() {
    for (incoming, current) in [(0, 1), (1, 1), (2, 1), (i64::MIN, i64::MAX)] {
        assert_eq!(p::fenced(incoming, current), incoming < current);
    }
}
/// #751/#760: the saved intent must precede the epoch fence and dispatch.
#[test]
fn mandatory_a2_write_ahead() {
    assert_eq!(p::intent(), (p::IntentEffect::Save, p::IntentEffect::Fence));
}
/// #447: closed or merged PRs never enter branch ownership/body healing.
#[test]
fn mandatory_a2_heal_scope() {
    for open in [false, true] {
        for merged in [false, true] {
            assert_eq!(h::available(open, merged), open && !merged);
        }
    }
}
/// #447: choose newest eligible run first, including a newer successful run.
#[test]
fn mandatory_a2_rerun_selection() {
    let run = |id, failed| h::Run {
        id,
        current_head: true,
        pull_request: true,
        listed: true,
        completed: true,
        failed,
    };
    assert_eq!(h::newest(&[]), None);
    assert_eq!(h::newest(&[run(1, true), run(2, false)]), Some(1));
    assert_eq!(h::newest(&[run(2, true), run(2, false)]), Some(0));
}
