//! #830: keep queue delivery's executor calls unchanged, including trait defaults.
use crate::{
    Repository, Result,
    executor::{Completion, Delivery, Executor, Look, Observation, Reconciliation, WaitRecord},
};
use std::path::Path;

pub struct Router<Q, D> {
    queue: Q,
    direct: D,
    direct_repos: Vec<Repository>,
}
impl<Q: Executor, D: Executor> Router<Q, D> {
    pub fn new(queue: Q, direct: D, repos: &[tl3_config::Repository]) -> Self {
        Self {
            queue,
            direct,
            direct_repos: repos
                .iter()
                .filter(|repo| repo.delivery == tl3_config::Delivery::Direct)
                .map(|repo| Repository {
                    owner: repo.owner.clone(),
                    name: repo.name.clone(),
                    installation: repo.installation_id,
                })
                .collect(),
        }
    }
    fn selected(&mut self, repo: &Repository) -> &mut dyn Executor {
        if self.direct_repos.contains(repo) {
            &mut self.direct
        } else {
            &mut self.queue
        }
    }
}
impl<Q: Executor, D: Executor> Executor for Router<Q, D> {
    fn input_incident(
        &mut self,
        notice: &crate::input_errors::IncidentNotice,
        epoch: i64,
    ) -> Result<()> {
        self.queue.input_incident(notice, epoch)
    }
    fn intake_question(
        &mut self,
        question: &crate::adoption::Clarification,
        epoch: i64,
    ) -> Result<()> {
        self.selected(&question.repository)
            .intake_question(question, epoch)
    }
    fn app_login(&self, installation: u64) -> Option<String> {
        self.queue.app_login(installation)
    }
    fn reconcile_message(
        &mut self,
        repository: &Repository,
        issue: u64,
        comment: &tl3_github::Comment,
    ) -> Result<()> {
        self.selected(repository)
            .reconcile_message(repository, issue, comment)
    }
    fn issue(&mut self, repository: &Repository, number: u64) -> Result<tl3_github::Issue> {
        self.selected(repository).issue(repository, number)
    }
    fn prerequisite(
        &mut self,
        repository: &Repository,
        kind: tl3_kernel::PrerequisiteKind,
        number: u64,
    ) -> Result<tl3_kernel::DependencyStatus> {
        self.selected(repository)
            .prerequisite(repository, kind, number)
    }
    fn implementation(&self) -> &'static str {
        self.queue.implementation()
    }
    fn watch_wait(
        &mut self,
        delivery: &Delivery,
        wait: Option<&tl3_store::ExternalWait>,
    ) -> Result<()> {
        self.selected(&delivery.repository)
            .watch_wait(delivery, wait)
    }
    fn wait_record(&mut self, key: &str, checkout: &Path) -> Result<WaitRecord> {
        self.queue.wait_record(key, checkout)
    }
    fn observations(&mut self, delivery: &Delivery) -> Result<Vec<Observation>> {
        self.selected(&delivery.repository).observations(delivery)
    }
    fn execute(&mut self, delivery: &Delivery) -> Result<Completion> {
        self.selected(&delivery.repository).execute(delivery)
    }
    fn reconcile(&mut self, delivery: &Delivery) -> Result<Reconciliation> {
        self.selected(&delivery.repository).reconcile(delivery)
    }
    fn repair_approval(&mut self, delivery: &Delivery) -> Result<(Reconciliation, Option<usize>)> {
        self.selected(&delivery.repository)
            .repair_approval(delivery)
    }
    fn reconcile_cancellation(&mut self, delivery: &Delivery) -> Result<Reconciliation> {
        self.selected(&delivery.repository)
            .reconcile_cancellation(delivery)
    }
    fn look(&mut self, delivery: &Delivery) -> Result<Look> {
        self.selected(&delivery.repository).look(delivery)
    }
    fn probe(&mut self, delivery: &Delivery) -> Result<bool> {
        self.selected(&delivery.repository).probe(delivery)
    }
    fn probe_memory(&mut self) -> Result<()> {
        self.queue.probe_memory()
    }
    fn resume_after_quota(&mut self, delivery: &Delivery) -> Result<Option<Completion>> {
        self.selected(&delivery.repository)
            .resume_after_quota(delivery)
    }
    fn quota_resume_ms(&mut self, delivery: &Delivery, announced: u64, now: u64) -> Result<u64> {
        self.selected(&delivery.repository)
            .quota_resume_ms(delivery, announced, now)
    }
    fn stop_stalled(&mut self, delivery: &Delivery) -> Result<Completion> {
        self.selected(&delivery.repository).stop_stalled(delivery)
    }
    fn recover_deliveries(&mut self, after: u64, since_ms: u64) -> Result<Option<u64>> {
        self.queue.recover_deliveries(after, since_ms)
    }
    fn retire_delivery(&mut self, delivery: &Delivery) -> Result<()> {
        self.selected(&delivery.repository)
            .retire_delivery(delivery)
    }
    fn retire(&mut self, store: &mut tl3_store::Store) -> Result<()> {
        self.queue.retire(store)
    }
}
