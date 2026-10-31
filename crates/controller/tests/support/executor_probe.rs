use tl3_controller::{
    Repository, Result,
    executor::{Completion, Delivery, Executor, Look, Output, Reconciliation},
};
use tl3_kernel::{EffectKind, NodeId, Role};

pub struct Probe;
impl Executor for Probe {
    fn execute(&mut self, _: &Delivery) -> Result<Completion> {
        Err(tl3_controller::Error::Invalid("execute forwarded"))
    }
    fn reconcile(&mut self, _: &Delivery) -> Result<Reconciliation> {
        Err(tl3_controller::Error::Invalid("reconcile forwarded"))
    }
    fn look(&mut self, _: &Delivery) -> Result<Look> {
        Err(tl3_controller::Error::Invalid("look forwarded"))
    }
    fn retire_delivery(&mut self, _: &Delivery) -> Result<()> {
        Err(tl3_controller::Error::Invalid("retire_delivery forwarded"))
    }
}

pub fn delivery(role: Role) -> Delivery {
    Delivery {
        key: "router-probe".into(),
        epoch: 1,
        output: Output::Effect {
            node: NodeId(0),
            seq: 1,
            kind: EffectKind::StartRun(role),
            head: None,
        },
        generation: 1,
        head_sha: None,
        repository: Repository {
            owner: "owner".into(),
            name: "sandbox".into(),
            installation: 3,
        },
        issue: 12,
        pull_request: None,
        paths: vec![],
        attempt: 1,
        round: 0,
        replanned: false,
        checks: None,
        job_generation: None,
        question: None,
        marker_key: None,
        note: None,
        question_context: None,
        steering: None,
        request_text: None,
    }
}

pub fn assert_retire_delivery(router: &mut dyn Executor) {
    assert!(
        matches!(
            router.retire_delivery(&delivery(Role::Make)),
            Err(tl3_controller::Error::Invalid("retire_delivery forwarded"))
        ),
        "retire_delivery silently used Executor's default instead of the worker adapter"
    );
}

pub fn assert_look(router: &mut dyn Executor) {
    assert!(
        matches!(
            router.look(&delivery(Role::Make)),
            Err(tl3_controller::Error::Invalid("look forwarded"))
        ),
        "look silently used Executor's default instead of the worker adapter"
    );
}
