use tl3_kernel::*;

fn confirm(w: &mut World) {
    for _ in 0..32 {
        let Some(effect) = w.nodes[0].effect else {
            return;
        };
        *w = step(
            w.clone(),
            Event::Ack {
                node: NodeId(0),
                seq: effect.seq,
                ack: Ack::Confirmed,
            },
        )
        .0;
    }
    panic!("fixture did not confirm effects");
}

fn planning(fail_fast: bool) -> World {
    let mut w = World::new(&[1], 1);
    w.fail_fast = fail_fast;
    w = step(
        w,
        Event::NewRequest {
            repo: RepoId(0),
            priority: Priority::Unranked,
        },
    )
    .0;
    confirm(&mut w);
    let generation = w.nodes[0].generation;
    w = step(
        w,
        Event::Reply {
            node: NodeId(0),
            generation,
            reply: Reply::IngestDone { size: 2 },
        },
    )
    .0;
    confirm(&mut w);
    let generation = w.nodes[0].generation;
    w = step(
        w,
        Event::Reply {
            node: NodeId(0),
            generation,
            reply: Reply::AuditAgrees,
        },
    )
    .0;
    confirm(&mut w);
    w.check().unwrap();
    assert!(matches!(w.nodes[0].phase, Phase::Planning { .. }));
    w
}

#[test]
fn actual_entry_first_repeat_stale_wrong_key_and_new_version() {
    for fail_fast in [false, true] {
        let w = planning(fail_fast);
        let generation = w.nodes[0].generation;
        for (request, budget, delivered) in [
            (NodeId(0), ReplanBudget::new(NodeId(0)), generation - 1),
            (NodeId(0), ReplanBudget::new(NodeId(1)), generation),
            (NodeId(99), ReplanBudget::new(NodeId(99)), generation),
        ] {
            let (same, budget, out) = step_planner(
                w.clone(),
                budget,
                request,
                delivered,
                PlannerResult::EmptyAllowedReplan,
            );
            assert_eq!(same, w);
            assert!(!budget.used());
            assert!(out.is_empty());
        }
        let (mut first, budget, out) = step_planner(
            w.clone(),
            ReplanBudget::new(NodeId(0)),
            NodeId(0),
            generation,
            PlannerResult::EmptyAllowedReplan,
        );
        assert!(budget.used());
        assert_eq!(first.nodes[0].phase, w.nodes[0].phase);
        assert_eq!(first.nodes[0].generation, generation + 1);
        assert_eq!(first.nodes[0].credit, w.nodes[0].credit);
        assert_eq!(first.nodes[0].plan_retries, w.nodes[0].plan_retries);
        assert_eq!(first.nodes[0].replanned, w.nodes[0].replanned);
        assert_eq!(first.next_question, w.next_question);
        assert!(out.iter().any(|out| matches!(
            out,
            Out::Effect {
                kind: EffectKind::StartRun(Role::Plan),
                ..
            }
        )));
        let (same, same_budget, out) = step_planner(
            first.clone(),
            budget,
            NodeId(0),
            generation,
            PlannerResult::EmptyAllowedReplan,
        );
        assert_eq!(same, first);
        assert_eq!(same_budget, budget);
        assert!(out.is_empty());
        confirm(&mut first);
        let generation = first.nodes[0].generation;
        let (stopped, budget, out) = step_planner(
            first,
            budget,
            NodeId(0),
            generation,
            PlannerResult::EmptyAllowedReplan,
        );
        assert!(budget.used());
        assert!(matches!(
            stopped.nodes[0].phase,
            Phase::Stopping(_) | Phase::Closed(_)
        ));
        assert_eq!(stopped.nodes[0].review_stop, Some(ReviewStop::PlanCantMeet));
        assert_eq!(stopped.nodes[0].credit, w.nodes[0].credit);
        assert_eq!(stopped.nodes[0].plan_retries, w.nodes[0].plan_retries);
        assert_eq!(stopped.nodes[0].replanned, w.nodes[0].replanned);
        assert!(stopped.nodes[0].question.is_none());
        assert!(!out.iter().any(|out| matches!(
            out,
            Out::Notice {
                notice: Notice::Ask { .. },
                ..
            }
        )));
        let fresh = ReplanBudget::new(NodeId(1));
        assert!(!fresh.used());
        assert!(fresh.bound_to(NodeId(1)) && budget.bound_to(NodeId(0)));
    }
}

#[test]
fn ordinary_entry_preserves_acceptance_and_stale_settling() {
    for fail_fast in [false, true] {
        let mut w = planning(fail_fast);
        let footprint = w.intern_footprint([FileId(0)]);
        let generation = w.nodes[0].generation;
        for reply in [
            Reply::PlanCantMeet,
            Reply::PlanSplit,
            Reply::PlanLeaf {
                footprint,
                fits: false,
            },
            Reply::PlanLeaf {
                footprint: Footprint::default(),
                fits: true,
            },
        ] {
            let (stopped, budget, _) = step_planner(
                w.clone(),
                ReplanBudget::import(NodeId(0), true),
                NodeId(0),
                generation,
                PlannerResult::Ordinary(reply),
            );
            assert!(budget.used());
            assert_eq!(stopped.nodes[0].review_stop, Some(ReviewStop::PlanCantMeet));
            assert!(matches!(
                stopped.nodes[0].phase,
                Phase::Stopping(_) | Phase::Closed(_)
            ));
            assert_eq!(stopped.nodes[0].credit, w.nodes[0].credit);
        }
        for reply in [
            Reply::ProtectedPaths,
            Reply::RunFailed,
            Reply::AuditAgrees,
            Reply::PlanLeaf {
                footprint,
                fits: true,
            },
            Reply::PlanLeaf {
                footprint: World::new(&[1], 1).intern_footprint([FileId(1000)]),
                fits: true,
            },
        ] {
            let expected = step(
                w.clone(),
                Event::Reply {
                    node: NodeId(0),
                    generation,
                    reply,
                },
            );
            let (actual, budget, out) = step_planner(
                w.clone(),
                ReplanBudget::import(NodeId(0), true),
                NodeId(0),
                generation,
                PlannerResult::Ordinary(reply),
            );
            assert_eq!((actual, out), expected, "{reply:?}");
            assert!(budget.used());
        }
        let expected = step(
            w.clone(),
            Event::Reply {
                node: NodeId(0),
                generation: generation - 1,
                reply: Reply::PlanCantMeet,
            },
        );
        let (actual, budget, out) = step_planner(
            w,
            ReplanBudget::new(NodeId(0)),
            NodeId(0),
            generation - 1,
            PlannerResult::Ordinary(Reply::PlanCantMeet),
        );
        assert_eq!((actual, out), expected);
        assert!(!budget.used());
    }
}
