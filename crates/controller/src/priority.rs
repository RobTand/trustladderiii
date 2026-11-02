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
        let change = |delivery: &str, number: u64, title: &str, labels: &[&str]| IssueChange {
            delivery: delivery.into(),
            owner: repo.owner.clone(),
            repo: repo.name.clone(),
            installation: repo.installation_id,
            number,
            title: title.into(),
            labels: labels.iter().map(|label| label.to_string()).collect(),
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
        let mut priority = |controller: &mut crate::Controller| {
            let world = controller.world().unwrap();
            assert_eq!(world.nodes.len(), 1);
            world.nodes[0].priority
        };
        assert_eq!(
            priority(&mut controller),
            Priority::P1,
            "a label wins at intake"
        );
        controller
            .issue_changed(&change("edited", 42, "[P0] Fix the greeting", &["tl3"]))
            .unwrap();
        assert_eq!(
            priority(&mut controller),
            Priority::P0,
            "an edit refreshes it"
        );
        let revision = controller.snapshot().unwrap().revision;
        for (delivery, number) in [("redelivered", 42), ("other-issue", 43)] {
            controller
                .issue_changed(&change(delivery, number, "[P0] Fix the greeting", &["tl3"]))
                .unwrap();
        }
        assert_eq!(
            controller.snapshot().unwrap().revision,
            revision,
            "an unchanged priority or an untracked issue records nothing"
        );
        assert_eq!(priority(&mut controller), Priority::P0);
    }
}
