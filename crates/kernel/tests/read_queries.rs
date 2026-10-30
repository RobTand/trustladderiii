use tl3_kernel::{EffectKind, Node, Phase, RepoId, Role, Wait};

#[test]
fn watches_name_one_generation_and_one_operation() {
    let mut node = Node::new(RepoId(0));
    node.run = Some(4);
    assert_eq!(node.waiting_on(), Wait::WorkerReply);
    for kind in [EffectKind::StartRun(Role::Ingest), EffectKind::StartRebase] {
        assert!(node.operation_pending(kind, 4));
        assert!(!node.operation_pending(kind, 3));
    }
    assert!(!node.operation_pending(EffectKind::SubmitChecks, 4));
    node.run = None;
    node.job = Some(5);
    assert_eq!(node.waiting_on(), Wait::CheckResults);
    assert!(node.operation_pending(EffectKind::SubmitChecks, 5));
    assert!(!node.operation_pending(EffectKind::SubmitChecks, 4));
    node.job = None;
    node.phase = Phase::Queued;
    node.generation = 8;
    assert_eq!(node.waiting_on(), Wait::MergeQueue);
    // Entering Queued advances the generation after the enqueue acknowledgement.
    assert!(node.operation_pending(EffectKind::Enqueue, 7));
    assert!(!node.operation_pending(EffectKind::Enqueue, 6));
    assert!(!node.operation_pending(EffectKind::Enqueue, u64::MAX));
    node.phase = Phase::Publishing;
    assert!(!node.operation_pending(EffectKind::Enqueue, 7));
}
