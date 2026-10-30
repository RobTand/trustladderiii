//! Stands in for the shell: replays a noisy run, checks crash recovery and times the kernel.
use std::time::Instant;
use tl3_kernel_trial::{Event, INIT, Item, Outcome, Verdict, pending, run, step};

/// A late, noisy run. Tests go quiet, PrismaBuild loses and retries a lease, the first commit
/// fails, a duplicate and a stale result arrive, and the second commit merges.
const SCRIPT: [Event; 14] = [
    Event::Start,
    Event::AgentDone { attempt: 0, head: 7 },
    Event::TestQuiet { attempt: 0 },
    Event::TestLeaseLost { attempt: 0 },
    Event::TestQuiet { attempt: 0 },
    Event::TestResult { attempt: 0, head: 7, outcome: Outcome::Fail },
    Event::TestResult { attempt: 0, head: 7, outcome: Outcome::Fail },
    Event::AgentDone { attempt: 1, head: 8 },
    Event::TestResult { attempt: 0, head: 7, outcome: Outcome::Pass },
    Event::TestResult { attempt: 1, head: 8, outcome: Outcome::Pass },
    Event::Reviewed { attempt: 1, head: 8, verdict: Verdict::Approve },
    Event::Reviewed { attempt: 1, head: 8, verdict: Verdict::Approve },
    Event::MergeDone { head: 8 },
    Event::MergeDone { head: 8 },
];

fn main() {
    let mut item = INIT;
    let mut live: Vec<Item> = vec![item.clone()];
    for (i, &event) in SCRIPT.iter().enumerate() {
        let out = step(item, event);
        println!("{i:>2} {event:?} -> {:?} {:?}", out.item.phase, out.effect);
        item = out.item;
        live.push(item.clone());
    }
    println!("final {item:?}");

    // A crash after any event: the restart replays the log, then asserts what the item needs in
    // flight. It must land on the live run's item.
    let mut recovered = true;
    for k in 0..=SCRIPT.len() {
        let replayed = run(INIT, &SCRIPT[..k]);
        println!("crash after {k:>2}: restart asserts {:?}", pending(&replayed));
        if replayed != live[k] {
            recovered = false;
            println!("replay differs from the live run after event {k}");
        }
    }
    println!("replay after a crash at every point matches the live run: {recovered}");

    let rounds = 1_000_000;
    let start = Instant::now();
    let mut steps = 0u64;
    for _ in 0..rounds {
        let mut item = INIT;
        for &event in &SCRIPT {
            item = step(item, event).item;
            steps += 1;
        }
        std::hint::black_box(&item);
    }
    let seconds = start.elapsed().as_secs_f64();
    println!("{steps} steps in {seconds:.2}s, {:.0} million steps per second", steps as f64 / seconds / 1e6);
    std::process::exit(if recovered { 0 } else { 1 });
}
