use tl3_controller::Bootstrap;
use tl3_kernel::*;

struct GateRun(World);
impl GateRun {
    fn configured(w: u32) -> Self {
        let config = tl3_config::Config::parse(
            &include_str!("../../config/example.toml")
                .replace("w = 1", &format!("w = {w}"))
                .replace("agent_pool = 1", "agent_pool = 8"),
        )
        .expect("Q29 amended permits W greater than one");
        let bootstrap = Bootstrap::from_config(&config).unwrap();
        Self(World::new(&[bootstrap.slots], bootstrap.pool))
    }
    fn send(&mut self, event: Event) {
        self.0 = step(self.0.clone(), event).0;
        assert!(self.0.valid(), "{:?}", self.0.check());
        assert!(self.0.is_settled());
    }
    fn confirm(&mut self, node: NodeId) {
        for _ in 0..30 {
            let Some(e) = self.0.nodes[node.0 as usize].effect else {
                return;
            };
            self.send(Event::Ack {
                node,
                seq: e.seq,
                ack: Ack::Confirmed,
            });
        }
        panic!("effects did not settle");
    }
    fn reply(&mut self, node: NodeId, reply: Reply) {
        self.confirm(node);
        self.send(Event::Reply {
            node,
            generation: self.0.nodes[node.0 as usize].generation,
            reply,
        });
        self.confirm(node);
    }
    fn request(&mut self) -> NodeId {
        let node = NodeId(self.0.nodes.len() as u64);
        self.send(Event::NewRequest { repo: RepoId(0) });
        self.reply(node, Reply::IngestDone { size: 1 });
        self.reply(node, Reply::AuditAgrees);
        node
    }
    fn plan(&mut self, node: NodeId, file: u64) {
        let footprint = self.0.intern_footprint([FileId(file)]);
        self.reply(
            node,
            Reply::PlanLeaf {
                footprint,
                fits: true,
            },
        );
    }
    fn stop(&mut self, node: NodeId) {
        self.send(Event::Command {
            node,
            id: 1,
            command: Command::Stop,
        });
        self.confirm(node);
        assert_eq!(
            self.0.nodes[node.0 as usize].phase,
            Phase::Closed(Outcome::Stopped)
        );
    }
}

#[test]
fn w3_three_disjoint_leaves_run_concurrently_and_fourth_waits() {
    let mut r = GateRun::configured(3);
    for file in [0, 64, 128, 192] {
        let node = r.request();
        r.plan(node, file);
    }
    assert_eq!(r.0.nodes.iter().filter(|n| n.holds).count(), 3);
    assert!(
        r.0.nodes[..3]
            .iter()
            .all(|n| n.phase == Phase::Making && n.pool)
    );
    assert_eq!(r.0.nodes[3].phase, Phase::Ready(Resume::Making));
    assert!(!r.0.nodes[3].holds && !r.0.nodes[3].pool);
    r.stop(NodeId(0));
    assert_eq!(r.0.nodes[3].phase, Phase::Making);
    assert!(r.0.nodes[3].holds);
}

#[test]
fn w3_overlapping_leaves_serialize_without_holding_slots_or_files() {
    let mut r = GateRun::configured(3);
    for _ in 0..3 {
        let node = r.request();
        r.plan(node, 0);
    }
    let second = r.0.nodes[1].ticket.unwrap();
    let third = r.0.nodes[2].ticket.unwrap();
    assert!(second < third);
    assert_eq!(r.0.nodes.iter().filter(|n| n.holds).count(), 1);
    assert!(r.0.nodes[1..].iter().all(|n| !n.holds && !n.pool));
    r.stop(NodeId(0));
    assert!(r.0.nodes[1].holds && !r.0.nodes[2].holds);
    assert_eq!(
        r.0.nodes[2].ticket,
        Some(third),
        "waiting preserves the place in line"
    );
    r.stop(NodeId(1));
    assert!(r.0.nodes[2].holds);
}

#[test]
fn w3_planning_admission_stops_at_three_waiters_and_resumes_below_three() {
    let mut r = GateRun::configured(3);
    for _ in 0..4 {
        let node = r.request();
        assert_eq!(
            r.0.nodes[node.0 as usize].phase,
            Phase::Planning { must_split: false }
        );
        r.plan(node, 0);
    }
    assert_eq!(
        r.0.nodes
            .iter()
            .filter(|n| n.ticket.is_some() && matches!(n.phase, Phase::Ready(_)))
            .count(),
        3
    );
    let next = r.request();
    assert_eq!(r.0.nodes[next.0 as usize].phase, Phase::Unplanned);
    assert!(!r.0.nodes[next.0 as usize].holds);
    r.stop(NodeId(0));
    assert_eq!(
        r.0.nodes[next.0 as usize].phase,
        Phase::Planning { must_split: false }
    );
}

#[test]
fn w2_gate_uses_both_slots_and_preserves_waiters() {
    let mut r = GateRun::configured(2);
    for file in 0..3 {
        let node = r.request();
        r.plan(node, file);
    }
    assert_eq!(r.0.nodes.iter().filter(|n| n.holds).count(), 2);
    let ticket = r.0.nodes[2].ticket.unwrap();
    r.send(Event::NewRequest { repo: RepoId(0) });
    assert_eq!(r.0.nodes[2].ticket, Some(ticket));
    r.stop(NodeId(0));
    assert!(r.0.nodes[2].holds);
}

#[test]
fn q48_file_blocked_waiter_allows_disjoint_but_reserves_overlapping_files() {
    let mut r = GateRun(World::new(&[3], 8));
    let holder = r.request();
    r.plan(holder, 0);
    let waiter = r.request();
    let footprint = r.0.intern_footprint([FileId(0), FileId(1)]);
    r.reply(
        waiter,
        Reply::PlanLeaf {
            footprint,
            fits: true,
        },
    );
    let place = r.0.nodes[waiter.0 as usize].ticket;
    let disjoint = r.request();
    r.plan(disjoint, 2);
    assert!(
        r.0.nodes[disjoint.0 as usize].holds,
        "Q48 lets a truly disjoint leaf use a spare slot"
    );
    let overlapping = r.request();
    r.plan(overlapping, 1);
    assert!(
        !r.0.nodes[overlapping.0 as usize].holds,
        "a younger leaf cannot take an older waiter's free file"
    );
    assert_eq!(r.0.nodes[waiter.0 as usize].ticket, place);
    assert!(!r.0.nodes[waiter.0 as usize].holds);
    r.stop(holder);
    assert!(
        r.0.nodes[waiter.0 as usize].holds,
        "the older waiter gets the released slot"
    );
    assert!(!r.0.nodes[overlapping.0 as usize].holds);
    r.stop(waiter);
    assert!(r.0.nodes[overlapping.0 as usize].holds);
}

#[test]
fn q48_repeated_disjoint_arrivals_cannot_delay_the_older_waiter_after_release() {
    for width in [2, 3] {
        let mut r = GateRun(World::new(&[width], 8));
        let holder = r.request();
        r.plan(holder, 0);
        let waiter = r.request();
        r.plan(waiter, 0);
        let place = r.0.nodes[waiter.0 as usize].ticket;
        let mut last = None;
        for _ in 0..8 {
            if let Some(last) = last {
                r.stop(last);
            }
            let disjoint = r.request();
            r.plan(disjoint, 1);
            assert!(
                r.0.nodes[disjoint.0 as usize].holds,
                "Q48 permits disjoint arrivals while a file is blocked"
            );
            assert_eq!(r.0.nodes[waiter.0 as usize].ticket, place);
            last = Some(disjoint);
        }
        r.stop(holder);
        assert!(
            r.0.nodes[waiter.0 as usize].holds,
            "the older leaf progresses while the younger leaf stays in flight"
        );
        assert!(r.0.nodes[last.unwrap().0 as usize].holds);
    }
}
