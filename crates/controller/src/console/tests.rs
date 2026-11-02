use super::*;
use tl3_kernel::QKind;

const KINDS: [QKind; 6] = [
    QKind::AuditDisagreement,
    QKind::NeedsSplitting,
    QKind::CantMeet,
    QKind::OutOfCredit,
    QKind::EffectFailed,
    QKind::CleanupFailed,
];

fn question(kind: QKind) -> Question {
    Question {
        id: 9,
        kind,
        cont: tl3_kernel::Cont::Nowhere,
        cost: 0,
        clarified: false,
    }
}

#[test]
fn options_are_the_answers_the_classifier_accepts_in_order() {
    for kind in KINDS {
        let card = card_question(question(kind), None);
        let answers: Vec<_> = card
            .options
            .iter()
            .map(|option| serde_json::to_value(option.answer).unwrap())
            .collect();
        let (_, accepted) = crate::steering::question(kind);
        assert_eq!(answers, accepted, "{kind:?}");
        assert!((2..=3).contains(&card.options.len()));
        assert_eq!(card.recommended, 0);
        assert_eq!(
            card.reason,
            "This option gives your request a clear next step."
        );
    }
}

#[test]
fn deferring_needs_a_recommendation_tl3_can_apply_alone() {
    for kind in KINDS {
        let card = card_question(question(kind), None);
        let expected = match kind {
            QKind::NeedsSplitting => Some(
                "TL3's recommended option, \"Change the request\", needs your new request from you.",
            ),
            QKind::CantMeet => Some(
                "TL3's recommended option, \"Try again with guidance\", needs your guidance from you.",
            ),
            QKind::OutOfCredit => {
                Some("TL3's recommended option, \"Add credit\", needs a credit amount from you.")
            }
            _ => None,
        };
        assert_eq!(card.defer_blocked.as_deref(), expected, "{kind:?}");
    }
    let mut clarified = question(QKind::EffectFailed);
    clarified.clarified = true;
    assert_eq!(
        card_question(clarified, None).prompt,
        "I could not tell which option you chose. Could you clarify your answer?"
    );
}

#[test]
fn choices_become_the_classifier_reply_for_the_same_answer() {
    assert_eq!(
        Choice::Continue {}.reply(9),
        json!({"kind": "answer", "question_id": "9", "answer": "continue"})
    );
    assert_eq!(
        Choice::ChangeRequest {
            text: "Smaller".into()
        }
        .reply(9),
        json!({"kind": "answer", "question_id": "9", "answer": "change_request", "new_text": "Smaller"})
    );
    assert_eq!(
        Choice::TryAgain {
            guidance: "Use the cache".into()
        }
        .reply(9),
        json!({"kind": "answer", "question_id": "9", "answer": "try_again", "guidance": "Use the cache"})
    );
    assert_eq!(
        Choice::AddCredit { amount: 5 }.reply(9),
        json!({"kind": "answer", "question_id": "9", "answer": "add_credit", "amount": 5})
    );
}

#[test]
fn commands_read_back_from_their_json() {
    let command: Command = serde_json::from_value(json!({
        "id": "00000000-0000-4000-8000-000000000001", "node": 3,
        "action": {"kind": "answer", "question": 9, "choice": {"answer": "add_credit", "amount": 5}}
    }))
    .unwrap();
    assert_eq!(
        command.action,
        Action::Answer {
            question: 9,
            choice: Choice::AddCredit { amount: 5 }
        }
    );
    for action in [
        json!({"kind": "pause", "extra": 1}),
        json!({"kind": "launch"}),
        json!({"kind": "answer", "question": 9, "choice": {"answer": "continue", "amount": 1}}),
    ] {
        assert!(
            serde_json::from_value::<Action>(action.clone()).is_err(),
            "{action}"
        );
    }
}

#[test]
fn options_read_as_the_github_question_does() {
    // #365 changed these words; the card takes them from the same facts.
    let audit = card_question(question(QKind::AuditDisagreement), None);
    assert_eq!(
        audit.options[0].outcome,
        "I will continue with the original understanding of your request and set the check's open objections aside."
    );
    assert_eq!(audit.decision, None);
    assert_eq!(audit.findings, None);
}

#[test]
fn findings_and_the_decision_reach_the_card() {
    let findings = json!({"source": "planner", "report": "docs/ARCHITECTURE.md alone is 170,542 tokens.",
        "decision": "How would you like to narrow the request?"});
    let context = json!({"node": 0, "question": 9, "findings": findings});
    let card = card_question(question(QKind::CantMeet), Some(&context));
    assert_eq!(card.findings, Some(findings));
    assert_eq!(
        card.decision.as_deref(),
        Some("How would you like to narrow the request?")
    );
    assert_eq!(
        card.options[0].outcome,
        "I will try again using your answer to the question above and any guidance you provide."
    );
    // Context without findings shows none.
    let bare = card_question(
        question(QKind::CantMeet),
        Some(&json!({"node": 0, "question": 9})),
    );
    assert_eq!(bare.findings, None);
    assert_eq!(
        bare.options[0].outcome,
        "I will try again using the guidance you provide."
    );
}

#[test]
fn the_doorbell_rings_once_until_answered() {
    let doorbell = Doorbell::default();
    assert!(doorbell.ring(), "the first wake goes out");
    assert!(!doorbell.ring(), "later ones wait for the drain");
    assert!(!doorbell.ring());
    doorbell.answer();
    assert!(
        doorbell.ring(),
        "a command filed after the drain wakes again"
    );
}
