//! State-table "Counter headroom": unrestricted kernel counterexamples outside
//! the controller's enforced scope. Unbounded progress is tracked in #805.

use tl3_kernel::*;

fn send(world: World, event: Event) -> World {
    let (world, _) = step(world, event);
    assert!(world.valid(), "invalid world after {event:?}: {world:#?}");
    world
}

fn confirm(world: World) -> World {
    let effect = world.nodes[0].effect.expect("an operation start effect");
    send(
        world,
        Event::Ack {
            node: NodeId(0),
            seq: effect.seq,
            ack: Ack::Confirmed,
        },
    )
}

fn paused_unplanned(last_cmd: u64) -> World {
    let world = send(
        World::new(&[1], 1),
        Event::NewRequest {
            repo: RepoId(0),
            priority: Priority::Unranked,
        },
    );
    let world = confirm(world);
    let generation = world.nodes[0].generation;
    let world = send(
        world,
        Event::Reply {
            node: NodeId(0),
            generation,
            reply: Reply::IngestDone { size: 1 },
        },
    );
    let world = confirm(world);
    let world = send(
        world,
        Event::Command {
            node: NodeId(0),
            id: last_cmd,
            command: Command::Pause,
        },
    );
    let generation = world.nodes[0].generation;
    let world = send(
        world,
        Event::Reply {
            node: NodeId(0),
            generation,
            reply: Reply::AuditAgrees,
        },
    );
    assert_eq!(world.nodes[0].phase, Phase::Unplanned);
    assert!(world.nodes[0].paused);
    assert!(world.nodes[0].effect.is_none());
    assert!(world.nodes[0].run.is_none());
    assert!(world.nodes[0].question.is_none());
    assert!(!world.nodes[0].waiting_incident);
    assert!(!world.nodes[0].had_pr);
    assert!(world.dependencies().is_empty());
    world
}

/// State-table "Counter headroom": exhausted IDs refute unrestricted progress.
#[test]
fn max_command_id_refutes_the_unrestricted_progress_witness() {
    let world = paused_unplanned(u64::MAX);
    assert_eq!(world.measure(), 28 * 64 + 2 + 8);
    assert_eq!(world.witness(), None, "outside the fresh-command scope");
    let after = send(
        world.clone(),
        Event::Command {
            node: NodeId(0),
            id: 0,
            command: Command::Resume,
        },
    );
    assert_eq!(after, world, "the kernel still ignores stale commands");
}

/// State-table "Counter headroom": even Stop cannot bypass an exhausted ID.
#[test]
fn exhausted_command_ids_ignore_resume_stop_and_replacement() {
    let world = paused_unplanned(u64::MAX);
    for id in [0, u64::MAX - 1, u64::MAX] {
        for command in [Command::Resume, Command::Stop, Command::ChangeRequest] {
            let (after, out) = step(
                world.clone(),
                Event::Command {
                    node: NodeId(0),
                    id,
                    command,
                },
            );
            assert_eq!(after, world);
            assert!(out.is_empty());
        }
    }
}

/// State-table progress witness: one remaining command ID is enough to resume.
#[test]
fn penultimate_command_id_still_has_a_progress_witness() {
    let world = paused_unplanned(u64::MAX - 1);
    let before = world.measure();
    let witness = world.witness().expect("an open world has a witness");
    assert!(matches!(
        witness,
        Event::Command {
            id: u64::MAX,
            command: Command::Resume,
            ..
        }
    ));
    let after = send(world, witness);
    assert!(after.measure() < before);
}

/// State-table "Counter headroom": an exhausted node cannot supply a fresh ID.
#[test]
fn exhausted_command_witness_returns_none_without_overflow() {
    assert_eq!(paused_unplanned(u64::MAX).witness(), None);
}

/// Q30/Q39 / #805: pin first-node and first-branch order when several inputs match.
#[test]
fn witness_uses_the_first_node_and_first_available_branch() {
    let mut world = World::new(&[2], 2);
    for _ in 0..2 {
        world = send(
            world,
            Event::NewRequest {
                repo: RepoId(0),
                priority: Priority::Unranked,
            },
        );
    }
    let seq = world.nodes[0].effect.unwrap().seq;
    world.nodes[0].effect.as_mut().unwrap().cancelling = true;
    world.nodes[0].waiting_incident = true;
    world.nodes[0].paused = true;
    assert!(world.valid());
    // Node 1 also has a confirmable effect. Node 0 simultaneously offers resume,
    // cancellation acknowledgement, incident clearing, and its Await item.
    assert_eq!(
        world.witness(),
        Some(Event::Command {
            node: NodeId(0),
            id: 1,
            command: Command::Resume,
        })
    );
    world.nodes[0].paused = false;
    assert!(world.valid());
    assert_eq!(
        world.witness(),
        Some(Event::Ack {
            node: NodeId(0),
            seq,
            ack: Ack::Confirmed
        })
    );
    world.nodes[0].effect = None;
    assert!(world.valid());
    assert_eq!(
        world.witness(),
        Some(Event::Fact {
            node: NodeId(0),
            fact: Fact::IncidentCleared { by_rob: true },
        })
    );
    world.nodes[0].waiting_incident = false;
    assert_eq!(
        world.witness(),
        Some(Event::Reply {
            node: NodeId(0),
            generation: world.nodes[0].generation,
            reply: Reply::IngestDone { size: 1 },
        })
    );
}
