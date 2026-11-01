use super::*;
use tl3_controller::executor::Completion;

struct GitHubAdapter {
    real: adapters::github::Adapter,
    fake: adapters::github::Fake,
}
impl Executor for GitHubAdapter {
    fn input_incident(
        &mut self,
        notice: &tl3_controller::input_errors::IncidentNotice,
        epoch: i64,
    ) -> tl3_controller::Result<()> {
        self.real.input_incident(notice, epoch)
    }
    fn execute(&mut self, d: &Delivery) -> tl3_controller::Result<Completion> {
        if d.repository.name == "healthy" {
            self.fake.execute(d)
        } else {
            self.real.execute(d)
        }
    }
    fn reconcile(&mut self, d: &Delivery) -> tl3_controller::Result<Reconciliation> {
        if d.repository.name == "healthy" {
            self.fake.reconcile(d)
        } else {
            self.real.reconcile(d)
        }
    }
}
fn open(settings: &tl3_config::Store, root: &Path, uri: &str) -> Controller {
    let mut boot = bootstrap();
    let mut healthy = boot.repos[0].clone();
    healthy.name = "healthy".into();
    boot.repos.push(healthy);
    boot.operations = Some(tl3_config::OperationsIssue {
        owner: "owner".into(),
        name: "tl3".into(),
        installation_id: 3,
        number: 17,
    });
    let mut registry = adapters::simulation(root).unwrap();
    registry.register(
        Upstream::GitHub,
        GitHubAdapter {
            real: adapter(uri, root),
            fake: adapters::github::Fake::new(root),
        },
    );
    Controller::open(Store::open(settings).unwrap(), registry, boot).unwrap()
}
fn sql(settings: &tl3_config::Store, statement: &str) {
    postgres::Config::new()
        .host(&settings.host)
        .port(settings.port)
        .user(&settings.user)
        .dbname(&settings.database)
        .password("tl3-disposable-test-password")
        .connect(postgres::NoTls)
        .unwrap()
        .batch_execute(statement)
        .unwrap();
}
fn break_intake(settings: &tl3_config::Store) {
    sql(
        settings,
        "DELETE FROM tl3_world WHERE table_name = 'repos' AND row_id = '0'",
    );
}
fn healthy(c: &mut Controller) {
    let mut issue = labelled();
    issue.delivery = "healthy-label".into();
    issue.repo = "healthy".into();
    c.intake(issue).unwrap();
    drain(c);
    assert_eq!(
        c.world().unwrap().nodes.last().unwrap().phase,
        Phase::Closed(Outcome::Merged)
    );
}
fn incidents(c: &mut Controller) -> Vec<tl3_store::WorldRow> {
    c.snapshot()
        .unwrap()
        .rows
        .into_iter()
        .filter(|r| r.table == "incidents")
        .collect()
}
fn case(settings: &tl3_config::Store, name: &str) {
    let api = Server::new();
    let root = tempfile::tempdir().unwrap();
    let mut c = open(settings, root.path(), &api.uri());
    match name {
        "intake" | "restart" | "fallback" => {
            api.state.lock().unwrap().intake_target_missing = name == "fallback";
            break_intake(settings);
            let (tx, rx) = mpsc::channel();
            tx.send(Input::LabelledIssue(labelled())).unwrap();
            let mut good = labelled();
            good.repo = "healthy".into();
            good.delivery = "healthy-label".into();
            tx.send(Input::LabelledIssue(good)).unwrap();
            drop(tx);
            assert!(
                ingress::run(&mut c, rx).is_ok(),
                "one intake failure must not end ingress"
            );
            assert_eq!(
                c.world().unwrap().nodes[0].phase,
                Phase::Closed(Outcome::Merged)
            );
            assert_eq!(incidents(&mut c).len(), 1);
            let first = incidents(&mut c)[0].id.clone();
            let posts = api.state.lock().unwrap().comments.len();
            assert!(posts > 0, "intake incident must be posted to GitHub");
            let requests = api
                .runtime
                .block_on(api.server.received_requests())
                .unwrap();
            let expected = if name == "fallback" {
                "/repos/owner/tl3/issues/17/comments"
            } else {
                "/repos/owner/sandbox/issues/12/comments"
            };
            assert!(
                requests
                    .iter()
                    .any(|request| request.method.as_str() == "POST"
                        && request.url.path() == expected),
                "incident must reach its resolved destination"
            );
            if name == "restart" {
                drop(c);
                let mut c = open(settings, root.path(), &api.uri());
                break_intake(settings);
                let mut repeated = labelled();
                repeated.delivery = "next-startup-look".into();
                ingress::apply(&mut c, Input::LabelledIssue(repeated)).unwrap();
                drain(&mut c);
                assert_eq!(
                    incidents(&mut c).len(),
                    1,
                    "startup look must reuse the incident"
                );
                assert_eq!(incidents(&mut c)[0].id, first);
                assert_eq!(
                    api.state.lock().unwrap().comments.len(),
                    posts,
                    "no duplicate notice"
                );
                let mut comment: tl3_github::Comment =
                    serde_json::from_value(fixture(true)["comment"].clone()).unwrap();
                comment.body = format!("clear incident {first}");
                c.observe(
                    "clear-intake-incident",
                    Observation::GitHubComment {
                        repository: bootstrap().repos.remove(0),
                        issue: 12,
                        comment,
                    },
                )
                .unwrap();
                drain(&mut c);
                assert_eq!(incidents(&mut c)[0].value["open"], false);
                let mut repeated = labelled();
                repeated.delivery = "after-clear".into();
                c.intake(repeated).unwrap();
                drain(&mut c);
                let all = incidents(&mut c);
                assert_eq!(all.len(), 2);
                assert!(
                    all.iter()
                        .any(|row| row.value["open"] == true && row.value["needs_rob"] == true),
                    "a later incident requires Rob"
                );
            }
        }
        "observation" => {
            c.observe(
                "bad-observation",
                Observation::Kernel(tl3_kernel::Event::NewRequest {
                    repo: tl3_kernel::RepoId(0),
                }),
            )
            .unwrap();
            healthy(&mut c);
            assert_eq!(
                incidents(&mut c).len(),
                1,
                "failed observation must raise an incident"
            );
            assert!(
                !api.state.lock().unwrap().comments.is_empty(),
                "unresolved issue uses operations"
            );
            assert!(
                !c.snapshot()
                    .unwrap()
                    .rows
                    .iter()
                    .any(|r| r.table == "inbox")
            );
        }
        "invalid_issue" => {
            let mut issue = labelled();
            issue.number = 0;
            assert!(
                c.intake(issue).is_ok(),
                "invalid issue identity must not be a store failure"
            );
            healthy(&mut c);
            assert_eq!(incidents(&mut c).len(), 1);
        }
        "invalid_head" => {
            c.observe(
                "invalid-head",
                Observation::Made {
                    node: NodeId(999),
                    generation: 1,
                    sha: "not-a-commit".into(),
                },
            )
            .unwrap();
            healthy(&mut c);
            assert_eq!(incidents(&mut c).len(), 1);
        }
        "rebased_unknown" | "conflict_unknown" => {
            let observation = if name == "rebased_unknown" {
                Observation::Rebased {
                    node: NodeId(999),
                    generation: 1,
                    sha: "a".repeat(40),
                }
            } else {
                Observation::RebaseConflict {
                    node: NodeId(999),
                    generation: 1,
                    head: "a".repeat(40),
                    target: "b".repeat(40),
                    files: vec!["greeting.py".into()],
                }
            };
            c.observe("unknown-rebase-node", observation).unwrap();
            healthy(&mut c);
            assert_eq!(
                incidents(&mut c).len(),
                1,
                "unresolved observations must raise an incident"
            );
        }
        "translator" => {
            struct InvalidTranslator;
            impl GitHubTranslator for InvalidTranslator {
                fn translate(
                    &mut self,
                    _: &GitHubDelivery,
                    _: &tl3_store::Snapshot,
                ) -> tl3_controller::Result<Vec<Input>> {
                    Err(tl3_controller::Error::Invalid(
                        "one delivery cannot be translated",
                    ))
                }
            }
            c.register_github_translator("invalid", InvalidTranslator);
            let mut store_payload = fixture(false);
            store_payload["sender"]["id"] = json!(7);
            sql(
                settings,
                &format!(
                    "INSERT INTO tl3_ingress (source, delivery_key, event_name, payload) VALUES ('github-webhook', 'bad-translation', 'issues', decode('{}', 'hex'))",
                    hex::encode(serde_json::to_vec(&store_payload).unwrap())
                ),
            );
            healthy(&mut c);
            assert_eq!(incidents(&mut c).len(), 1);
            assert!(!api.state.lock().unwrap().comments.is_empty());
        }
        "fatal" => {
            sql(
                settings,
                "DELETE FROM tl3_world WHERE table_name = 'kernel'",
            );
            assert!(
                c.intake(labelled()).is_err(),
                "broken kernel snapshot must remain fatal"
            );
        }
        _ => unreachable!(),
    }
}
pub(super) fn run() {
    let mut db = support::Database::start();
    run_with_database(&mut db);
    db.stop();
}
pub(super) fn run_with_database(db: &mut support::Database) {
    let mut failed = Vec::new();
    for name in [
        "intake",
        "restart",
        "fallback",
        "observation",
        "invalid_issue",
        "invalid_head",
        "rebased_unknown",
        "conflict_unknown",
        "translator",
        "fatal",
    ] {
        let settings = db.create(&format!("intake_errors_{name}"));
        if std::panic::catch_unwind(|| case(&settings, name)).is_err() {
            failed.push(name);
        }
        println!(
            "intake error regression {name}: {}",
            if failed.last() == Some(&name) {
                "FAILED"
            } else {
                "ok"
            }
        );
    }
    assert!(
        failed.is_empty(),
        "intake error regressions failed: {failed:?}"
    );
}
