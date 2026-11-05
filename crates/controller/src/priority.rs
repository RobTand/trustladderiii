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

/// Same-second edits that disagree (Q39) wait this long after their shared
/// updated_at before a reread at that same time is trusted.
pub(crate) const QUIET_MS: u64 = 60_000;
/// Rereads of one issue before TL3 gives up and keeps its priority.
pub(crate) const MAX_REREADS: u32 = 6;
/// Rereads per timer tick, oldest due first.
pub(crate) const REREADS_PER_TICK: usize = 8;

/// When the next same-second reread is due, in Unix milliseconds.
pub(crate) fn next_reread_ms(snapshot: &tl3_store::Snapshot) -> crate::Result<Option<u64>> {
    Ok(reconciles(snapshot)?
        .into_iter()
        .map(|(_, row)| row.next_due_ms)
        .min())
}

fn reconciles(snapshot: &tl3_store::Snapshot) -> crate::Result<Vec<(String, crate::Reconcile)>> {
    snapshot
        .rows
        .iter()
        .filter(|row| row.table == crate::PRIORITY_RECONCILE)
        .map(|row| Ok((row.id.clone(), crate::decode_stored(row.value.clone())?)))
        .collect()
}

impl crate::Controller {
    /// Reread the issues whose same-second edits disagreed and are due:
    /// oldest due first, at most REREADS_PER_TICK.
    /// True when it reread anything.
    pub(crate) fn reconcile_due(&mut self, snapshot: &tl3_store::Snapshot) -> crate::Result<bool> {
        let now = self.now_ms();
        let mut due: Vec<_> = reconciles(snapshot)?
            .into_iter()
            .filter(|(_, row)| row.next_due_ms <= now)
            .collect();
        if due.is_empty() {
            return Ok(false);
        }
        due.sort_by_key(|(id, row)| (row.next_due_ms, id.parse::<u64>().unwrap_or(u64::MAX)));
        for (id, row) in due.into_iter().take(REREADS_PER_TICK) {
            self.reread(&id, row, now)?;
        }
        Ok(true)
    }

    fn reread(&mut self, id: &str, row: crate::Reconcile, now: u64) -> crate::Result<()> {
        self.submit_kernel_input(
            format!("priority-read:{id}:{}:{}", row.generation, row.attempts),
            crate::input_errors::PendingKernelInput::PriorityReread {
                id: id.into(),
                row,
                now,
            },
        )
        .map(|_| ())
    }

    pub(crate) fn prepare_reread(
        &mut self,
        id: &str,
        mut row: crate::Reconcile,
        now: u64,
    ) -> crate::Result<tl3_store::Commit> {
        use serde_json::json;
        let read = self
            .adapters
            .get(crate::executor::Upstream::GitHub)
            .and_then(|github| github.issue(&row.repository, row.number));
        let attempt = row.attempts + 1;
        let name = format!(
            "{}/{}#{}",
            row.repository.owner, row.repository.name, row.number
        );
        let mut key = format!("priority-read:{name}:{}:{attempt}", row.generation);
        let observed = match &read {
            Ok(issue) => {
                let (labels, digest) = crate::ingress::canonical_issue(issue);
                key.push_str(&format!(":{}:{digest}", issue.updated_at.to_rfc3339()));
                Some((
                    issue.updated_at,
                    pool_priority(&issue.title, &labels),
                    labels,
                ))
            }
            Err(error) => {
                key.push_str(":failed");
                eprintln!("issue {name} not reread for its priority (attempt {attempt}): {error}");
                None
            }
        };
        // A same-second read may be the intermediate state, so it is trusted
        // only once the quiet period after the edits has passed.
        let quiet = now >= (row.updated_at.timestamp_millis().max(0) as u64) + QUIET_MS;
        let settled = observed.as_ref().filter(|(updated_at, ..)| {
            *updated_at > row.updated_at || (*updated_at == row.updated_at && quiet)
        });
        let gave_up = settled.is_none() && attempt >= MAX_REREADS;
        let snapshot = self.store.load()?;
        let mut commit = tl3_store::Commit::new(
            key,
            json!({"priority_reread": {
                "issue": name,
                "generation": row.generation,
                "attempt": attempt,
                "read": observed.as_ref().map(|(updated_at, _, labels)| json!({"updated_at": updated_at, "labels": labels})),
                "gave_up": gave_up,
            }}),
            &snapshot,
        );
        let world = crate::snapshot::decode(&snapshot)?;
        let node = tl3_kernel::NodeId(
            id.parse()
                .map_err(|_| crate::Error::Invalid("invalid request node"))?,
        );
        let open = world
            .nodes
            .get(node.0 as usize)
            .is_some_and(|n| !n.phase.is_closed());
        let delete = tl3_store::RowChange::Delete {
            table: crate::PRIORITY_RECONCILE.into(),
            id: id.into(),
        };
        if !open || settled.is_some() || gave_up {
            commit.rows.push(delete);
        } else {
            row.attempts = attempt;
            // 60 s, 2 min, 4 min, ...
            row.next_due_ms = now + QUIET_MS * (1 << (attempt - 1).min(16));
            commit
                .rows
                .push(crate::put(crate::PRIORITY_RECONCILE, id.into(), json!(row)));
        }
        if gave_up {
            eprintln!(
                "issue {name}: priority not settled after {MAX_REREADS} rereads; keeping the last applied priority"
            );
        }
        if open && let Some((updated_at, priority, _)) = settled {
            let edit = snapshot
                .rows
                .iter()
                .find(|r| r.table == crate::ISSUE_EDITS && r.id == id)
                .map(|r| crate::decode_stored::<crate::IssueEdit>(r.value.clone()))
                .transpose()?;
            if edit
                .as_ref()
                .is_none_or(|edit| *updated_at > edit.updated_at)
            {
                commit.rows.push(crate::put(
                    crate::ISSUE_EDITS,
                    id.into(),
                    json!(crate::IssueEdit {
                        updated_at: *updated_at,
                        reconciliations: edit.map_or(0, |edit| edit.reconciliations),
                    }),
                ));
            }
            if world.nodes[node.0 as usize].priority != *priority {
                self.apply(
                    &snapshot,
                    world,
                    tl3_kernel::Event::Fact {
                        node,
                        fact: tl3_kernel::Fact::Priority(*priority),
                    },
                    &mut commit,
                )?;
            }
        }
        Ok(commit)
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

    #[derive(Clone)]
    struct Clock(std::rc::Rc<std::cell::Cell<u64>>);
    impl crate::clock::Clock for Clock {
        fn now(&self) -> std::time::SystemTime {
            std::time::UNIX_EPOCH + std::time::Duration::from_millis(self.0.get())
        }
    }

    /// Seconds after this instant name issue times and the controller's clock.
    const EPOCH: i64 = 1_800_000_000;

    /// Tracked requests for issues 42.. at P1, from a label, on a simulated upstream.
    struct Intake {
        _repo: crate::git_tests::Repo,
        _db: crate::git_tests::support::Database,
        controller: crate::Controller,
        repo: tl3_config::Repository,
        bootstrap: crate::Bootstrap,
        fakes: std::path::PathBuf,
        clock: Clock,
    }

    fn intake(name: &str, issues: u64) -> Intake {
        use crate::git_tests::{Repo, support::Database};
        let f = Repo::new();
        let mut db = Database::start();
        let mut config =
            tl3_config::Config::parse(include_str!("../../config/example.toml")).unwrap();
        config.store = db.create(name);
        config.workers.runs = f.root.path().join("runs");
        let fakes = f.root.path().join("fakes");
        let clock = Clock(std::rc::Rc::new(std::cell::Cell::new(0)));
        let mut controller = crate::Controller::open_with_clock(
            tl3_store::Store::open(&config.store).unwrap(),
            crate::adapters::simulation(&fakes).unwrap(),
            crate::Bootstrap::from_config(&config).unwrap(),
            clock.clone(),
        )
        .unwrap();
        let repo = config.github.repos[0].clone();
        for number in 42..42 + issues {
            controller
                .intake(crate::LabelledIssue {
                    delivery: format!("labelled-{number}"),
                    owner: repo.owner.clone(),
                    repo: repo.name.clone(),
                    installation: repo.installation_id,
                    number,
                    title: "Fix the greeting".into(),
                    actor: config.github.rob.id,
                    label: "tl3".into(),
                    labels: vec!["tl3".into(), "P1".into()],
                })
                .unwrap();
        }
        Intake {
            _repo: f,
            _db: db,
            controller,
            repo,
            bootstrap: crate::Bootstrap::from_config(&config).unwrap(),
            fakes,
            clock,
        }
    }

    /// Issue `number` as GitHub has it `at` seconds in.
    fn issue_n(number: u64, at: i64, labels: &[&str]) -> tl3_github::Issue {
        tl3_github::Issue {
            number,
            title: "Fix the greeting".into(),
            body: None,
            state: "open".into(),
            state_reason: None,
            updated_at: chrono::DateTime::from_timestamp(EPOCH + at, 0).unwrap(),
            labels: labels
                .iter()
                .map(|name| tl3_github::Label {
                    name: name.to_string(),
                })
                .collect(),
            pull_request: None,
        }
    }

    fn issue(at: i64, labels: &[&str]) -> tl3_github::Issue {
        issue_n(42, at, labels)
    }

    fn priority_of(controller: &mut crate::Controller) -> Priority {
        controller.world().unwrap().nodes[0].priority
    }

    /// An edit of issue `number`'s labels at `at` seconds in.
    fn relabel(
        t: &Intake,
        delivery: &str,
        number: u64,
        at: i64,
        labels: &[&str],
    ) -> crate::IssueChange {
        crate::IssueChange {
            delivery: delivery.into(),
            owner: t.repo.owner.clone(),
            repo: t.repo.name.clone(),
            installation: t.repo.installation_id,
            number,
            title: "Fix the greeting".into(),
            labels: labels.iter().map(|label| label.to_string()).collect(),
            updated_at: issue(at, &[]).updated_at,
        }
    }

    /// Set the controller's clock to `at` seconds in.
    fn at(t: &Intake, at: i64) {
        t.clock.0.set(((EPOCH + at) * 1000) as u64);
    }

    /// In second `when`, P3 was removed from issue `number` and P0 added;
    /// `first` arrives first and applies, the other disagrees.
    fn conflict(t: &mut Intake, number: u64, when: i64, p0_first: bool, tag: &str) {
        let removed = relabel(t, &format!("remove-{tag}-{number}"), number, when, &["tl3"]);
        let added = relabel(
            t,
            &format!("add-{tag}-{number}"),
            number,
            when,
            &["tl3", "P0"],
        );
        let (first, second) = if p0_first {
            (added, removed)
        } else {
            (removed, added)
        };
        t.controller.issue_changed(&first).unwrap();
        t.controller.issue_changed(&second).unwrap();
    }

    fn pending(t: &mut Intake) -> Vec<crate::Reconcile> {
        t.controller
            .snapshot()
            .unwrap()
            .rows
            .into_iter()
            .filter(|row| row.table == "priority_reconcile")
            .map(|row| crate::decode_stored(row.value).unwrap())
            .collect()
    }

    /// The timer fires: the controller runs what is due now.
    fn tick(t: &mut Intake) {
        t.controller.look_due().unwrap();
    }

    #[test]
    fn a_failed_reread_is_retried_after_backoff_without_a_new_webhook() {
        let mut t = intake("priority_failed_reread", 1);
        at(&t, 75);
        conflict(&mut t, 42, 70, false, "a");
        assert_eq!(priority_of(&mut t.controller), Priority::Unranked);
        assert_eq!(
            pending(&mut t).len(),
            1,
            "the disagreement waits for a reread"
        );
        // GitHub can't be read yet: no issue in the fake.
        tick(&mut t);
        let row = &pending(&mut t)[0];
        assert_eq!(row.attempts, 1, "the failed read counts");
        let wait = t
            .controller
            .next_look_in()
            .unwrap()
            .expect("a reread is scheduled");
        assert!(wait.as_secs() >= 59 && wait.as_secs() <= 60, "{wait:?}");
        crate::adapters::github::set_fake_issue(&t.fakes, &issue(70, &["tl3", "P0"]));
        at(&t, 135);
        tick(&mut t);
        assert_eq!(
            priority_of(&mut t.controller),
            Priority::P0,
            "the retry applies P0"
        );
        assert!(pending(&mut t).is_empty());
    }

    #[test]
    fn a_same_second_reread_applies_only_after_the_quiet_period() {
        let mut t = intake("priority_quiet", 1);
        at(&t, 75);
        conflict(&mut t, 42, 70, true, "a");
        assert_eq!(priority_of(&mut t.controller), Priority::P0);
        // Within the quiet period, GitHub returns the intermediate snapshot.
        crate::adapters::github::set_fake_issue(&t.fakes, &issue(70, &["tl3"]));
        tick(&mut t);
        assert_eq!(
            priority_of(&mut t.controller),
            Priority::P0,
            "a same-second read inside the quiet period doesn't apply"
        );
        assert_eq!(pending(&mut t)[0].attempts, 1);
        crate::adapters::github::set_fake_issue(&t.fakes, &issue(70, &["tl3", "P0"]));
        at(&t, 135);
        tick(&mut t);
        assert_eq!(priority_of(&mut t.controller), Priority::P0);
        assert!(
            pending(&mut t).is_empty(),
            "a read after the quiet period applies"
        );
    }

    #[test]
    fn two_reconciliations_of_one_issue_do_not_collide() {
        let mut t = intake("priority_generations", 1);
        crate::adapters::github::set_fake_issue(&t.fakes, &issue(70, &["tl3", "P0"]));
        at(&t, 75);
        conflict(&mut t, 42, 70, false, "a");
        tick(&mut t);
        at(&t, 135);
        tick(&mut t);
        assert!(pending(&mut t).is_empty());
        assert_eq!(priority_of(&mut t.controller), Priority::P0);
        // A late same-second edit disagrees again, with GitHub unchanged.
        at(&t, 140);
        t.controller
            .issue_changed(&relabel(&t, "late-remove", 42, 70, &["tl3"]))
            .unwrap();
        let row = &pending(&mut t)[0];
        assert_eq!(row.generation, 2, "a second reconciliation of the issue");
        tick(&mut t);
        assert!(pending(&mut t).is_empty());
        assert_eq!(priority_of(&mut t.controller), Priority::P0);
    }

    #[test]
    fn rereads_cap_out_and_keep_the_last_applied_priority() {
        let mut t = intake("priority_cap", 1);
        at(&t, 75);
        conflict(&mut t, 42, 70, true, "a");
        let mut now = 75;
        for _ in 0..40 {
            if pending(&mut t).is_empty() {
                break;
            }
            tick(&mut t);
            now += t
                .controller
                .next_look_in()
                .unwrap()
                .map_or(0, |wait| wait.as_secs() as i64 + 1);
            at(&t, now);
        }
        assert!(pending(&mut t).is_empty(), "the row clears at the cap");
        assert_eq!(
            priority_of(&mut t.controller),
            Priority::P0,
            "the last applied priority stays"
        );
        let events = t
            .controller
            .store
            .events_after(tl3_store::Revision::ZERO, 1000)
            .unwrap();
        let reads: Vec<_> = events
            .iter()
            .filter(|event| event.key.starts_with("priority-read:"))
            .collect();
        assert_eq!(
            reads.len(),
            MAX_REREADS as usize,
            "exactly the capped rereads"
        );
        assert!(
            reads
                .last()
                .is_some_and(|event| event.payload["priority_reread"]["gave_up"] == true),
            "the last reread records giving up"
        );
    }

    #[test]
    fn rereads_rotate_fairly_beyond_the_per_tick_limit() {
        let issues = REREADS_PER_TICK as u64 + 4;
        let mut t = intake("priority_rotation", issues);
        at(&t, 75);
        for number in 42..42 + issues {
            conflict(&mut t, number, 70, true, "a");
        }
        assert_eq!(pending(&mut t).len(), issues as usize);
        tick(&mut t);
        let read = |t: &mut Intake| pending(t).iter().filter(|row| row.attempts > 0).count();
        assert_eq!(
            read(&mut t),
            REREADS_PER_TICK,
            "a tick rereads at most the limit"
        );
        tick(&mut t);
        assert_eq!(
            read(&mut t),
            issues as usize,
            "the next tick reaches the rest first"
        );
    }

    #[test]
    fn a_catch_up_replay_with_reordered_labels_is_a_no_op() {
        let mut t = intake("priority_reordered", 1);
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
        let (first, reordered) = (snapshot(&["tl3", "P2"]), snapshot(&["P2", "tl3"]));
        assert_eq!(
            first, reordered,
            "the key and the payload are both canonical"
        );
        assert_ne!(
            snapshot(&["tl3", "P3"]).delivery,
            first.delivery,
            "a different snapshot in the same second has its own key"
        );
        t.controller.issue_changed(&first).unwrap();
        let revision = t.controller.snapshot().unwrap().revision;
        t.controller.issue_changed(&reordered).unwrap();
        assert_eq!(t.controller.snapshot().unwrap().revision, revision);
        assert_eq!(priority_of(&mut t.controller), Priority::P2);
    }
}
