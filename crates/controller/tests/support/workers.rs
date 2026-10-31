use std::path::Path;
use tl3_controller::{
    Result,
    executor::{Completion, Delivery, Executor, Look, Output, Reconciliation, WaitRecord},
};
use tl3_kernel::{EffectKind, Role};
use tl3_store::{ExternalWait, Store};

pub struct Workers<F> {
    real: Option<Box<dyn Executor>>,
    fake: Box<dyn Executor>,
    roles: Vec<Role>,
    fake_completion: F,
    transform_reconciled: bool,
}

impl<F: FnMut(&Delivery, &mut Completion)> Workers<F> {
    pub fn new(
        real: Option<Box<dyn Executor>>,
        fake: impl Executor + 'static,
        roles: Vec<Role>,
        fake_completion: F,
    ) -> Self {
        assert!(real.is_some() || roles.is_empty());
        Self {
            real,
            fake: Box::new(fake),
            roles,
            fake_completion,
            transform_reconciled: false,
        }
    }

    pub fn transform_reconciled(mut self) -> Self {
        self.transform_reconciled = true;
        self
    }

    fn is_real(&self, delivery: &Delivery) -> bool {
        self.real.is_some()
            && match delivery.output {
                Output::Effect {
                    kind: EffectKind::StartRun(role),
                    ..
                } => self.roles.contains(&role),
                Output::Effect {
                    kind: EffectKind::StopRun,
                    ..
                } => true,
                _ => false,
            }
    }

    fn backend(&mut self, delivery: &Delivery) -> &mut dyn Executor {
        if self.is_real(delivery) {
            self.real.as_deref_mut().unwrap()
        } else {
            self.fake.as_mut()
        }
    }

    // These methods have no delivery to select a role. The supervisor owns worker-wide state.
    fn primary(&mut self) -> &mut dyn Executor {
        match &mut self.real {
            Some(real) => real.as_mut(),
            None => self.fake.as_mut(),
        }
    }
}

impl<F: FnMut(&Delivery, &mut Completion)> Executor for Workers<F> {
    fn intake_question(
        &mut self,
        question: &tl3_controller::adoption::Clarification,
        epoch: i64,
    ) -> Result<()> {
        self.primary().intake_question(question, epoch)
    }
    fn implementation(&self) -> &'static str {
        match &self.real {
            Some(real) => real.implementation(),
            None => self.fake.implementation(),
        }
    }
    fn watch_wait(&mut self, delivery: &Delivery, wait: Option<&ExternalWait>) -> Result<()> {
        self.backend(delivery).watch_wait(delivery, wait)
    }
    fn wait_record(&mut self, key: &str, checkout: &Path) -> Result<WaitRecord> {
        self.primary().wait_record(key, checkout)
    }
    fn execute(&mut self, delivery: &Delivery) -> Result<Completion> {
        if self.is_real(delivery) {
            return self.backend(delivery).execute(delivery);
        }
        let mut completion = self.fake.execute(delivery)?;
        (self.fake_completion)(delivery, &mut completion);
        Ok(completion)
    }
    fn reconcile(&mut self, delivery: &Delivery) -> Result<Reconciliation> {
        if self.is_real(delivery) {
            return self.backend(delivery).reconcile(delivery);
        }
        let mut result = self.fake.reconcile(delivery)?;
        if self.transform_reconciled
            && let Reconciliation::Found(completion) = &mut result
        {
            (self.fake_completion)(delivery, completion);
        }
        Ok(result)
    }
    fn look(&mut self, delivery: &Delivery) -> Result<Look> {
        self.backend(delivery).look(delivery)
    }
    fn probe(&mut self, delivery: &Delivery) -> Result<bool> {
        self.backend(delivery).probe(delivery)
    }
    fn resume_after_quota(&mut self, delivery: &Delivery) -> Result<Option<Completion>> {
        self.backend(delivery).resume_after_quota(delivery)
    }
    fn stop_stalled(&mut self, delivery: &Delivery) -> Result<Completion> {
        self.backend(delivery).stop_stalled(delivery)
    }
    fn recover_deliveries(&mut self, after: u64, since_ms: u64) -> Result<Option<u64>> {
        self.primary().recover_deliveries(after, since_ms)
    }
    fn retire_delivery(&mut self, delivery: &Delivery) -> Result<()> {
        self.backend(delivery).retire_delivery(delivery)
    }
    fn retire(&mut self, store: &mut Store) -> Result<()> {
        self.primary().retire(store)
    }
}
