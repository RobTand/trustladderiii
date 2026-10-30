use tl3_github::message::*;

fn draft() -> Draft {
    Draft {
        issue_title: "Add a blue button".into(),
        request_quote: "Add a blue button".into(),
        obstacle: "The design you chose needs permission to use its artwork.".into(),
        options: vec![
            OptionDraft {
                action: "Choose different artwork".into(),
                outcome: "I can finish using a free image.".into(),
            },
            OptionDraft {
                action: "Grant permission".into(),
                outcome: "I can finish with your original design.".into(),
            },
        ],
        recommended_option: Some(0),
        recommendation_reason: "The free image avoids a licensing delay.".into(),
        how_to_answer: "Reply with option one or two in this conversation.".into(),
    }
}

fn rejects(mut change: impl FnMut(&mut Draft), expected: Violation) {
    let mut draft = draft();
    change(&mut draft);
    assert!(
        check(&draft, &Policy::default())
            .unwrap_err()
            .contains(&expected),
        "{expected:?}"
    );
}

#[test]
fn renders_a_complete_self_contained_message() {
    let message = check(&draft(), &Policy::default()).unwrap();
    assert!(message.as_str().contains("\"Add a blue button\""));
    assert!(
        message
            .as_str()
            .contains("1. Choose different artwork: I can finish using a free image.")
    );
    assert!(message.as_str().contains("I recommend option 1."));
}

#[test]
fn checks_every_required_part() {
    rejects(|d| d.request_quote.clear(), Violation::MissingRequest);
    rejects(|d| d.issue_title.clear(), Violation::MissingRequest);
    rejects(
        |d| d.request_quote = "Do something else".into(),
        Violation::RequestDoesNotQuoteTitle,
    );
    rejects(|d| d.obstacle.clear(), Violation::MissingObstacle);
    rejects(
        |d| d.obstacle = "One. Two. Three.".into(),
        Violation::ObstacleTooLong,
    );
    rejects(
        |d| {
            d.options.pop();
        },
        Violation::OptionCount,
    );
    rejects(
        |d| {
            d.options.extend(d.options.clone());
        },
        Violation::OptionCount,
    );
    rejects(
        |d| d.options[0].action.clear(),
        Violation::MissingOptionAction(0),
    );
    rejects(
        |d| d.options[1].outcome.clear(),
        Violation::MissingOptionOutcome(1),
    );
    rejects(
        |d| d.recommended_option = None,
        Violation::MissingRecommendation,
    );
    rejects(
        |d| d.recommended_option = Some(2),
        Violation::MissingRecommendation,
    );
    rejects(
        |d| d.recommendation_reason.clear(),
        Violation::MissingRecommendationReason,
    );
    rejects(
        |d| d.how_to_answer.clear(),
        Violation::MissingAnswerInstructions,
    );
}

#[test]
fn blocks_vocabulary_inflections_case_and_hidden_unicode() {
    for term in [
        "leaf",
        "leaves",
        "lineage",
        "replan",
        "re-plan",
        "measures",
        "node",
        "generation",
        "effect",
        "TL3",
        "AwaitingRob",
        "outbox",
        "ＥＦＦＥＣＴ",
        "ef\u{200b}fect",
    ] {
        rejects(
            |d| d.obstacle = format!("The {term} needs attention."),
            Violation::InternalVocabulary,
        );
    }
}

#[test]
fn blocks_ids_names_paths_and_logs_in_every_section() {
    for term in [
        "deadbeef",
        "b5862850-8ed7-47f7-8fdf-887a96ba872c",
        "id: 12",
        "request_id",
        "effectKey",
        "RepoSlot",
        "#123",
        "<!-- hidden -->",
    ] {
        rejects(
            |d| d.options[0].outcome = term.into(),
            Violation::InternalNameOrId,
        );
    }
    for path in [
        "/home/rob/work",
        "src/main.rs",
        "README.md",
        "C:\\work\\file.txt",
        "../private",
        "~/work",
        "/tmp/output",
    ] {
        rejects(
            |d| d.recommendation_reason = path.into(),
            Violation::FilePath,
        );
    }
    for log in [
        "```text\nraw\n```",
        "Traceback",
        "thread panicked",
        "stderr",
        "ERROR: failed",
        "2026-10-30T01:12:00Z",
    ] {
        rejects(|d| d.how_to_answer = log.into(), Violation::Log);
    }
    let mut d = draft();
    d.obstacle = "The Copper Finch needs help.".into();
    assert!(
        check(
            &d,
            &Policy {
                internal_terms: vec!["Copper Finch".into()]
            }
        )
        .unwrap_err()
        .contains(&Violation::InternalNameOrId)
    );
}

#[test]
fn three_options_and_punctuation_are_allowed() {
    let mut d = draft();
    d.options.push(OptionDraft {
        action: "Stop".into(),
        outcome: "I will leave your current design as it is.".into(),
    });
    d.obstacle = "The artwork needs permission. You can grant it or choose another image.".into();
    assert!(check(&d, &Policy::default()).is_ok());
}

#[test]
fn invisible_characters_cannot_fill_required_sections() {
    rejects(
        |d| d.obstacle = "\u{200b} \u{2060}".into(),
        Violation::MissingObstacle,
    );
    rejects(
        |d| d.options[0].action = "\u{200b}".into(),
        Violation::MissingOptionAction(0),
    );
    rejects(
        |d| d.options[0].outcome = "\u{200b}".into(),
        Violation::MissingOptionOutcome(0),
    );
    rejects(
        |d| d.recommendation_reason = "\u{200b}".into(),
        Violation::MissingRecommendationReason,
    );
    rejects(
        |d| d.how_to_answer = "\u{200b}".into(),
        Violation::MissingAnswerInstructions,
    );
}
