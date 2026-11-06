use tl3_mergify::decide::{heal as h, plan as p};

/// Preserved existing behaviour (no rule): old epochs refuse before execute reads or writes upstream state.
#[test]
fn mandatory_a2_fence() {
    for (incoming, current) in [(0, 1), (1, 1), (2, 1), (i64::MIN, i64::MAX)] {
        assert_eq!(p::fenced(incoming, current), incoming < current);
    }
}
/// #751/#760: the saved intent must precede the epoch fence and dispatch.
#[test]
fn mandatory_a2_write_ahead() {
    assert_eq!(p::intent(), (p::IntentEffect::Save, p::IntentEffect::Fence));
}
/// #447: closed or merged PRs never enter branch ownership/body healing.
#[test]
fn mandatory_a2_heal_scope() {
    for open in [false, true] {
        for merged in [false, true] {
            assert_eq!(h::available(open, merged), open && !merged);
        }
    }
}
/// Preserved existing behaviour (no rule): choose newest eligible run first, including a newer successful run.
#[test]
fn mandatory_a2_rerun_selection() {
    let run = |id, failed| h::Run {
        id,
        current_head: true,
        pull_request: true,
        listed: true,
        completed: true,
        failed,
    };
    assert_eq!(h::newest(&[]), None);
    assert_eq!(h::newest(&[run(1, true), run(2, false)]), Some(1));
    assert_eq!(h::newest(&[run(2, true), run(2, false)]), Some(0));
}

use super::*;
use io::{Entry, Frame, Profile, ReadOverride, Schedule};
fn schedule(profile: Profile) -> Schedule {
    Schedule {
        profile,
        initial: 0,
        frames: vec![],
        appears_at: 0,
        write_results: vec![],
        overrides: vec![],
        file_results: vec![],
    }
}
fn compare(s: &Schedule, entry: Entry, config: A2Config, heal: bool) -> Value {
    io::a2_compare(s, |new, d| {
        if new {
            a2_current::a2_run(entry, d, config, heal)
        } else {
            a2_reference::a2_run(entry, d, config, heal)
        }
    })
}
fn count(report: &Value, op: &str) -> usize {
    report["calls"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["op"] == op)
        .count()
}
fn file(report: &Value, suffix: &str) -> Option<Value> {
    report["files"]
        .as_object()
        .unwrap()
        .iter()
        .find(|(p, _)| p.ends_with(suffix))
        .map(|(_, v)| serde_json::from_str(v.as_str().unwrap()).unwrap())
}
fn over(s: &mut Schedule, op: &str, suffix: &str, nth: usize, word: u16) {
    s.overrides.push(ReadOverride {
        op: op.into(),
        suffix: suffix.into(),
        nth,
        frame: Frame {
            word,
            elapsed_ms: 0,
        },
    });
}
/// Preserved existing behaviour (no rule): fence refusal occurs before execute I/O.
#[test]
fn mandatory_a2_shell_fence_refusal() {
    let r = compare(
        &schedule(Profile::Plain),
        Entry::Execute,
        A2Config {
            epoch: 2,
            ..A2Config::default()
        },
        false,
    );
    assert_eq!(r["result"]["error"], "Mergify effect epoch is fenced");
    assert_eq!(r["calls"], json!([]));
}
/// #447 body self-heal; write/rerun ordering and pending retry preserve existing behaviour (no rule).
#[test]
fn mandatory_a2_shell_heal_edit_and_failed_rerun() {
    for failed in [false, true] {
        let mut s = schedule(Profile::Body);
        if failed {
            s.write_results = vec![0, 1];
        }
        let r = compare(&s, Entry::Read, A2Config::default(), false);
        assert_eq!(count(&r, "github.patch_body"), 1, "{r:#}");
        assert_eq!(count(&r, "github.rerun_failed"), 1, "{r:#}");
        assert_eq!(file(&r, ".body-heal").unwrap()["reran"], !failed);
        assert_eq!(r["result"]["facts"], json!([]));
    }
}
/// #447 open Publishing/Queued; branch and empty-list gates preserve existing behaviour (no rule).
#[test]
fn mandatory_a2_shell_heal_gates() {
    for config in [
        A2Config {
            no_policy: true,
            ..A2Config::default()
        },
        A2Config {
            no_branches: true,
            ..A2Config::default()
        },
        A2Config {
            foreign: true,
            ..A2Config::default()
        },
        A2Config {
            inactive: true,
            ..A2Config::default()
        },
    ] {
        let r = compare(&schedule(Profile::Body), Entry::Read, config, true);
        assert_eq!(r["result"]["edited"], false, "{r:#}");
        assert_eq!(count(&r, "github.patch_body"), 0);
        assert_eq!(count(&r, "github.runs"), 0);
    }
    let r = compare(
        &schedule(Profile::Body),
        Entry::Read,
        A2Config {
            empty_reruns: true,
            ..A2Config::default()
        },
        true,
    );
    assert_eq!(r["result"]["edited"], true, "{r:#}");
    assert_eq!(count(&r, "github.runs"), 0);
    assert_eq!(file(&r, ".body-heal").unwrap()["reran"], true);
}
/// Preserved existing behaviour (no rule): saved request skips issue I/O, missing request reads it.
#[test]
fn mandatory_a2_shell_request_sources() {
    for saved in [false, true] {
        let mut d = io::fixture_delivery();
        d.request_text = saved.then(|| "Change type: fix".into());
        io::with_inputs(
            io::A1Inputs {
                delivery: Some(d),
                ..Default::default()
            },
            || {
                let r = compare(
                    &schedule(Profile::Body),
                    Entry::Read,
                    A2Config::default(),
                    true,
                );
                assert_eq!(r["result"]["edited"], true, "{r:#}");
                assert_eq!(count(&r, "github.issue"), usize::from(!saved));
            },
        );
    }
}
/// #751/#760: dispatch and retry schedules compare real old/new shells at every durable write.
#[test]
fn mandatory_a2_shell_execute_branches() {
    for profile in [Profile::Plain, Profile::Command] {
        let r = compare(
            &schedule(profile),
            Entry::Execute,
            A2Config::default(),
            false,
        );
        assert_eq!(
            count(&r, "github.add_label"),
            usize::from(profile == Profile::Plain)
        );
        assert_eq!(
            count(&r, "github.command"),
            usize::from(profile == Profile::Command)
        );
        assert_eq!(file(&r, ".applied"), Some(json!(true)), "{r:#}");
        assert_eq!(
            r["result"]["error"],
            "enqueue request sent; Mergify acceptance is still unknown"
        );
        let calls = r["calls"].as_array().unwrap();
        let intent = calls
            .iter()
            .position(|c| {
                c["op"] == "file.rename" && c["args"].as_str().unwrap().ends_with(".intent")
            })
            .unwrap();
        let fence = calls
            .iter()
            .enumerate()
            .skip(intent + 1)
            .find(|(_, c)| {
                c["op"] == "file.read" && c["args"].as_str().unwrap().ends_with("epoch.fence")
            })
            .unwrap()
            .0;
        let effect = calls
            .iter()
            .position(|c| c["op"] == "github.add_label" || c["op"] == "github.command")
            .unwrap();
        let applied = calls
            .iter()
            .position(|c| {
                c["op"] == "file.rename" && c["args"].as_str().unwrap().ends_with(".applied")
            })
            .unwrap();
        assert!(intent < fence && fence < effect && effect < applied);
    }
    let pending = compare(
        &schedule(Profile::Flip),
        Entry::Execute,
        A2Config::default(),
        false,
    );
    assert_eq!(
        pending["result"]["error"],
        "Mergify acceptance is still unknown"
    );
    assert!(file(&pending, ".applied").is_none());
}

/// Preserved existing behaviour (no rule): completed and stale heals skip reruns; pending same-head resumes without an edit.
#[test]
fn mandatory_a2_shell_pending_heal() {
    for (same_head, reran) in [(true, false), (true, true), (false, false), (false, true)] {
        let r = compare(
            &schedule(Profile::Plain),
            Entry::Read,
            A2Config {
                saved_heal: Some((same_head, reran)),
                ..Default::default()
            },
            true,
        );
        assert_eq!(r["result"]["edited"], false, "{r:#}");
        assert_eq!(count(&r, "github.patch_body"), 0);
        assert_eq!(count(&r, "github.issue"), 0);
        assert_eq!(
            count(&r, "github.runs"),
            usize::from(same_head && !reran),
            "{r:#}"
        );
    }
}
/// Preserved existing behaviour (no rule): boundary grouping and conclusion parsing are compared with the pinned real helper.
#[test]
fn mandatory_a2_rerun_boundary_shapes() {
    use tl3_github::WorkflowRun;
    let run = |id, path: &str, head: &str, event: &str, status: &str, conclusion: Option<&str>| {
        WorkflowRun {
            id,
            check_suite_id: 1,
            head_sha: head.into(),
            run_attempt: 1,
            status: status.into(),
            conclusion: conclusion.map(str::to_owned),
            event: event.into(),
            path: path.into(),
        }
    };
    for conclusion in [
        None,
        Some("success"),
        Some("failure"),
        Some("cancelled"),
        Some("timed_out"),
        Some("startup_failure"),
        Some("skipped"),
        Some("neutral"),
    ] {
        let runs = vec![
            run(
                1,
                "a/gate.yml",
                "head",
                "pull_request",
                "completed",
                Some("failure"),
            ),
            run(
                3,
                "a/gate.yml",
                "head",
                "pull_request",
                "completed",
                conclusion,
            ),
            run(
                3,
                "a/gate.yml",
                "head",
                "pull_request",
                "completed",
                Some("failure"),
            ),
            run(
                9,
                "a/gate.yml",
                "other",
                "pull_request",
                "completed",
                Some("failure"),
            ),
            run(
                2,
                "b/gate.yml",
                "head",
                "pull_request",
                "completed",
                Some("failure"),
            ),
            run(
                4,
                "c/gate.yml",
                "head",
                "push",
                "completed",
                Some("failure"),
            ),
            run(
                4,
                "d/unlisted.yml",
                "head",
                "pull_request",
                "completed",
                Some("failure"),
            ),
            run(
                4,
                "e/gate.yml",
                "head",
                "pull_request",
                "in_progress",
                Some("failure"),
            ),
        ];
        let expected = if matches!(
            conclusion,
            Some("failure" | "cancelled" | "timed_out" | "startup_failure")
        ) {
            vec![3, 2]
        } else {
            vec![2]
        };
        for workflows in [vec![], vec!["gate.yml".into()]] {
            let old = a2_reference::a2_reruns(&runs, "head", &workflows);
            assert_eq!(old, a2_current::a2_reruns(&runs, "head", &workflows));
            assert_eq!(
                old,
                if workflows.is_empty() {
                    vec![]
                } else {
                    expected.clone()
                }
            );
        }
    }
    for phase in [
        None,
        Some(tl3_kernel::Phase::Publishing),
        Some(tl3_kernel::Phase::Queued),
        Some(tl3_kernel::Phase::Checking),
    ] {
        assert_eq!(a2_reference::a2_phase(phase), a2_current::a2_phase(phase));
    }
}

/// Preserved existing behaviour (no rule): Confirmed without local receipt is delivered, while an existing receipt is reused.
#[test]
fn mandatory_a2_shell_confirmed_receipts() {
    for missing in [false, true] {
        let mut s = schedule(Profile::Active);
        if missing {
            over(&mut s, "file.read", ".json", 3, 1);
        }
        let r = compare(&s, Entry::Execute, A2Config::default(), false);
        assert_eq!(r["result"]["completion"]["ack"], "Confirmed", "{r:#}");
        assert_eq!(file(&r, ".json").unwrap()["deliveries"], 1, "{r:#}");
        assert_eq!(count(&r, "github.add_label"), 0);
        assert_eq!(count(&r, "github.command"), 0);
        let writes = r["calls"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["op"] == "file.rename" && c["args"].as_str().unwrap().ends_with(".json"))
            .count();
        assert_eq!(writes, 1 + usize::from(missing));
    }
    let mut existing = schedule(Profile::Active);
    over(&mut existing, "file.read", ".json", 0, 2);
    let r = compare(&existing, Entry::Execute, A2Config::default(), false);
    assert_eq!(file(&r, ".json").unwrap()["deliveries"], 0);
    assert_eq!(count(&r, "file.rename"), 0);
    let mut s = schedule(Profile::Plain);
    over(&mut s, "github.checks", "", 2, 1024);
    let r = compare(&s, Entry::Execute, A2Config::default(), false);
    assert_eq!(r["result"]["completion"]["ack"], "Confirmed", "{r:#}");
    assert_eq!(file(&r, ".applied"), Some(json!(true)));
    assert_eq!(file(&r, ".json").unwrap()["deliveries"], 2, "{r:#}");
}
/// #760: changed final head re-runs completion after intent, before applied.
#[test]
fn mandatory_a2_shell_retryable_reread() {
    let mut s = schedule(Profile::Plain);
    over(&mut s, "github.pull", "", 4, 2);
    let r = compare(&s, Entry::Execute, A2Config::default(), false);
    assert_eq!(r["result"]["error"], "PR changed during enqueue", "{r:#}");
    assert!(file(&r, ".intent").is_some());
    assert!(file(&r, ".applied").is_none());
    assert_eq!(count(&r, "github.add_label"), 0);
    assert_eq!(count(&r, "github.checks"), 3, "{r:#}");
}
/// Preserved existing behaviour (no rule): a saved attempt skips the new-attempt event read.
#[test]
fn mandatory_a2_shell_saved_attempt() {
    let mut s = schedule(Profile::Plain);
    over(&mut s, "file.read", ".intent", 1, 14);
    let r = compare(&s, Entry::Execute, A2Config::default(), false);
    assert_eq!(count(&r, "github.add_label"), 1, "{r:#}");
    assert_eq!(file(&r, ".intent").unwrap()["last_event"], 1);
    let calls = r["calls"].as_array().unwrap();
    let intent = calls
        .iter()
        .position(|c| c["op"] == "file.rename" && c["args"].as_str().unwrap().ends_with(".intent"))
        .unwrap();
    assert!(!calls[..intent].iter().any(|c| c["op"] == "github.events"));
}
/// Preserved existing behaviour (no rule): malformed, denied and other file errors propagate; upstream errors retain their classes.
#[test]
fn mandatory_a2_shell_heal_error_shapes() {
    for word in [1024, 2048, 4096] {
        let mut s = schedule(Profile::Body);
        over(&mut s, "file.read", ".body-heal", 0, word);
        let r = compare(&s, Entry::Read, A2Config::default(), true);
        assert!(r["result"].get("error").is_some(), "{r:#}");
        assert_eq!(count(&r, "github.patch_body"), 0);
    }
    for (op, nth) in [("github.issue", 0), ("github.runs", 0)] {
        let mut s = schedule(Profile::Body);
        over(&mut s, op, "", nth, 256);
        let r = compare(&s, Entry::Read, A2Config::default(), true);
        assert!(r["result"].get("error").is_some(), "{r:#}");
    }
}

/// Preserved existing behaviour (no rule): fresh attempts retain Ready when the independently read issue gains the queue label.
#[test]
fn mandatory_a2_shell_new_attempt_ready() {
    let mut s = schedule(Profile::Command);
    over(&mut s, "file.read", ".intent", 1, 1);
    let r = compare(&s, Entry::Execute, A2Config::default(), false);
    assert_eq!(
        file(&r, ".intent").unwrap_or_else(|| panic!("{r:#}"))["requeue"],
        "Ready",
        "{r:#}"
    );
    assert_eq!(file(&r, ".intent").unwrap()["last_event"], 2);
    assert_eq!(count(&r, "github.command"), 1, "{r:#}");
    assert_eq!(count(&r, "github.add_label"), 0);
}

/// Preserved existing behaviour (no rule): initial/final non-confirmed completions do not deliver a receipt; Retryable can return Found.
#[test]
fn mandatory_a2_shell_nonconfirmed_receipts() {
    for final_read in [false, true] {
        let mut s = schedule(Profile::Plain);
        over(&mut s, "github.pull", "", if final_read { 5 } else { 0 }, 2);
        over(&mut s, "github.pull", "", if final_read { 6 } else { 1 }, 2);
        let r = compare(&s, Entry::Execute, A2Config::default(), false);
        assert_eq!(r["result"]["completion"]["ack"], "Retryable", "{r:#}");
        assert!(file(&r, ".json").is_none());
        assert_eq!(file(&r, ".applied").is_some(), final_read);
    }
    let mut s = schedule(Profile::Plain);
    over(&mut s, "github.pull", "", 4, 2);
    over(&mut s, "github.pull", "", 5, 2);
    over(&mut s, "github.pull", "", 6, 2);
    let r = compare(&s, Entry::Execute, A2Config::default(), false);
    assert_eq!(r["result"]["completion"]["ack"], "Retryable", "{r:#}");
    assert!(file(&r, ".applied").is_none());
    assert_eq!(count(&r, "github.checks"), 3);
    let mut refused = schedule(Profile::Command);
    refused.initial = 4;
    let r = compare(&refused, Entry::Execute, A2Config::default(), false);
    assert_eq!(r["result"]["completion"]["ack"], "Permanent", "{r:#}");
    assert!(file(&r, ".json").is_none());
    assert_eq!(count(&r, "github.command"), 0);
}
/// #447 open-PR self-heal: closed and merged reads skip the body-heal file and upstream edits.
#[test]
fn mandatory_a2_shell_closed_and_merged() {
    for word in [1, 4] {
        let mut s = schedule(Profile::Body);
        over(&mut s, "github.pull", "", 0, word);
        over(&mut s, "github.pull", "", 1, word);
        let r = compare(&s, Entry::Read, A2Config::default(), false);
        assert_eq!(count(&r, "github.patch_body"), 0);
        assert_eq!(count(&r, "github.runs"), 0);
        assert!(
            !r["calls"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["op"] == "file.read"
                    && c["args"].as_str().unwrap().ends_with(".body-heal"))
        );
    }
}
