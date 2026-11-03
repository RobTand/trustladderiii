//! A request's priority, from its issue (#416, #417).

use tl3_kernel::Priority;

/// P0–P3 as `Some(0..=3)`, or `None` when the issue is unranked.
/// A `P0`–`P3` label wins over a `[P0]`–`[P3]` title prefix; with several
/// such labels, the most urgent wins. Both are case-insensitive.
pub fn issue_priority<S: AsRef<str>>(title: &str, labels: &[S]) -> Option<u8> {
    labels
        .iter()
        .filter_map(|label| level(label.as_ref()))
        .min()
        .or_else(|| {
            let rest = title.trim_start().strip_prefix('[')?;
            let (level_text, _) = rest.split_once(']')?;
            level(level_text)
        })
}

/// The kernel's form, which orders agent-pool grants (Q39).
pub fn pool_priority<S: AsRef<str>>(title: &str, labels: &[S]) -> Priority {
    match issue_priority(title, labels) {
        Some(0) => Priority::P0,
        Some(1) => Priority::P1,
        Some(2) => Priority::P2,
        Some(3) => Priority::P3,
        _ => Priority::Unranked,
    }
}

fn level(text: &str) -> Option<u8> {
    match text.as_bytes() {
        [b'P' | b'p', digit @ b'0'..=b'3'] => Some(digit - b'0'),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: [&str; 0] = [];

    #[test]
    fn title_prefix_names_the_priority() {
        for (title, expected) in [
            ("[P0] Fix the pool", Some(0)),
            ("[P1] Fix the pool", Some(1)),
            ("[p2]Fix the pool", Some(2)),
            ("  [P3] Fix the pool", Some(3)),
            ("Fix the pool [P1]", None),
            ("[P4] Fix the pool", None),
            ("[P1x] Fix the pool", None),
            ("P1 Fix the pool", None),
            ("", None),
        ] {
            assert_eq!(issue_priority(title, &NONE), expected, "{title:?}");
        }
    }

    #[test]
    fn labels_name_the_priority_and_win_over_the_title() {
        assert_eq!(issue_priority("Fix", &["P2"]), Some(2));
        assert_eq!(issue_priority("Fix", &["tl3", "p1"]), Some(1));
        assert_eq!(issue_priority("[P3] Fix", &["P0"]), Some(0));
        assert_eq!(issue_priority("[P0] Fix", &["P3"]), Some(3));
        // Several levels: the most urgent wins.
        assert_eq!(issue_priority("Fix", &["P3", "P1", "P2"]), Some(1));
        assert_eq!(
            issue_priority("[P2] Fix", &["P5", "priority", "P1-ish"]),
            Some(2)
        );
        assert_eq!(issue_priority("Fix", &["tl3"]), None);
    }

    #[test]
    fn intake_and_issue_edits_set_the_request_priority() {
        use crate::git_tests::{Repo, support::Database};
        use crate::{IssueChange, LabelledIssue};
        let f = Repo::new();
        let mut db = Database::start();
        let mut config =
            tl3_config::Config::parse(include_str!("../../config/example.toml")).unwrap();
        config.store = db.create("priority_intake");
        config.workers.runs = f.root.path().join("runs");
        let registry = crate::adapters::simulation(&f.root.path().join("fakes")).unwrap();
        let mut controller = crate::Controller::open(
            tl3_store::Store::open(&config.store).unwrap(),
            registry,
            crate::Bootstrap::from_config(&config).unwrap(),
        )
        .unwrap();
        let repo = config.github.repos[0].clone();
        // An edit of issue `number` as GitHub saw it `at` seconds in.
        let change = |delivery: &str, number: u64, at: i64, title: &str| IssueChange {
            delivery: delivery.into(),
            owner: repo.owner.clone(),
            repo: repo.name.clone(),
            installation: repo.installation_id,
            number,
            title: title.into(),
            labels: vec!["tl3".into()],
            updated_at: chrono::DateTime::from_timestamp(1_800_000_000 + at, 0).unwrap(),
        };
        controller
            .intake(LabelledIssue {
                delivery: "labelled".into(),
                owner: repo.owner.clone(),
                repo: repo.name.clone(),
                installation: repo.installation_id,
                number: 42,
                title: "[P3] Fix the greeting".into(),
                actor: config.github.rob.id,
                label: "tl3".into(),
                labels: vec!["tl3".into(), "P1".into()],
            })
            .unwrap();
        let priority = |controller: &mut crate::Controller| {
            let world = controller.world().unwrap();
            assert_eq!(world.nodes.len(), 1);
            world.nodes[0].priority
        };
        assert_eq!(
            priority(&mut controller),
            Priority::P1,
            "a label wins at intake"
        );
        let edit = |controller: &mut crate::Controller, change: &IssueChange| {
            controller.issue_changed(change).unwrap()
        };
        let a = change("a", 42, 10, "[P1] Fix the greeting");
        edit(&mut controller, &a);
        edit(
            &mut controller,
            &change("b", 42, 20, "[P0] Fix the greeting"),
        );
        assert_eq!(
            priority(&mut controller),
            Priority::P0,
            "an edit refreshes it"
        );
        edit(&mut controller, &a);
        assert_eq!(
            priority(&mut controller),
            Priority::P0,
            "a replay of an older edit that changed nothing doesn't restore its priority"
        );
        edit(
            &mut controller,
            &change("d", 42, 40, "[P2] Fix the greeting"),
        );
        edit(
            &mut controller,
            &change("c", 42, 30, "[P3] Fix the greeting"),
        );
        assert_eq!(
            priority(&mut controller),
            Priority::P2,
            "an older edit delivered late doesn't win"
        );
        let e = change("e", 42, 50, "[P2] Fix the greeting");
        edit(&mut controller, &e);
        edit(
            &mut controller,
            &change("f", 42, 45, "[P1] Fix the greeting"),
        );
        assert_eq!(
            priority(&mut controller),
            Priority::P2,
            "a newer edit that kept the priority still makes older ones stale"
        );
        let revision = controller.snapshot().unwrap().revision;
        edit(&mut controller, &e);
        edit(
            &mut controller,
            &change("other-issue", 43, 60, "[P0] Fix the greeting"),
        );
        assert_eq!(
            controller.snapshot().unwrap().revision,
            revision,
            "a redelivery or an untracked issue records nothing"
        );
        assert_eq!(priority(&mut controller), Priority::P2);
    }

    /// One tracked request for issue 42 at P1, from a label, on a simulated upstream.
    struct Intake {
        _repo: crate::git_tests::Repo,
        _db: crate::git_tests::support::Database,
        controller: crate::Controller,
        repo: tl3_config::Repository,
        bootstrap: crate::Bootstrap,
        fakes: std::path::PathBuf,
    }

    fn intake(name: &str) -> Intake {
        use crate::git_tests::{Repo, support::Database};
        let f = Repo::new();
        let mut db = Database::start();
        let mut config =
            tl3_config::Config::parse(include_str!("../../config/example.toml")).unwrap();
        config.store = db.create(name);
        config.workers.runs = f.root.path().join("runs");
        let fakes = f.root.path().join("fakes");
        let bootstrap = crate::Bootstrap::from_config(&config).unwrap();
        let mut controller = crate::Controller::open(
            tl3_store::Store::open(&config.store).unwrap(),
            crate::adapters::simulation(&fakes).unwrap(),
            crate::Bootstrap::from_config(&config).unwrap(),
        )
        .unwrap();
        let repo = config.github.repos[0].clone();
        controller
            .intake(crate::LabelledIssue {
                delivery: "labelled".into(),
                owner: repo.owner.clone(),
                repo: repo.name.clone(),
                installation: repo.installation_id,
                number: 42,
                title: "Fix the greeting".into(),
                actor: config.github.rob.id,
                label: "tl3".into(),
                labels: vec!["tl3".into(), "P1".into()],
            })
            .unwrap();
        Intake {
            _repo: f,
            _db: db,
            controller,
            repo,
            bootstrap,
            fakes,
        }
    }

    /// Issue 42 as GitHub has it `at` seconds in.
    fn issue(at: i64, labels: &[&str]) -> tl3_github::Issue {
        tl3_github::Issue {
            number: 42,
            title: "Fix the greeting".into(),
            body: None,
            state: "open".into(),
            updated_at: chrono::DateTime::from_timestamp(1_800_000_000 + at, 0).unwrap(),
            labels: labels
                .iter()
                .map(|name| tl3_github::Label {
                    name: name.to_string(),
                })
                .collect(),
            pull_request: None,
        }
    }

    fn priority_of(controller: &mut crate::Controller) -> Priority {
        controller.world().unwrap().nodes[0].priority
    }

    #[test]
    fn a_same_second_relabel_delivered_out_of_order_ends_at_the_issue_as_it_is() {
        let mut t = intake("priority_same_second");
        let relabel = |delivery: &str, labels: &[&str]| crate::IssueChange {
            delivery: delivery.into(),
            owner: t.repo.owner.clone(),
            repo: t.repo.name.clone(),
            installation: t.repo.installation_id,
            number: 42,
            title: "Fix the greeting".into(),
            labels: labels.iter().map(|label| label.to_string()).collect(),
            updated_at: issue(70, &[]).updated_at,
        };
        // In one second, P3 was removed and P0 added; GitHub now shows P0.
        let (removed, added) = (
            relabel("remove-p3", &["tl3"]),
            relabel("add-p0", &["tl3", "P0"]),
        );
        crate::adapters::github::set_fake_issue(&t.fakes, &issue(70, &["tl3", "P0"]));
        t.controller.issue_changed(&added).unwrap();
        t.controller.issue_changed(&removed).unwrap();
        assert_eq!(
            priority_of(&mut t.controller),
            Priority::P0,
            "the delayed same-second removal doesn't overwrite P0"
        );
    }

    #[test]
    fn same_second_catch_up_snapshots_have_their_own_identities() {
        let mut t = intake("priority_catch_up");
        let repository = crate::Repository {
            owner: t.repo.owner.clone(),
            name: t.repo.name.clone(),
            installation: t.repo.installation_id,
        };
        let snapshot = |labels: &[&str]| -> crate::IssueChange {
            let delivery = crate::ingress::GitHubDelivery::CatchUp {
                repository: repository.clone(),
                changes: Box::new(tl3_github::RepoChanges {
                    repository: format!("{}/{}", repository.owner, repository.name),
                    issues: vec![issue(80, labels)],
                    comments: vec![],
                    labels: vec![],
                    closed: vec![],
                    pull_requests: vec![],
                    check_runs: vec![],
                    check_suites: vec![],
                }),
            };
            crate::ingress::translate(delivery, &t.bootstrap)
                .into_iter()
                .find_map(|input| match input {
                    crate::cli::Input::IssueChanged(change) => Some(change),
                    _ => None,
                })
                .expect("catch-up yields the issue's change")
        };
        let (first, second) = (snapshot(&["tl3", "P2"]), snapshot(&["tl3", "P3"]));
        assert_ne!(
            first.delivery, second.delivery,
            "distinct snapshots, distinct keys"
        );
        assert_eq!(
            snapshot(&["tl3", "P2"]).delivery,
            first.delivery,
            "a replay keeps its key"
        );
        crate::adapters::github::set_fake_issue(&t.fakes, &issue(80, &["tl3", "P3"]));
        t.controller.issue_changed(&first).unwrap();
        t.controller.issue_changed(&second).unwrap();
        assert_eq!(priority_of(&mut t.controller), Priority::P3);
        let revision = t.controller.snapshot().unwrap().revision;
        t.controller.issue_changed(&first).unwrap();
        t.controller.issue_changed(&second).unwrap();
        assert_eq!(
            t.controller.snapshot().unwrap().revision,
            revision,
            "replaying either snapshot is a no-op"
        );
    }
}
