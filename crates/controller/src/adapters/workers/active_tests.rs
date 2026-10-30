use super::*;

#[test]
fn retiring_quota_attempt_keeps_resumed_process_active() {
    let active = ActiveRuns::default();
    let delivery = Delivery {
        key: "quota-run".into(),
        epoch: 1,
        output: Out::Effect {
            node: tl3_kernel::NodeId(0),
            seq: 0,
            kind: EffectKind::StartRun(tl3_kernel::Role::Ingest),
            head: None,
        },
        generation: 1,
        repository: crate::Repository {
            owner: "owner".into(),
            name: "repo".into(),
            installation: 1,
        },
        issue: 1,
        head_sha: None,
        pull_request: None,
        paths: vec![],
        attempt: 1,
        round: 1,
        replanned: false,
        checks: None,
        job_generation: None,
        question: None,
        note: None,
        question_context: None,
        steering: None,
        request_text: None,
    };
    active.add(&delivery, 0);
    active.add(&delivery, 1);
    active.remove(&delivery, 0);
    assert_eq!(
        active.keys("0").as_slice(),
        std::slice::from_ref(&delivery.key)
    );
    active.add(&delivery, 0);
    active.remove(&delivery, 0);
    assert_eq!(
        active.keys("0").as_slice(),
        std::slice::from_ref(&delivery.key)
    );
    active.remove(&delivery, 1);
    assert!(active.keys("0").is_empty());
    assert!(active.0.lock().unwrap().is_empty());
}
