use tl3_mergify::decide::{heal as h, plan as p};

/// #608/#751: old epochs refuse before execute reads or writes upstream state.
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
/// #447: choose newest eligible run first, including a newer successful run.
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
/// #447: edit writes pending before PATCH, reruns then marks complete; failures retain pending.
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
    for profile in [
        Profile::Plain,
        Profile::Label,
        Profile::Flip,
        Profile::Command,
        Profile::Active,
    ] {
        let r = compare(
            &schedule(profile),
            Entry::Execute,
            A2Config::default(),
            false,
        );
        println!("A2 execute {profile:?}: {r}");
    }
}
