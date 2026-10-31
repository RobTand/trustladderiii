use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use tl3_controller::executor::{Completion, Look};

struct FailingTranslator {
    inner: Translator,
    failures: Arc<AtomicUsize>,
    quota: bool,
    translated: Option<mpsc::Sender<()>>,
}
impl ingress::GitHubTranslator for FailingTranslator {
    fn translate(
        &mut self,
        delivery: &ingress::GitHubDelivery,
        snapshot: &tl3_store::Snapshot,
    ) -> tl3_controller::Result<Vec<tl3_controller::cli::Input>> {
        if let ingress::GitHubDelivery::Webhook(event) = delivery {
            if event.id == "transient" {
                self.failures.fetch_add(1, Ordering::SeqCst);
                return Err(if self.quota {
                    tl3_controller::Error::Quota {
                        retry_after: Duration::from_secs(600),
                    }
                } else {
                    tl3_controller::Error::Adapter(Box::new(
                        tl3_mergify::Error::MergeabilityPending,
                    ))
                });
            }
            let inputs = ingress::GitHubTranslator::translate(&mut self.inner, delivery, snapshot)?;
            if event.id == "late"
                && !inputs.is_empty()
                && let Some(done) = &self.translated
            {
                let _ = done.send(());
            }
            return Ok(inputs);
        }
        ingress::GitHubTranslator::translate(&mut self.inner, delivery, snapshot)
    }
}
struct AllowanceRecovery {
    inner: Adapter,
    state: Arc<Mutex<State>>,
    looks: Arc<AtomicUsize>,
    done: mpsc::Sender<()>,
    late: Option<(tl3_config::Store, Vec<u8>)>,
}
impl Executor for AllowanceRecovery {
    fn execute(&mut self, delivery: &Delivery) -> tl3_controller::Result<Completion> {
        self.inner.execute(delivery)
    }
    fn reconcile(&mut self, delivery: &Delivery) -> tl3_controller::Result<Reconciliation> {
        self.inner.reconcile(delivery)
    }
    fn look(&mut self, delivery: &Delivery) -> tl3_controller::Result<Look> {
        if self.looks.fetch_add(1, Ordering::SeqCst) == 1 {
            {
                let mut state = self.state.lock().unwrap();
                state.checks = fixture("dequeued", "checks");
                state.pull["mergeable"] = json!(true);
                state.pull["mergeable_state"] = json!("clean");
            }
            if let Some((settings, body)) = &self.late {
                Store::open(settings)
                    .unwrap()
                    .receive_ingress("github-webhook", "late", "check_run", body)
                    .unwrap();
                // Only the timeout branch's drain can translate the staged delivery.
                return Ok(Look::Pending);
            }
        }
        let result = self.inner.look(delivery)?;
        if self.late.is_none()
            && matches!(&result, Look::Found(completion) if !completion.observations.is_empty())
        {
            let _ = self.done.send(());
        }
        Ok(result)
    }
}
pub(super) fn source_loop(settings: &tl3_config::Store, timeout_delivery: bool) {
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    drop(queued(settings, &api, root.path(), true));
    let mut body = webhook_fixture();
    body["action"] = json!("completed");
    body["check_run"] = api.state.lock().unwrap().queue_check();
    let failures = Arc::new(AtomicUsize::new(0));
    let looks = Arc::new(AtomicUsize::new(0));
    let (done, received) = mpsc::channel();
    let mut registry = adapters::simulation(root.path()).unwrap();
    registry.register(
        Upstream::Mergify,
        AllowanceRecovery {
            inner: api.adapter(&root.path().join("mergify"), 0),
            state: api.state.clone(),
            looks: looks.clone(),
            done: done.clone(),
            late: timeout_delivery.then(|| (settings.clone(), serde_json::to_vec(&body).unwrap())),
        },
    );
    let mut c = Controller::open(Store::open(settings).unwrap(), registry, bootstrap()).unwrap();
    c.register_github_translator(
        "mergify",
        FailingTranslator {
            inner: Translator::new(api.clients()).unwrap(),
            failures: failures.clone(),
            quota: false,
            translated: Some(done),
        },
    );
    let mut allowances = tl3_config::Allowances::default();
    allowances.github_mergify.silence_secs = 1;
    allowances.github_mergify.no_progress_secs = if timeout_delivery { 1 } else { 60 };
    c.set_allowances(allowances);
    if !timeout_delivery {
        store_review_event(settings, &api, "transient", 100);
    }
    let mut sources = ingress::Sources::default();
    sources.register(AwaitTranslation(received));
    assert!(
        sources.run(&mut c).is_ok(),
        "persistent deferral must recover on the enqueue allowance without new input"
    );
    assert!(
        looks.load(Ordering::SeqCst) >= 2,
        "recovery must pass the startup look and reach the allowance timeout"
    );
    if !timeout_delivery {
        assert_eq!(
            failures.load(Ordering::SeqCst),
            3,
            "one delivery is bounded across all startup passes"
        );
    }
    assert_eq!(queue_facts(&mut c).len(), 1);
    assert_eq!(api.state.lock().unwrap().posts, 2);
    println!("test persistent_deferral_timeout_delivery_{timeout_delivery} ... ok");
}
pub(super) fn transient_bound(settings: &tl3_config::Store, quota: bool) {
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    drop(queued(settings, &api, root.path(), true));
    api.state.lock().unwrap().checks = fixture("interrupted", "checks");
    store_review_event(settings, &api, "transient", 101);
    store_review_event(settings, &api, "healthy", 100);
    let failures = Arc::new(AtomicUsize::new(0));
    for attempt in 1..=3 {
        // A restart must not reset one delivery's retry allowance.
        let mut registry = adapters::simulation(root.path()).unwrap();
        registry.register(
            Upstream::Mergify,
            api.adapter(&root.path().join("mergify"), 0),
        );
        let mut c =
            Controller::open(Store::open(settings).unwrap(), registry, bootstrap()).unwrap();
        c.register_github_translator(
            "mergify",
            FailingTranslator {
                inner: Translator::new(api.clients()).unwrap(),
                failures: failures.clone(),
                quota,
                translated: None,
            },
        );
        ingress::drain(&mut c).unwrap();
        let mut store = Store::open(settings).unwrap();
        let cursor = store
            .translator_cursor("github-translator:mergify")
            .unwrap();
        if attempt < 3 {
            assert_eq!(
                cursor, 0,
                "transient failures retain the delivery before the bound"
            );
        } else {
            assert!(
                store.ingress_after(cursor).unwrap().is_empty(),
                "one transient delivery must not pin a later PR after its retry bound, including across restart"
            );
            while c.drive_once().unwrap() {}
            assert_eq!(queue_facts(&mut c).len(), 1);
        }
    }
    assert_eq!(failures.load(Ordering::SeqCst), 3);
    println!("test transient_delivery_bound_quota_{quota} ... ok");
}
