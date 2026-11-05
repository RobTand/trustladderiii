use tl3_controller_core::translate::*;

/// Q52/#447, Q58; docs/state-table.md:542-545,574-575,667 and
/// docs/state-machine.md:789-807 on cdacce8f.
#[test]
fn production_decision_witnesses() {
    let matching = PriorFailure {
        same_repository: true,
        same_number: true,
        same_head: true,
        infra: true,
        overlapping_test: true,
    };
    assert!(repeated(&[matching]));
    assert!(!repeated(&[]));
    for field in 0..5 {
        let mut row = matching;
        match field {
            0 => row.same_repository = false,
            1 => row.same_number = false,
            2 => row.same_head = false,
            3 => row.infra = false,
            _ => row.overlapping_test = false,
        }
        assert!(!repeated(&[row]));
        assert!(repeated(&[row, matching]));
    }
    for (kind, action) in [
        (EvidenceKind::Setup, IncidentAction::Setup),
        (EvidenceKind::Missing, IncidentAction::Missing),
        (EvidenceKind::Blocked, IncidentAction::Blocked),
        (EvidenceKind::Reported, IncidentAction::Clear),
    ] {
        assert_eq!(
            dispatch(begin(false).unwrap(), Route::Incident, kind),
            Decision::Incident(action)
        );
    }
    assert_eq!(
        clear_reported(&[
            IncidentRow {
                own_incident: false,
                open: true
            },
            IncidentRow {
                own_incident: true,
                open: false
            },
            IncidentRow {
                own_incident: true,
                open: true
            }
        ]),
        Some(2)
    );
    assert_eq!(
        previous_commit(
            &[
                CommitRow {
                    is_commit: false,
                    id: 1
                },
                CommitRow {
                    is_commit: true,
                    id: 1
                }
            ],
            Some(1)
        ),
        Some(1)
    );
    let plan = vec!["arbitrary plan payload", "including selected failures"];
    let carried = carry_plan(7, "new head", Some(plan.clone())).unwrap();
    assert_eq!(carried.plan, plan);
    assert_eq!(carried.head, "new head");
    assert_eq!(carried.node, 7);
    assert!(head_current(1, Some(1)));
    assert!(!head_current(1, None));
    assert!(enqueue_pending(true, false));
    assert!(!enqueue_pending(true, true));
}
