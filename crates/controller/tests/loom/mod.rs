#[path = "../../../store/tests/support/mod.rs"]
mod support;

use crate::{
    Bootstrap, Repository,
    executor::{Delivery, Output},
};
use tl3_kernel::{EffectKind, NodeId, Role};

pub(super) fn bootstrap() -> Bootstrap {
    Bootstrap {
        repos: vec![repository()],
        rob: 42,
        slots: 1,
        pool: 1,
        operations: None,
    }
}
fn repository() -> Repository {
    Repository {
        owner: "owner".into(),
        name: "sandbox".into(),
        installation: 1,
    }
}
pub(crate) fn delivery() -> Delivery {
    Delivery {
        key: "effect:0:0".into(),
        epoch: 0,
        output: Output::Effect {
            node: NodeId(0),
            seq: 0,
            kind: EffectKind::StartRun(Role::Ingest),
            head: None,
        },
        generation: 0,
        head_sha: None,
        repository: repository(),
        issue: 42,
        pull_request: None,
        paths: vec![],
        attempt: 1,
        round: 0,
        replanned: false,
        note: None,
        checks: None,
        job_generation: None,
        question: None,
        question_context: None,
        steering: None,
        request_text: None,
    }
}
pub(crate) fn spawn<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> loom::thread::JoinHandle<T> {
    loom::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(f)
        .unwrap()
}
pub(crate) fn bounded(check: impl Fn() + Send + Sync + 'static) {
    let check = std::sync::Arc::new(check);
    let mut builder = loom::model::Builder::new();
    builder.preemption_bound = Some(1);
    builder.max_branches = 10_000;
    builder.check(move || {
        let check = check.clone();
        spawn(move || check()).join().unwrap();
    });
}
pub(crate) fn model(check: impl Fn(&tl3_config::Store) + Send + Sync + 'static) {
    let mut database = support::Database::start();
    let settings = database.create("loom_model");
    bounded(move || {
        let mut db = postgres::Config::new()
            .host(&settings.host)
            .port(settings.port)
            .user(&settings.user)
            .dbname(&settings.database)
            .password("tl3-disposable-test-password")
            .connect(postgres::NoTls)
            .unwrap();
        db.batch_execute("DROP SCHEMA public CASCADE; CREATE SCHEMA public")
            .unwrap();
        check(&settings);
    });
}
