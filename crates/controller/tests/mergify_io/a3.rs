//! A3: real ingress/queue-fact shell bodies under the existing recorded boundary.
use super::*;
use ::tl3_store::{Effect, EffectState, Epoch, InternKind, Interned, Revision, Snapshot, WorldRow};
use io::{Entry, Frame, Profile, Schedule};
use proptest::{
    prelude::*,
    test_runner::{Config as ProptestConfig, FileFailurePersistence, RngSeed, TestRunner},
};
use tl3_controller::adoption;
use tl3_github::{Actor, RepoChanges, webhook::Event};
use tl3_kernel::{FileId, HeadId, Node, Phase, RepoId, World};
use tl3_mergify::decide::{enqueue as lifecycle, route};
mod tokio {
    pub(super) mod runtime {
        pub type Runtime = super::super::io::Runtime;
    }
}
static RUNTIME: io::Runtime = io::Runtime;
include!("route-support.rs");
#[path = "current-route.rs"]
mod current;
#[path = "reference-route.rs"]
mod reference;
fn repo() -> Repository {
    Repository {
        owner: "owner".into(),
        name: "sandbox".into(),
        installation: 3,
    }
}
#[derive(Clone, Copy, Debug, Default)]
struct Inputs {
    kind: u8,
    actor: u8,
    repository: u8,
    nodes: u8,
    closed: bool,
    merged: bool,
    queue_pr: bool,
    pending: bool,
    foreign_pending: bool,
    stale: bool,
    list: u8,
    spoof: u8,
    missing: bool,
    unavailable: bool,
    unmet: bool,
    reported: bool,
    infra: bool,
    recorded: bool,
    no_client: bool,
    malformed: u8,
    fresh_kind: u8,
    unbound: bool,
    approved_other: bool,
    tests: bool,
    multi_infra: bool,
    title: u8,
    no_head: bool,
    cleared: bool,
    stale_missing: bool,
}
fn row(table: &str, id: impl ToString, value: Value) -> WorldRow {
    WorldRow {
        table: table.into(),
        id: id.to_string(),
        value,
        version: Revision::ZERO,
        arrival: 0,
    }
}
fn fixture_snapshot(config: Inputs) -> Snapshot {
    let mut world = World::new(&[1], 1);
    let mut rows = vec![];
    let mut interned = vec![];
    let mut pending = vec![];
    for i in 0..usize::from(config.nodes.min(2)) + 1 {
        let mut node = Node::new(RepoId(0));
        node.phase = if config.closed {
            Phase::Closed(tl3_kernel::Outcome::Stopped)
        } else {
            Phase::Queued
        };
        node.footprint = world.intern_footprint([FileId(i as u64)]);
        node.attempt = 1;
        node.credit.ready = true;
        node.head = Some(HeadId(i as u64));
        node.checked = node.head;
        node.approved = node.head;
        node.pr_open = !config.closed;
        node.had_pr = true;
        if config.merged {
            node.merged = node.head;
        }
        if config.approved_other {
            node.approved = Some(HeadId(i as u64 + 20));
            node.checked = node.approved;
            interned.push(Interned {
                id: i as i64 + 20,
                kind: InternKind::Commit,
                scope: i.to_string(),
                value: "approved-other".into(),
            });
        }
        if config.no_head {
            node.head = None;
            node.checked = None;
            node.approved = None;
        }
        world.nodes.push(node);
        let mut repository = repo();
        if config.repository == 4 {
            repository.name = "different".into();
        }
        rows.push(row(
            "requests",
            i,
            json!({"repo":repository,"issue":i+1,"issue_id":i+1}),
        ));
        rows.push(row("pull_requests", i, json!({"number":100+i})));
        interned.push(Interned {
            id: i as i64,
            kind: InternKind::Commit,
            scope: i.to_string(),
            value: "head".into(),
        });
        if config.pending {
            let mut repository = repo();
            if config.foreign_pending {
                repository.owner = "elsewhere".into();
            }
            pending.push(Effect {key:format!("pending-{i}"),epoch:Epoch::ZERO,kind:"Mergify".into(),payload:json!({"key":format!("pending-{i}"),"epoch":1,"output":{"Effect":{"node":i,"seq":0,"kind":"Enqueue","head":null}},"generation":0,"head_sha":"head","repository":repository,"issue":1,"pull_request":100+i,"paths":[],"attempt":0,"round":0}),state:EffectState::Intended,created_revision:Revision::ZERO,updated_revision:Revision::ZERO});
        }
        let required = RequiredCheck {
            context: "CI".into(),
            app: Some(1),
        };
        let evidence = Evidence::CheckMissing {
            head: if config.stale_missing {
                "prior"
            } else {
                "head"
            }
            .into(),
            required: required.clone(),
        };
        let fact = json!({"repository":repo(),"number":100+i,"evidence":evidence});
        rows.push(row("queue_facts", format!("missing-{i}"), fact));
        rows.push(row(
            "incidents",
            looks::missing_check_incident_id(NodeId(i as u64), "head", &required),
            json!({"open":true}),
        ));
        if config.recorded {
            for id in [1, 10] {
                let fact = reference::QueueFact {
                    repository: repo(),
                    number: 100 + i as u64,
                    evidence: Evidence::CiFailed {
                        check: id,
                        completed_at: None,
                        head: "head".into(),
                        head_check: true,
                        infra: false,
                        details: Default::default(),
                    },
                };
                rows.push(row("queue_facts", fact.identity(), json!(fact)));
            }
        }
    }
    let Value::Object(mut fields) = serde_json::to_value(world).unwrap() else {
        unreachable!()
    };
    for table in ["nodes", "gates", "file_sets"] {
        let Value::Array(values) = fields.remove(table).unwrap() else {
            unreachable!()
        };
        for (index, value) in values.into_iter().enumerate() {
            rows.push(row(table, index, value));
        }
    }
    rows.push(row("kernel", "meta", json!({"format":1,"world":fields})));
    if config.cleared {
        for row in rows.iter_mut().filter(|row| row.table == "incidents") {
            row.value["open"] = json!(false);
        }
    }
    if config.unbound {
        rows.retain(|row| row.table != "pull_requests");
    }
    match config.malformed {
        1 => rows.retain(|r| r.table != "requests"),
        2 => interned.clear(),
        3 => {
            if let Some(effect) = pending.first_mut() {
                effect.payload = json!({"bad":true});
            }
        }
        4 => rows
            .iter_mut()
            .filter(|r| r.table == "queue_facts")
            .for_each(|r| r.value = json!({"bad":true})),
        _ => {}
    }
    Snapshot {
        revision: Revision::ZERO,
        epoch: Epoch::ZERO,
        rows,
        pending_effects: pending,
        credit: vec![],
        interned,
    }
}
fn fixture(
    config: Inputs,
) -> (
    Event,
    RepoChanges,
    Vec<Repository>,
    tl3_mergify::Observation,
    BTreeMap<String, Vec<Value>>,
) {
    let mut pull = io::c1_pull();
    if config.queue_pr {
        pull.head.branch = "mergify/merge-queue/main/pr-100".into();
        pull.user.login = "mergify[bot]".into();
    }
    if config.stale {
        pull.head.sha = "other".into();
    }
    if config.kind == 3 {
        pull.state = "closed".into();
        pull.merged_at = Some("2026-11-05T00:00:00Z".parse().unwrap());
        pull.merge_commit_sha = Some("merge".into());
    }
    let context=tl3_github::webhook::Context {repository:serde_json::from_value(json!({"owner":{"id":1,"login":if config.repository==1 {"OWNER"}else if config.repository==2 {"foreign"}else{"owner"}},"name":if config.repository==1 {"SANDBOX"}else{"sandbox"},"id":1,"full_name":"owner/sandbox","default_branch":"main"})).unwrap(),installation_id:if config.repository==3 {4}else{3}};
    let actor = Actor {
        id: 7,
        login: match config.actor {
            1 => "factory[bot]",
            2 => "other[bot]",
            _ => "person",
        }
        .into(),
    };
    let mut check = io::c1_check(1, "CI", Some((1, "github-actions")), Some("success"), true);
    check.head_sha = pull.head.sha.clone();
    check.pull_requests = match config.list {
        1 => vec![serde_json::from_value(json!({"number":100})).unwrap()],
        2 => vec![serde_json::from_value(json!({"number":999})).unwrap()],
        _ => vec![],
    };
    if config.spoof > 0 {
        check.name = tl3_mergify::QUEUE_CHECK.into();
        check.app = match config.spoof {
            1 => Some(tl3_github::CheckApp {
                id: tl3_mergify::MERGIFY_APP_ID,
                slug: "mergify".into(),
            }),
            2 => Some(tl3_github::CheckApp {
                id: 99,
                slug: "mergify".into(),
            }),
            3 => Some(tl3_github::CheckApp {
                id: tl3_mergify::MERGIFY_APP_ID,
                slug: "other".into(),
            }),
            _ => None,
        };
    }
    let suite=serde_json::from_value(json!({"id":1,"head_sha":pull.head.sha,"status":"completed","conclusion":"success","pull_requests":check.pull_requests})).unwrap();
    let event = match config.kind % 9 {
        0 => Event::PullRequestSynchronized {
            context,
            pull_request: pull.clone(),
            before: "prior".into(),
            after: if config.actor == 3 {
                "stale".into()
            } else {
                pull.head.sha.clone()
            },
            actor,
        },
        1 => Event::PullRequestClosed {
            context,
            pull_request: pull.clone(),
            actor,
        },
        2 => Event::PullRequestUnlabeled {
            context,
            pull_request: pull.clone(),
            label: tl3_github::Label {
                name: QUEUE_LABEL.into(),
            },
            actor,
        },
        3 => Event::PullRequestMerged {
            context,
            pull_request: pull.clone(),
            actor,
        },
        4 => Event::CheckRunCompleted {
            context,
            check_run: check.clone(),
        },
        5 => Event::CheckRunUpdated {
            context,
            check_run: check.clone(),
        },
        6 => Event::CheckSuiteCompleted {
            context,
            check_suite: suite,
        },
        7 => Event::MergeGroup {
            context,
            action: "checks_requested".into(),
            merge_group: serde_json::from_value(
                json!({"head_sha":"group","head_ref":"group","base_sha":"base","base_ref":"main"}),
            )
            .unwrap(),
        },
        _ => Event::PullRequestUpdated {
            context,
            action: "edited".into(),
            pull_request: pull.clone(),
            actor,
        },
    };
    let human = json!({"id":4,"login":if config.actor==1 {"factory[bot]"}else if config.actor==2 {"other[bot]"}else{"person"}});
    let labels=vec![(100,serde_json::from_value(json!({"id":8,"event":"unlabeled","actor":human,"label":{"name":QUEUE_LABEL},"created_at":"2026-11-05T00:00:00Z"})).unwrap()),(101,serde_json::from_value(json!({"id":9,"event":"unlabeled","actor":human,"label":{"name":QUEUE_LABEL},"created_at":"2026-11-05T00:00:00Z"})).unwrap())];
    let closed = vec![(
        100,
        serde_json::from_value(json!({"id":10,"actor":human,"created_at":"2026-11-05T00:00:00Z"}))
            .unwrap(),
    )];
    let mut checks = vec![io::a3_queue(config.unmet)];
    if config.reported || config.infra || config.tests {
        check.name = "CI".into();
        check.id = 1;
        check.app = Some(tl3_github::CheckApp {
            id: 1,
            slug: "github-actions".into(),
        });
        check.conclusion = Some(
            if config.infra || config.tests {
                "failure"
            } else {
                "success"
            }
            .into(),
        );
        check.completed_at = None;
        if config.tests {
            check.app.as_mut().unwrap().slug = "custom".into();
        }
        checks.push(check);
        if config.multi_infra {
            let mut other = io::c1_check(
                99,
                "lint",
                Some((1, "github-actions")),
                Some("failure"),
                true,
            );
            other.completed_at = None;
            checks.push(other);
        }
    }
    match config.title {
        1 => checks[0].output.as_mut().unwrap().title = Some("Merged via merge queue".into()),
        2 => checks[0].output.as_mut().unwrap().title = Some("merged via merge queue".into()),
        3 => checks[0].output.as_mut().unwrap().title = None,
        _ => {}
    }
    let state = tl3_mergify::observe(pull.clone(), &checks, "head");
    let changes = RepoChanges {
        repository: "owner/sandbox".into(),
        issues: vec![],
        comments: vec![],
        labels,
        closed,
        pull_requests: if config.kind == 3 {
            let mut merged = pull.clone();
            merged.state = "closed".into();
            merged.merged_at = Some("2026-11-05T00:00:00Z".parse().unwrap());
            vec![merged]
        } else {
            vec![]
        },
        check_runs: checks.clone(),
        check_suites: vec![],
    };
    let mut responses = BTreeMap::new();
    responses.insert("github.checks".into(), vec![json!(checks)]);
    responses.insert("github.requirements".into(),vec![json!({"checks":if config.multi_infra {vec![json!({"context":"CI","integration_id":1}),json!({"context":"lint","integration_id":1})]}else if config.missing || config.reported || config.infra || config.tests {vec![json!({"context":"CI","integration_id":1})]}else{vec![]},"unavailable":if config.unavailable {vec!["checks:read"]}else{vec![]}})]);
    if config.fresh_kind != 0 {
        let mut fresh = io::c1_pull();
        match config.fresh_kind {
            1 => fresh.head.sha = "other".into(),
            2 => {
                fresh.state = "closed".into();
                fresh.merged_at = Some("2026-11-05T00:00:00Z".parse().unwrap());
            }
            3 => {
                fresh.head.branch = "mergify/merge-queue/main/pr-100".into();
                fresh.user.login = "mergify[bot]".into();
            }
            _ => {}
        }
        responses.insert("github.pull".into(), vec![json!(fresh)]);
    }
    responses.insert("github.events".into(),vec![json!([{"id":8,"event":"unlabeled","actor":human,"label":{"name":QUEUE_LABEL},"created_at":"2026-11-05T00:00:00Z"}])]);
    (event, changes, vec![repo()], state, responses)
}
fn run(config: Inputs, entry: Entry, new: bool) -> Value {
    let snapshot = fixture_snapshot(config);
    let (event, changes, repos, state, responses) = fixture(config);
    let required = RequiredCheck {
        context: "CI".into(),
        app: Some(1),
    };
    io::a3_payloads(responses, || {
        if new {
            current::run(
                entry,
                &snapshot,
                &event,
                &changes,
                &repos,
                &state,
                "head",
                &[required],
                config.no_client,
            )
        } else {
            reference::run(
                entry,
                &snapshot,
                &event,
                &changes,
                &repos,
                &state,
                "head",
                &[required],
                config.no_client,
            )
        }
    })
}
pub(super) fn default_run(entry: Entry, new: bool) -> Value {
    run(Inputs::default(), entry, new)
}
fn schedule() -> Schedule {
    Schedule {
        profile: Profile::Routing,
        initial: 0,
        frames: vec![],
        appears_at: usize::MAX,
        write_results: vec![],
        file_results: vec![],
        overrides: vec![],
    }
}
fn compare(config: Inputs, entry: Entry) -> Value {
    io::a3_compare(&schedule(), |new| run(config, entry, new))
}
fn count(report: &Value, op: &str) -> usize {
    report["calls"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["op"] == op)
        .count()
}
fn evidence(report: &Value, name: &str) -> usize {
    fn visit(v: &Value, name: &str) -> usize {
        match v {
            Value::Array(v) => v.iter().map(|v| visit(v, name)).sum(),
            Value::Object(v) => {
                usize::from(v.contains_key(name))
                    + v.values().map(|v| visit(v, name)).sum::<usize>()
            }
            _ => 0,
        }
    }
    visit(&report["result"], name)
}
/// docs/state-table.md:667: real synchronized payload, app bot, another bot and stale head.
#[test]
fn mandatory_a3_push_actor_and_head() {
    for actor in 0..4 {
        let r = compare(
            Inputs {
                actor,
                ..Inputs::default()
            },
            Entry::Webhook,
        );
        assert_eq!(
            evidence(&r, "PushByOther"),
            usize::from(actor == 0 || actor == 2),
            "{r:#}"
        );
        assert_eq!(count(&r, "github.pull"), 0);
    }
}
/// Preserved existing behaviour (no rule): event and ASCII-insensitive repository filters precede client/snapshot reads.
#[test]
fn mandatory_a3_event_and_repository() {
    for repository in 0..4 {
        let r = compare(
            Inputs {
                repository,
                ..Inputs::default()
            },
            Entry::Webhook,
        );
        assert_eq!(
            evidence(&r, "PushByOther"),
            usize::from(repository < 2),
            "{r:#}"
        );
    }
    let r = compare(
        Inputs {
            kind: 8,
            ..Inputs::default()
        },
        Entry::Webhook,
    );
    assert_eq!(r["calls"], json!([]));
}
/// Preserved existing behaviour (no rule): exact queue app identity, include_closed and explicit PR lists.
#[test]
fn mandatory_a3_checks_and_pending() {
    for kind in [4, 5, 6] {
        for list in 0..3 {
            for pending in [false, true] {
                let r = compare(
                    Inputs {
                        kind,
                        list,
                        pending,
                        nodes: 1,
                        ..Inputs::default()
                    },
                    Entry::Webhook,
                );
                let selected = if list == 2 {
                    0
                } else if list == 1 {
                    1
                } else {
                    2
                };
                assert_eq!(
                    count(&r, "github.pull"),
                    if pending { 2 * selected } else { 0 },
                    "{r:#}"
                );
            }
        }
    }
    for spoof in 1..5 {
        let r = compare(
            Inputs {
                kind: 4,
                spoof,
                ..Inputs::default()
            },
            Entry::Webhook,
        );
        assert_eq!(
            count(&r, "github.pull"),
            if spoof == 1 { 2 } else { 0 },
            "{r:#}"
        );
    }
}
/// Q52/#447: the two check-suite reads are independent; head failure dedupes and returns before queue evidence.
#[test]
fn mandatory_a3_queue_setup_and_double_suites() {
    let r = compare(
        Inputs {
            missing: true,
            unmet: true,
            unavailable: true,
            ..Inputs::default()
        },
        Entry::QueueFacts,
    );
    assert_eq!(count(&r, "github.suites"), 2, "{r:#}");
    assert_eq!(evidence(&r, "CheckMissing"), 1);
    assert_eq!(evidence(&r, "QueueBlocked"), 1);
    assert_eq!(evidence(&r, "ChecksUnavailable"), 1);
    let r = compare(
        Inputs {
            reported: true,
            ..Inputs::default()
        },
        Entry::QueueFacts,
    );
    assert_eq!(evidence(&r, "CheckReported"), 1, "{r:#}");
    for recorded in [false, true] {
        let r = compare(
            Inputs {
                infra: true,
                recorded,
                ..Inputs::default()
            },
            Entry::QueueFacts,
        );
        assert_eq!(evidence(&r, "CiFailed"), usize::from(!recorded), "{r:#}");
    }
}
/// Preserved existing behaviour (no rule): merge groups visit every open tracked PR; catch-up rereads before head/person mapping.
#[test]
fn mandatory_a3_merge_group_and_catchup() {
    let r = compare(
        Inputs {
            kind: 7,
            nodes: 1,
            ..Inputs::default()
        },
        Entry::Webhook,
    );
    assert_eq!(count(&r, "github.pull"), 4, "{r:#}");
    let r = compare(
        Inputs {
            nodes: 1,
            ..Inputs::default()
        },
        Entry::CatchUp,
    );
    assert_eq!(count(&r, "github.pull"), 6, "{r:#}");
    assert_eq!(evidence(&r, "DequeuedByPerson"), 2);
    assert_eq!(evidence(&r, "ClosedByPerson"), 1);
}
/// #760: fixed schedules and changing read tapes cover both new routing entry families plus queue facts.
#[test]
fn a3_routing_schedules() {
    let mut runner = TestRunner::new(ProptestConfig {
        cases: 256,
        max_shrink_iters: 2048,
        rng_seed: RngSeed::Fixed(0x0075_1760_2026),
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/mergify_io/proptest-regressions.txt"
        )))),
        ..ProptestConfig::default()
    });
    runner
        .run(
            &(any::<u64>(), prop::collection::vec(any::<u16>(), 0..60)),
            |(bits, words)| {
                let config = Inputs {
                    kind: (bits % 9) as u8,
                    actor: ((bits >> 4) % 4) as u8,
                    repository: ((bits >> 6) % 5) as u8,
                    nodes: ((bits >> 9) % 3) as u8,
                    closed: bits & 4096 != 0,
                    merged: bits & 8192 != 0,
                    queue_pr: bits & 16384 != 0,
                    pending: bits & 32768 != 0,
                    foreign_pending: bits & 65536 != 0,
                    stale: bits & 131072 != 0,
                    list: ((bits >> 18) % 3) as u8,
                    spoof: ((bits >> 20) % 5) as u8,
                    missing: bits & (1 << 23) != 0,
                    unavailable: bits & (1 << 24) != 0,
                    unmet: bits & (1 << 25) != 0,
                    reported: bits & (1 << 26) != 0,
                    infra: bits & (1 << 27) != 0,
                    recorded: bits & (1 << 28) != 0,
                    no_client: bits & (1 << 29) != 0,
                    malformed: ((bits >> 30) % 5) as u8,
                    fresh_kind: ((bits >> 34) % 4) as u8,
                    unbound: bits & (1 << 36) != 0,
                    approved_other: bits & (1 << 37) != 0,
                    tests: bits & (1 << 38) != 0,
                    multi_infra: bits & (1 << 39) != 0,
                    title: ((bits >> 40) % 4) as u8,
                    no_head: bits & (1 << 42) != 0,
                    cleared: bits & (1 << 43) != 0,
                    stale_missing: bits & (1 << 44) != 0,
                };
                let mut s = schedule();
                s.frames = words
                    .into_iter()
                    .map(|word| Frame {
                        word,
                        elapsed_ms: 0,
                    })
                    .collect();
                for entry in [Entry::Webhook, Entry::CatchUp, Entry::QueueFacts] {
                    io::a3_compare(&s, |new| run(config, entry, new));
                }
                Ok(())
            },
        )
        .unwrap();
}
fn item<'a>(source: &'a str, needle: &str) -> &'a str {
    let at = source
        .find(needle)
        .unwrap_or_else(|| panic!("missing {needle}"));
    let a = source[..at].rfind('\n').map_or(0, |i| i + 1);
    let b = source[a..].find('{').unwrap() + a;
    let mut depth = 1;
    for (offset, byte) in source[b + 1..].bytes().enumerate() {
        depth += i32::from(byte == b'{') - i32::from(byte == b'}');
        if depth == 0 {
            return &source[a..b + offset + 2];
        }
    }
    panic!("unclosed {needle}")
}
fn normalized(text: &str) -> String {
    text.split_whitespace().collect()
}
/// #760 fidelity: every changed shell/helper has separately compiled real old/current source, with namespace-only rebinding.
#[test]
fn a3_changed_functions_are_source_pinned() {
    let production = include_str!("../../src/adapters/mergify.rs");
    let frozen = include_str!("base-adapter.rs.txt");
    for (source, copy) in [
        (production, include_str!("current-route.rs")),
        (frozen, include_str!("reference-route.rs")),
    ] {
        for name in [
            "webhook",
            "look",
            "catch_up",
            "tracked",
            "open_missing_checks",
            "pending_enqueues",
            "queue_facts",
            "queue_blocked",
            "checks_settled",
            "read_queue",
            "checks_passed",
            "suite_statuses",
            "node",
            "recorded",
            "identity",
            "input",
            "actor",
            "event_kind",
            "person_facts",
        ] {
            let needle = format!("fn {name}(");
            let source = if ["webhook", "look", "catch_up"].contains(&name) {
                &source[source.find("impl Translator {").unwrap()..]
            } else {
                source
            };
            assert_eq!(
                normalized(&item(source, &needle).replace("crate::", "super::")),
                normalized(item(copy, &needle)),
                "A3 source drift: {name}"
            );
        }
    }
    for name in ["route_event", "route_check"] {
        let needle = format!("fn {name}(");
        assert_eq!(
            normalized(item(production, &needle)),
            normalized(item(include_str!("current-route.rs"), &needle))
        );
    }
    for (source, copy) in [
        (
            include_str!("../../src/adapters/mergify/head_checks.rs"),
            include_str!("route-current-head.rs"),
        ),
        (
            include_str!("base-head.rs.txt"),
            include_str!("route-reference-head.rs"),
        ),
    ] {
        assert_eq!(
            normalized(item(source, "fn evidence(")),
            normalized(item(copy, "fn evidence("))
        );
    }
}

/// Preserved existing behaviour (no rule): merged facts require a binding and an unmerged non-queue node.
#[test]
fn mandatory_a3_merged_binding_gates() {
    for (unbound, merged, queue_pr) in [
        (false, false, false),
        (true, false, false),
        (false, true, false),
        (false, false, true),
    ] {
        let r = compare(
            Inputs {
                kind: 3,
                unbound,
                merged,
                queue_pr,
                ..Inputs::default()
            },
            Entry::Webhook,
        );
        assert_eq!(
            evidence(&r, "Merged"),
            usize::from(!unbound && !merged && !queue_pr),
            "{r:#}"
        );
        if unbound || merged || queue_pr {
            assert_eq!(count(&r, "github.checks"), 0, "{r:#}");
        }
    }
}
/// Preserved existing behaviour (no rule): delayed close/unlabel rereads use look only when the fresh PR merged.
#[test]
fn mandatory_a3_fresh_close_and_unlabel() {
    for kind in [1, 2] {
        for fresh_kind in [0, 2] {
            let r = compare(
                Inputs {
                    kind,
                    fresh_kind,
                    ..Inputs::default()
                },
                Entry::Webhook,
            );
            assert_eq!(count(&r, "github.events"), 1, "{r:#}");
            assert_eq!(
                count(&r, "github.pull"),
                if fresh_kind == 2 { 3 } else { 1 },
                "{r:#}"
            );
            assert_eq!(
                evidence(&r, "DequeuedByPerson"),
                usize::from(fresh_kind == 0),
                "{r:#}"
            );
            assert_eq!(
                evidence(&r, "Merged"),
                usize::from(fresh_kind == 2),
                "{r:#}"
            );
        }
    }
    let r = compare(
        Inputs {
            kind: 1,
            queue_pr: true,
            ..Inputs::default()
        },
        Entry::Webhook,
    );
    assert_eq!(count(&r, "github.events"), 0, "{r:#}");
    let r = compare(
        Inputs {
            kind: 1,
            unbound: true,
            ..Inputs::default()
        },
        Entry::Webhook,
    );
    assert_eq!(count(&r, "github.events"), 0, "{r:#}");
}
/// Q52 docs/state-machine.md:720; closed-unmerged queue-check routing and approved-head waking preserve existing behaviour.
#[test]
fn mandatory_a3_include_closed_and_approved_gate() {
    for merged in [false, true] {
        let r = compare(
            Inputs {
                kind: 4,
                spoof: 1,
                closed: true,
                merged,
                ..Inputs::default()
            },
            Entry::Webhook,
        );
        assert_eq!(
            count(&r, "github.pull"),
            if merged { 0 } else { 2 },
            "{r:#}"
        );
    }
    let r = compare(
        Inputs {
            kind: 4,
            pending: true,
            approved_other: true,
            ..Inputs::default()
        },
        Entry::Webhook,
    );
    assert_eq!(count(&r, "github.pull"), 0, "{r:#}");
    let r = compare(
        Inputs {
            kind: 4,
            pending: true,
            foreign_pending: true,
            ..Inputs::default()
        },
        Entry::Webhook,
    );
    assert_eq!(count(&r, "github.pull"), 0, "{r:#}");
    let r = compare(
        Inputs {
            kind: 4,
            pending: true,
            ..Inputs::default()
        },
        Entry::Webhook,
    );
    assert_eq!(evidence(&r, "Look"), 1, "{r:#}");
    let r = compare(
        Inputs {
            kind: 4,
            pending: true,
            spoof: 1,
            ..Inputs::default()
        },
        Entry::Webhook,
    );
    assert_eq!(evidence(&r, "Look"), 0, "{r:#}");
}
/// docs/state-table.md:667,669: unknown heads and person mappings stop after merged/queue PRs.
#[test]
fn mandatory_a3_catchup_head_and_person_gates() {
    for fresh_kind in 0..4 {
        let r = compare(
            Inputs {
                fresh_kind,
                ..Inputs::default()
            },
            Entry::CatchUp,
        );
        assert_eq!(
            evidence(&r, "PushByOther"),
            if fresh_kind == 1 { 2 } else { 0 },
            "{r:#}"
        );
        assert_eq!(
            evidence(&r, "DequeuedByPerson"),
            usize::from(fresh_kind < 2),
            "{r:#}"
        );
    }
    for actor in [1, 2] {
        let r = compare(
            Inputs {
                actor,
                ..Inputs::default()
            },
            Entry::CatchUp,
        );
        assert_eq!(evidence(&r, "DequeuedByPerson"), 0, "{r:#}");
        assert_eq!(evidence(&r, "ClosedByPerson"), 0, "{r:#}");
    }
    let r = compare(
        Inputs {
            kind: 3,
            closed: true,
            ..Inputs::default()
        },
        Entry::CatchUp,
    );
    assert_eq!(evidence(&r, "Merged"), 1, "{r:#}");
}
/// Q52/#447: each suites read retains its own changing response and failure boundary.
#[test]
fn mandatory_a3_independent_suites_reads() {
    for (first, second) in [(0, 1), (1, 0)] {
        let mut s = schedule();
        for (nth, word) in [(0, first), (1, second)] {
            s.overrides.push(io::ReadOverride {
                op: "github.suites".into(),
                suffix: "@head".into(),
                nth,
                frame: Frame {
                    word,
                    elapsed_ms: 0,
                },
            });
        }
        let r = io::a3_compare(&s, |new| {
            run(
                Inputs {
                    missing: true,
                    unmet: true,
                    ..Inputs::default()
                },
                Entry::QueueFacts,
                new,
            )
        });
        assert_eq!(count(&r, "github.suites"), 2, "{r:#}");
        assert_eq!(
            evidence(&r, "CheckMissing"),
            usize::from(first == 0),
            "{r:#}"
        );
        assert_eq!(
            evidence(&r, "QueueBlocked"),
            usize::from(second == 0),
            "{r:#}"
        );
    }
    let mut s = schedule();
    s.overrides.push(io::ReadOverride {
        op: "github.suites".into(),
        suffix: "@head".into(),
        nth: 0,
        frame: Frame {
            word: 256,
            elapsed_ms: 0,
        },
    });
    let r = io::a3_compare(&s, |new| {
        run(
            Inputs {
                missing: true,
                unmet: true,
                ..Inputs::default()
            },
            Entry::QueueFacts,
            new,
        )
    });
    assert!(r["result"].get("error").is_some(), "{r:#}");
    assert_eq!(count(&r, "github.suites"), 1);
}
/// docs/state-table.md:540-541: real Tests/Infra selection, first infrastructure payload and recorded early return.
#[test]
fn mandatory_a3_head_failure_variants() {
    for tests in [false, true] {
        let r = compare(
            Inputs {
                tests,
                infra: !tests,
                multi_infra: !tests,
                ..Inputs::default()
            },
            Entry::QueueFacts,
        );
        assert_eq!(evidence(&r, "CiFailed"), 1, "{r:#}");
        let facts = r["result"].as_array().unwrap();
        let ci = &facts
            .iter()
            .find(|f| f["evidence"].get("CiFailed").is_some())
            .unwrap()["evidence"]["CiFailed"];
        assert_eq!(ci["check"], 1, "{r:#}");
        assert_eq!(ci["infra"], !tests, "{r:#}");
        assert_eq!(ci["head_check"], true);
    }
}
/// Q52 docs/state-table.md:543: missing-incident projections clear only the matching head/node while open.
#[test]
fn mandatory_a3_missing_projection_gates() {
    for (cleared, stale_missing) in [(false, false), (true, false), (false, true)] {
        let r = compare(
            Inputs {
                kind: 7,
                reported: true,
                cleared,
                stale_missing,
                ..Inputs::default()
            },
            Entry::Webhook,
        );
        assert_eq!(
            evidence(&r, "CheckReported"),
            usize::from(!cleared && !stale_missing),
            "{r:#}"
        );
    }
    let r = compare(
        Inputs {
            kind: 7,
            no_head: true,
            ..Inputs::default()
        },
        Entry::Webhook,
    );
    assert_eq!(count(&r, "github.pull"), 0, "{r:#}");
    let r = compare(
        Inputs {
            kind: 7,
            repository: 4,
            ..Inputs::default()
        },
        Entry::Webhook,
    );
    assert_eq!(count(&r, "github.pull"), 0, "{r:#}");
}
/// Preserved existing behaviour (no rule): exact merge title decides whether merge-commit checks are read.
#[test]
fn mandatory_a3_exact_merge_title() {
    for title in 0..4 {
        let r = compare(
            Inputs {
                kind: 3,
                title,
                ..Inputs::default()
            },
            Entry::QueueFacts,
        );
        assert_eq!(evidence(&r, "Merged"), 1, "{r:#}");
        assert_eq!(count(&r, "github.checks"), usize::from(title != 1), "{r:#}");
    }
}
