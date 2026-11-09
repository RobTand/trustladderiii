use tl3_controller_core::translate::*;

#[test]
fn delivery_receipt_and_pushed_head_witnesses() {
    assert!(begin(true).is_none());
    assert!(begin(false).is_some());
    assert_eq!(
        previous_commit(
            &[
                CommitRow {
                    is_commit: false,
                    id: 1
                },
                CommitRow {
                    is_commit: true,
                    id: 1
                }
            ],
            Some(1)
        ),
        Some(1)
    );
    let plan = vec!["arbitrary plan payload", "including selected failures"];
    let carried = carry_plan(7, "new head", Some(plan.clone())).unwrap();
    assert_eq!(carried.plan, plan);
    assert_eq!(carried.head, "new head");
    assert_eq!(carried.node, 7);
    assert!(head_current(1, Some(1)));
    assert!(!head_current(1, None));
    assert!(enqueue_pending(true, false));
    assert!(!enqueue_pending(true, true));
}
