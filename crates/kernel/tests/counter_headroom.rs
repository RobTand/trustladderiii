use tl3_kernel::*;

fn fixture() -> World {
    let mut world = World::new(&[1], 1);
    world.nodes.push(Node::new(RepoId(0)));
    world
}

/// #771: each independently exhausted machine counter closes the theorem scope.
#[test]
fn each_counter_has_its_own_headroom_check() {
    type CounterCase = (&'static str, fn(&mut World));
    let cases: [CounterCase; 8] = [
        ("generation", |w| w.nodes[0].generation = u64::MAX),
        ("next_seq", |w| w.nodes[0].next_seq = u64::MAX),
        ("next_ticket", |w| w.next_ticket = u64::MAX),
        ("next_question", |w| w.next_question = u64::MAX),
        ("credit.start", |w| w.nodes[0].credit.start = u32::MAX),
        ("credit.earned", |w| w.nodes[0].credit.earned = u32::MAX),
        ("credit.added", |w| w.nodes[0].credit.added = u32::MAX),
        ("credit.spent", |w| w.nodes[0].credit.spent = u32::MAX),
    ];
    assert!(fixture().headroom_ok());
    for (counter, exhaust) in cases {
        let mut world = fixture();
        exhaust(&mut world);
        assert!(!world.headroom_ok(), "{counter} must be checked");
        assert_eq!(world.counter_exhaustion().unwrap().counter, counter);
    }
}

/// #771: global ticket reservations account for arbitrary node counts.
#[test]
fn ticket_budget_scales_with_the_world() {
    let mut world = fixture();
    assert_eq!(world.ticket_step_budget(), Some(4));
    world.nodes.extend([Node::new(RepoId(0)); 100]);
    assert_eq!(world.ticket_step_budget(), Some(204));
    world.next_ticket = u64::MAX - 203;
    assert!(!world.headroom_ok());
}

/// #771: checked allocation includes the final fresh ID and then exhausts.
#[test]
fn checked_command_allocation_has_no_wrapped_id() {
    let mut n = Node::new(RepoId(0));
    assert_eq!(n.next_command_id(), Some(1));
    n.last_cmd = u64::MAX - 1;
    assert_eq!(n.next_command_id(), Some(u64::MAX));
    n.last_cmd = u64::MAX;
    assert_eq!(n.next_command_id(), None);
}
