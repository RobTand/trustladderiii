//! Counterexamples that constrain the unbounded progress claim in #771.

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

/// State-table progress witness and Q27: this claim needs a counter headroom rule.
#[test]
fn settled_open_world_has_a_progress_witness_at_max_command_id() {
    let world = paused_unplanned(u64::MAX);
    let before = world.measure();
    let witness = std::panic::catch_unwind(|| world.witness());
    assert!(
        witness.is_ok(),
        "a consistent settled open world must construct its progress witness"
    );
    let event = witness.unwrap().expect("an open world has a witness");
    let after = send(world, event);
    assert!(
        after.measure() < before,
        "the progress witness must strictly lower the whole-world measure"
    );
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
