use super::*;
use io::{Entry, Scenario, Schedule};
use proptest::{
    prelude::*,
    test_runner::{Config, FileFailurePersistence, RngSeed, TestRunner},
};
fn check(entry: Entry, scenario: Scenario) {
    let mut runner = TestRunner::new(Config {
        cases: 64,
        max_shrink_iters: 1024,
        rng_seed: RngSeed::Fixed(0x0769_0730_2026),
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/publication_io/proptest-regressions.txt"
        )))),
        ..Config::default()
    });
    let tape = prop_oneof![2=>Just(vec![]),3=>(0usize..48,any::<u16>()).prop_map(|(n,w)|{let mut v=vec![0;n];v.push(w);v}),3=>prop::collection::vec(any::<u16>(),0..64)];
    let strategy = (
        tape,
        prop::collection::vec(0u8..12, 0..12),
        prop::collection::vec(0u8..4, 0..32),
        0u8..3,
        0u8..3,
        0u8..3,
        proptest::option::of(0u8..5),
        0u8..4,
    )
        .prop_map(
            move |(
                reads,
                api_results,
                file_results,
                fence_epoch,
                owner_epoch,
                effect_epoch,
                initial_attempt,
                initial_comments,
            )| Schedule {
                scenario,
                reads,
                api_results,
                file_results,
                overrides: vec![],
                fence_epoch,
                owner_epoch,
                effect_epoch,
                initial_attempt,
                initial_comments,
            },
        );
    let comparisons = ::std::cell::Cell::new(0usize);
    runner
        .run(&strategy, |s| {
            comparisons
                .set(comparisons.get() + io::compare(&s, entry).map_err(TestCaseError::fail)?);
            Ok(())
        })
        .unwrap();
    eprintln!(
        "{entry:?}/{scenario:?}: 64 schedules, {} complete/crash comparisons",
        comparisons.get()
    );
}
#[test]
fn reconcile_schedules() {
    for s in [
        Scenario::Approval,
        Scenario::Duplicates,
        Scenario::OpenPr,
        Scenario::ClosePr,
        Scenario::Correction,
        Scenario::Restart,
        Scenario::Console,
        Scenario::Clarify,
        Scenario::Replacement,
        Scenario::Incident,
        Scenario::WrongIncident,
        Scenario::RestartDropped,
        Scenario::SummaryMerged,
        Scenario::SummaryDuplicate,
        Scenario::Plain,
        Scenario::EvidencePermanent,
    ] {
        check(Entry::Reconcile, s);
    }
}
#[test]
fn publication_schedules() {
    for s in [
        Scenario::Approval,
        Scenario::Duplicates,
        Scenario::OpenPr,
        Scenario::ClosePr,
        Scenario::EvidencePermanent,
    ] {
        check(Entry::Publish, s);
    }
}
#[test]
fn delivery_schedules() {
    for s in [
        Scenario::Approval,
        Scenario::Duplicates,
        Scenario::Correction,
        Scenario::Restart,
        Scenario::Console,
        Scenario::Clarify,
        Scenario::Replacement,
        Scenario::Incident,
        Scenario::WrongIncident,
        Scenario::RestartDropped,
        Scenario::SummaryMerged,
        Scenario::SummaryDuplicate,
        Scenario::Plain,
    ] {
        check(Entry::Execute, s);
    }
}
#[test]
fn repair_schedules() {
    check(Entry::Repair, Scenario::Approval);
    check(Entry::Repair, Scenario::Duplicates);
}
fn ops(r: &io::Run, op: &str) -> usize {
    r.calls.iter().filter(|c| c.op == op).count()
}
#[test]
fn mandatory_pr_evidence_permanence() {
    let s = Schedule::stable(Scenario::EvidencePermanent);
    io::compare(&s, Entry::Reconcile).unwrap();
    let r = io::observe(&s, Entry::Reconcile);
    assert_eq!(r.result["found"]["ack"], "Permanent");
    assert_eq!(r.result["found"]["failure_class"], "pr_evidence");
    assert_eq!(ops(&r, "github.branch_prs"), 0);
}
#[test]
fn mandatory_duplicate_convergence() {
    for e in [
        Entry::Reconcile,
        Entry::Publish,
        Entry::Execute,
        Entry::Repair,
    ] {
        let s = Schedule::stable(Scenario::Duplicates);
        io::compare(&s, e).unwrap();
        let r = io::observe(&s, e);
        assert_eq!(ops(&r, "github.patch_check"), 3);
        assert_eq!(ops(&r, "github.post_check"), 0);
        assert!(r.upstream.values().all(|v| v["conclusion"] == "success"));
    }
}
#[test]
fn mandatory_stale_epoch_fence_refusal() {
    for e in [Entry::Reconcile, Entry::Execute, Entry::Repair] {
        let mut s = Schedule::stable(Scenario::Duplicates);
        s.fence_epoch = 2;
        io::compare(&s, e).unwrap();
        let r = io::observe(&s, e);
        assert!(
            r.result["error"]
                .as_str()
                .unwrap()
                .contains("older controller epoch")
        );
        assert!(r.effects.is_empty());
        assert_eq!(ops(&r, "github.check_runs"), 0);
    }
}
#[test]
fn mandatory_lost_post_is_uncertain() {
    let mut s = Schedule::stable(Scenario::Approval);
    s.api_results = vec![5];
    io::compare(&s, Entry::Execute).unwrap();
    let r = io::observe(&s, Entry::Execute);
    assert_eq!(r.result["error"], "approval check dispatch outcome unknown");
    assert!(r.upstream.contains_key("check:200"));
    assert!(r.files.values().any(|v| v == b"\"Uncertain\""));
    assert!(!r.files.values().any(|v| v == b"\"Refused\""));
}
#[test]
fn mandatory_message_rejection_fallback() {
    let mut s = Schedule::stable(Scenario::Clarify);
    s.over("writer.draft", 0, 1);
    io::compare(&s, Entry::Execute).unwrap();
    let r = io::observe(&s, Entry::Execute);
    assert_eq!(r.result["completion"]["ack"], "Confirmed");
    assert_eq!(ops(&r, "github.post_comment"), 1);
    assert!(r.upstream.values().any(|v| {
        v["body"]
            .as_str()
            .is_some_and(|b| b.contains("I couldn't act on your comment. Could you rephrase it?"))
    }));
    let mut s = Schedule::stable(Scenario::Incident);
    s.over("writer.draft", 0, 1);
    io::compare(&s, Entry::Execute).unwrap();
    let r = io::observe(&s, Entry::Execute);
    assert_eq!(r.result["completion"]["failure_class"], "message_rejected");
    assert_eq!(ops(&r, "github.post_comment"), 0);
}
#[test]
fn mandatory_refusals_rate_limits_and_double_refused_write() {
    for mode in 1..12 {
        let mut s = Schedule::stable(Scenario::Approval);
        s.api_results = vec![mode];
        io::compare(&s, Entry::Execute).unwrap();
        let r = io::observe(&s, Entry::Execute);
        assert_eq!(ops(&r, "github.post_check"), 1);
        if mode == 9 {
            assert_eq!(
                r.calls
                    .iter()
                    .filter(|c| c.op == "file.write" && c.response.starts_with(b"\"Refused\""))
                    .count(),
                2
            );
        }
    }
}
#[test]
fn mandatory_close_and_comment_duplicates() {
    let mut s = Schedule::stable(Scenario::SummaryDuplicate);
    s.initial_comments = 3;
    io::compare(&s, Entry::Reconcile).unwrap();
    let r = io::observe(&s, Entry::Reconcile);
    assert_eq!(ops(&r, "github.delete_comment"), 2);
    assert_eq!(ops(&r, "github.issue"), 1);
    assert_eq!(ops(&r, "github.close_issue"), 1);
    assert_eq!(r.result["found"]["ack"], "Confirmed");
}
fn function<'a>(s: &'a str, name: &str, after: &str, indent: usize) -> &'a str {
    let s = &s[if after.is_empty() {
        0
    } else {
        s.find(after).unwrap()
    }..];
    let spaces = " ".repeat(indent);
    let at = s
        .lines()
        .scan(0, |offset, line| {
            let at = *offset;
            *offset += line.len() + 1;
            Some((at, line))
        })
        .find(|(_, line)| {
            line.starts_with(&spaces)
                && (line.contains(&format!("fn {name}(")) || line.contains(&format!("fn {name}<")))
        })
        .unwrap()
        .0;
    let end = s[at..].find(&format!("\n{spaces}}}\n")).unwrap() + indent + 3;
    &s[at..at + end]
}
#[test]
fn current_functions_are_production() {
    let prod = include_str!("../../src/adapters/github.rs");
    let copy = include_str!("current.rs");
    for name in [
        "body_lines",
        "semantic_paths",
        "fence",
        "client",
        "request_notice",
        "linked",
        "file_request",
        "console_note",
        "restart",
        "close_reason",
        "execute_delivery",
        "reconcile",
        "repair_approval",
        "reconcile_cancellation",
    ] {
        assert_eq!(
            function(
                prod,
                name,
                if ["reconcile", "repair_approval", "reconcile_cancellation"].contains(&name) {
                    "impl Executor for Adapter {"
                } else {
                    ""
                },
                4
            ),
            function(copy, name, "", 4),
            "production drift: {name}"
        );
    }
    for name in ["publication_comment", "publication_route"] {
        assert_eq!(function(prod, name, "", 4), function(copy, name, "", 4));
    }
    assert_eq!(
        include_str!("../../src/adapters/github/publishing.rs"),
        include_str!("current-publishing.rs")
    );
    assert!(include_str!("../../src/adapters/record.rs").starts_with(include_str!("records.rs")));
}
#[test]
fn references_are_frozen_bodies() {
    let base = include_str!("base-github.rs.txt");
    let copy = include_str!("reference.rs");
    for name in [
        "body_lines",
        "semantic_paths",
        "fence",
        "client",
        "request_notice",
        "linked",
        "file_request",
        "console_note",
        "restart",
        "close_reason",
        "execute_delivery",
        "reconcile",
        "repair_approval",
        "reconcile_cancellation",
    ] {
        assert_eq!(
            function(
                base,
                name,
                if ["reconcile", "repair_approval", "reconcile_cancellation"].contains(&name) {
                    "impl Executor for Adapter {"
                } else {
                    ""
                },
                4
            ),
            function(copy, name, "", 4),
            "reference drift: {name}"
        );
    }
    assert_eq!(
        include_str!("base-publishing.rs.txt"),
        include_str!("reference-publishing.rs")
    );
    assert!(include_str!("base-records.rs.txt").starts_with(include_str!("records.rs")));
}
#[test]
fn shared_helpers_are_production() {
    assert!(
        include_str!("../../src/adapters/record.rs").starts_with(include_str!("records.rs")),
        "atomic record writer drift"
    );
    for (source, copy, names, indent) in [
        (
            include_str!("../../src/adapters/github.rs"),
            include_str!("message.rs"),
            vec!["question_draft", "fixed_notice_text"],
            0,
        ),
        (
            include_str!("../../src/evidence.rs"),
            include_str!("evidence.rs"),
            vec![
                "publication_goals",
                "required_publication_lines",
                "publication_lines",
                "validate_publication",
                "validate_body",
            ],
            0,
        ),
        (
            include_str!("../../src/goals.rs"),
            include_str!("goals.rs"),
            vec!["branch_satisfies"],
            0,
        ),
        (
            include_str!("../../../github/src/client.rs"),
            include_str!("comments.rs"),
            vec![
                "owns_comment",
                "post_comment",
                "post_formatted_comment",
                "reconcile_comments",
                "delete_comment",
            ],
            4,
        ),
        (
            include_str!("../../../github/src/client.rs"),
            include_str!("comments.rs"),
            vec!["comment_marker", "marked_body", "repo_path", "segment"],
            0,
        ),
    ] {
        for n in names {
            assert_eq!(
                function(source, n, "", indent),
                function(copy, n, "", indent),
                "shared helper drift: {n}"
            );
        }
    }
}
#[test]
fn frozen_sha256_pins() {
    assert_eq!(
        format!("{:x}", Sha256::digest(include_bytes!("base-github.rs.txt"))),
        "e44f664e6ab38a0052775b92119fa78b26ac48800bf216c56cac434cc864a545"
    );
    assert_eq!(
        format!(
            "{:x}",
            Sha256::digest(include_bytes!("base-publishing.rs.txt"))
        ),
        "9c6e402efa02c13162c40b41f87a5ec3c79c4a8e189d5d83e59633caf0dbc93e"
    );
    assert_eq!(
        format!(
            "{:x}",
            Sha256::digest(include_bytes!("base-records.rs.txt"))
        ),
        "6c18ed51bcf362d32a342645c6c830b7c0dd2536b56033b6738dea1211d2aaa4"
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(include_bytes!("base-client.rs.txt"))),
        "5bde5cbc901f35a409b55028cbfb073d86b68ba3179a80abedd57eb50b8dfbcd"
    );
}

/// #769/#766: maintenance fences, duplicate convergence and attempt states are unchanged.
#[test]
fn protected_approval_helpers_are_unchanged() {
    for (old, new, names) in [
        (
            include_str!("base-github.rs.txt"),
            include_str!("../../src/adapters/github.rs"),
            vec![
                "fence",
                "repair_approval",
                "reconcile_cancellation",
                "retire",
                "retire_delivery",
            ],
        ),
        (
            include_str!("base-publishing.rs.txt"),
            include_str!("../../src/adapters/github/publishing.rs"),
            vec![
                "retire_approval",
                "converge_approvals",
                "repair_approval_copies",
                "approval_was_refused",
            ],
        ),
    ] {
        for name in names {
            let after = if [
                "repair_approval",
                "reconcile_cancellation",
                "retire",
                "retire_delivery",
            ]
            .contains(&name)
            {
                "impl Executor for Adapter {"
            } else {
                ""
            };
            assert_eq!(
                function(old, name, after, 4),
                function(new, name, after, 4),
                "protected helper changed: {name}"
            );
        }
    }
    fn attempt(s: &str) -> &str {
        let at = s.find("enum ApprovalAttempt {").unwrap();
        &s[at..at + s[at..].find("\n}\n").unwrap() + 3]
    }
    assert_eq!(
        attempt(include_str!("base-publishing.rs.txt")),
        attempt(include_str!("../../src/adapters/github/publishing.rs"))
    );
}
