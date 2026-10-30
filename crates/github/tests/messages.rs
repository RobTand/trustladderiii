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
        "lineage",
        "replan",
        "replans",
        "replanned",
        "replanning",
        "re-plan",
        "re-planned",
        "re-planning",
        "measure",
        "measures",
        "lineages",
        "re-plans",
        "ＬＥＡＦ",
        "lea\u{200b}f",
    ] {
        for form in [term.to_string(), term.to_uppercase()] {
            rejects(
                |d| d.obstacle = format!("The {form} needs attention."),
                Violation::InternalVocabulary,
            );
        }
    }
}

#[test]
fn blocks_ids_names_paths_and_logs_in_every_section() {
    for term in [
        "deadb33f",
        "b5862850-8ed7-47f7-8fdf-887a96ba872c",
        "id: 12",
        "request_id",
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
                internal_terms: vec!["Copper Finch".into()],
                ..Policy::default()
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

#[test]
fn quotes_robs_title_verbatim_without_scanning_it() {
    for title in [
        "Fix the kernel node pool",
        "Deploy to GitHub",
        "Re-plan the leaf lineage measure in src/main.rs for request_id #123",
        "ERROR: \"RepoSlot\" needs attention\n<!-- original text -->",
    ] {
        let mut d = draft();
        d.issue_title = title.into();
        d.request_quote = title.into();
        let policy = Policy {
            internal_terms: vec!["RepoSlot".into()],
            ..Policy::default()
        };
        let checked = check(&d, &policy).unwrap();
        assert!(
            checked
                .as_str()
                .starts_with(&format!("You asked for \"{title}\"."))
        );
        d.obstacle = "The leaf needs attention.".into();
        assert!(
            check(&d, &policy)
                .unwrap_err()
                .contains(&Violation::InternalVocabulary)
        );
    }
}

#[test]
fn plain_english_and_product_names_pass_in_all_written_fields() {
    for sentence in [
        "Reply yes/no when you are ready.",
        "Choose an image and/or a color.",
        "The service runs 24/7.",
        "Deploy to GitHub using PostgreSQL and YouTube.",
        "Fix the kernel node pool.",
        "The next generation has a useful effect.",
        "We can settle this after the test.",
        "The outbox is full while we are auditing footprints from earlier epochs.",
    ] {
        let mut d = draft();
        d.obstacle = sentence.into();
        d.options[0].action = sentence.into();
        d.options[0].outcome = sentence.into();
        d.recommendation_reason = sentence.into();
        d.how_to_answer = sentence.into();
        assert!(check(&d, &Policy::default()).is_ok(), "{sentence}");
    }
}

#[test]
fn actual_internal_names_are_supplied_by_policy() {
    for name in ["effectKey", "RepoSlot", "AwaitingRob", "TL3"] {
        let mut d = draft();
        d.obstacle = format!("The {name} needs attention.");
        assert!(
            check(
                &d,
                &Policy {
                    internal_terms: vec![name.into()],
                    ..Policy::default()
                }
            )
            .unwrap_err()
            .contains(&Violation::InternalNameOrId)
        );
    }
}

#[test]
fn hex_ids_need_at_least_seven_characters_a_digit_and_a_letter() {
    for id in [
        "abc1234",
        "23b00f2ca28caec396e5553a9cd98911632d8b8e",
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    ] {
        for form in [id.to_string(), id.to_uppercase()] {
            rejects(
                |d| d.options[0].outcome = format!("Use {form}."),
                Violation::InternalNameOrId,
            );
        }
    }
    for text in [
        "defaced", "10000000", "2026", "accede", "deadbeef", "abc123", "abc1234g",
    ] {
        let mut d = draft();
        d.options[0].outcome = format!("Use {text}.");
        assert!(check(&d, &Policy::default()).is_ok(), "{text}");
    }
}

#[test]
fn common_inflections_and_words_containing_vocabulary_pass() {
    for word in [
        "leaves",
        "measured",
        "measuring",
        "measurement",
        "measurements",
        "leaflet",
        "lineageology",
        "replanner",
        "re planned",
    ] {
        for form in [word.to_string(), word.to_uppercase()] {
            let mut d = draft();
            d.options[0].outcome = form;
            assert!(check(&d, &Policy::default()).is_ok(), "{word}");
        }
    }
}

#[test]
fn paths_are_known_names_explicit_paths_or_slash_paths_with_extensions() {
    let known = ["deploy/github-app", "workers/src", "README.md", "workers"];
    let policy = Policy {
        known_paths: known.into_iter().map(String::from).collect(),
        ..Policy::default()
    };
    for path in known.into_iter().chain([
        "/etc/hosts",
        "~/.config/tl3/github-app.json",
        "src/main.rs",
        "./work",
        "../work",
        "assets/image.avif",
        "data/file.a1",
        "data/file.12345",
    ]) {
        for text in [
            path.to_string(),
            format!("Use ({path})."),
            format!("Use `{path}`, please."),
        ] {
            let mut d = draft();
            d.options[0].outcome = text.clone();
            assert!(
                check(&d, &policy)
                    .unwrap_err()
                    .contains(&Violation::FilePath),
                "{text}"
            );
        }
    }
    for text in [
        "10/30/2026",
        "pass/fail/skip",
        "Mon/Tue/Wed",
        "TCP/IP/UDP",
        "Node.js",
        "Next.js",
        "approx./",
        "yes/no",
        "and/or",
        "24/7",
        "README.md",
        "deploy/github-app",
        "workers/src",
        "src/work",
        "a/b/c",
        "data/file.RS",
        "data/file.abcdef",
        "~/",
        "./",
        "../",
        "C:\\work\\file.txt",
    ] {
        let mut d = draft();
        d.options[0].outcome = format!("Use {text}.");
        assert!(check(&d, &Policy::default()).is_ok(), "{text}");
    }
    let mut d = draft();
    d.options[0].outcome = "Use other/workers/src or workers/src-extra.".into();
    assert!(check(&d, &policy).is_ok());
}
