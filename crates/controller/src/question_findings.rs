//! The deciding worker's findings for a question to Rob (#322). The writer
//! summarises them; the worker process redacts them before any model sees them.

use serde_json::{Value, json};
use tl3_kernel::{Cont, Phase, QKind, Question, Resume, ReviewStop};

/// Longest report passed to the writer; it summarises, and never pastes, the report.
pub(crate) const REPORT_CHARS: usize = 6_000;
const PIECE_CHARS: usize = 600;
const ITEM_CHARS: usize = 600;
const ITEMS: usize = 8;

const AUDIT: &str = "Should I go ahead with this reading of your request even though its independent check still has open objections?";
const SPLIT: &str = "How would you like to narrow the request?";
const CANT_MEET: &str = "What should I change about the approach so the request can be met?";
const CREDIT: &str = "Should I spend more of the request's allowance on another attempt?";
const EFFECT: &str = "Can you fix the cause so that I can continue?";

const OUT_OF_ROOM: &str = "The work ran out of room in a single attempt, so it has to be split into smaller parts before it can continue.";
const RECHECK: &str =
    "The change needs another check after its latest update, and its free rechecks are used up.";
const REBASE: &str =
    "Bringing the change up to date with the main branch needs further code changes.";

/// Where a question's evidence can come from.
pub(crate) struct Sources<'a> {
    /// Worker documents written by the step that raised the question.
    pub fresh: &'a dyn Fn(&str) -> Option<Value>,
    /// The node's worker documents before that step.
    pub previous: &'a dyn Fn(&str) -> Option<Value>,
    /// Incidents raised by the step that raised the question.
    pub incidents: &'a [Value],
    /// The node's phase before that step.
    pub prior: Phase,
}

impl Sources<'_> {
    fn stored(&self, table: &str) -> Option<Value> {
        (self.fresh)(table).or_else(|| (self.previous)(table))
    }
}

/// The question a kind asks when no worker findings can be shown.
pub(crate) fn fixed_decision(kind: QKind) -> Option<&'static str> {
    match kind {
        QKind::AuditDisagreement => Some(AUDIT),
        QKind::NeedsSplitting => Some(SPLIT),
        QKind::CantMeet => Some(CANT_MEET),
        QKind::OutOfCredit => Some(CREDIT),
        QKind::EffectFailed => Some(EFFECT),
        QKind::CleanupFailed => None,
    }
}

/// Findings for `question` from the step that raised it. A source that does
/// not match the question's cause gives no findings, never stale ones.
pub(crate) fn findings(question: &Question, sources: &Sources) -> Option<Value> {
    match question.kind {
        QKind::AuditDisagreement => audit(sources),
        QKind::NeedsSplitting => split(&(sources.fresh)("plans")?),
        QKind::CantMeet if matches!(question.cont, Cont::Planning { .. }) => {
            planner(&(sources.fresh)("plans")?)
        }
        QKind::CantMeet => rung(sources, CANT_MEET),
        QKind::OutOfCredit => match question.cont {
            Cont::Planning { must_split: true } => caused(OUT_OF_ROOM),
            Cont::Ready(Resume::Reviewing | Resume::Checking) => caused(RECHECK),
            _ if sources.prior == Phase::Rebasing => caused(REBASE),
            _ => rung(sources, CREDIT),
        },
        QKind::EffectFailed => incident(sources.incidents),
        QKind::CleanupFailed => None,
    }
}

/// Q54: the evidence behind a stop for TL3 review, from the step that
/// stopped. The summary has no decision for Rob, so none is kept.
pub(crate) fn review_stop(step: ReviewStop, sources: &Sources) -> Option<Value> {
    let found = match step {
        ReviewStop::PlanCantMeet => planner(&(sources.fresh)("plans")?),
        ReviewStop::MakerOutOfRoom | ReviewStop::MakerNeedsFiles => {
            let reply = (sources.fresh)("maker_replies")?;
            reported("maker", reply["report"].as_str()?, CANT_MEET)
        }
        ReviewStop::ReviewerOutOfRoom | ReviewStop::PublicationBody => None,
        ReviewStop::ChecksFailed
        | ReviewStop::ChangesRequested
        | ReviewStop::CiFailed
        | ReviewStop::MakerCantMeet => rung(sources, CANT_MEET),
    }?;
    Some(json!({"source": found["source"], "report": found["report"]}))
}

fn caused(report: &str) -> Option<Value> {
    Some(json!({"source": "credit", "report": report, "decision": CREDIT}))
}

fn audit(sources: &Sources) -> Option<Value> {
    let audit = (sources.fresh)("audits")?;
    let brief = sources.stored("briefs")?;
    let objections = items(
        audit["objections"]
            .as_array()?
            .iter()
            .filter_map(Value::as_str),
    );
    if audit["verdict"] != "disagree" || objections.is_empty() {
        return None;
    }
    if brief["kind"] == "early_end" {
        // A duplicate, already-done or not-actionable reading has no brief.
        return Some(json!({"source": "audit",
            "reading": clip(brief["reason"].as_str()?, REPORT_CHARS),
            "outcome": brief["outcome"].as_str()?, "goals": [],
            "objections": objections, "decision": AUDIT}));
    }
    let goals = items(
        brief["goals"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|goal| goal["description"].as_str()),
    );
    Some(
        json!({"source": "audit", "reading": clip(brief["brief"].as_str()?, REPORT_CHARS),
        "goals": goals, "objections": objections, "decision": AUDIT}),
    )
}

/// The planner's size estimate, when it found the work too large.
fn size(forecast: &Value) -> Option<String> {
    let tokens = forecast["tokens"].as_u64()?;
    let limit = forecast["fit_tokens"].as_u64()?;
    let mut report = format!(" Its size estimate is {tokens} tokens against a limit of {limit}.");
    let mut files: Vec<_> = forecast["file_tokens"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(path, tokens)| Some((tokens.as_u64()?, path)))
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
    let largest: Vec<_> = files
        .iter()
        .take(3)
        .map(|(tokens, path)| format!("{} ({tokens} tokens)", clip(path, ITEM_CHARS)))
        .collect();
    if !largest.is_empty() {
        report.push_str(&format!(" The largest files are {}.", largest.join(", ")));
    }
    Some(report)
}

fn split(plan: &Value) -> Option<Value> {
    if plan["kind"] != "split" {
        return None;
    }
    let reason = match plan["reason"].as_str()? {
        "does_not_fit" => "is too large to make as one change",
        "multiple_repos" => "spans more than one repository",
        _ => "ran out of room in a single attempt",
    };
    let report =
        format!("The planned change {reason}.") + &size(&plan["forecast"]).unwrap_or_default();
    let pieces: Vec<_> = plan["pieces"]
        .as_array()?
        .iter()
        .take(ITEMS)
        .map(|piece| {
            let files = items(
                piece["files"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str),
            );
            json!({"brief": clip(piece["brief"].as_str().unwrap_or_default(), PIECE_CHARS),
                "files": files})
        })
        .collect();
    Some(json!({"source": "planner", "report": report, "pieces": pieces, "decision": SPLIT}))
}

fn planner(plan: &Value) -> Option<Value> {
    let violations = items(
        plan["violations"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str),
    );
    let report = match plan["report"].as_str() {
        Some(report) if plan["kind"] == "cant_meet" => report.to_owned(),
        // A rejected plan's reasons are its violations.
        _ if !violations.is_empty() => violations.join("\n"),
        _ if plan["forecast"]["fits"] == false => {
            "The planned change is too large to make as one change.".to_owned()
                + &size(&plan["forecast"])?
        }
        _ => return None,
    };
    let mut found = reported("planner", &report, CANT_MEET)?;
    if plan["kind"] == "cant_meet" {
        found["outcome"] = json!("cant_meet");
    }
    Some(found)
}

/// Evidence from a failed run: the maker's own report, or the failures it just got.
fn rung(sources: &Sources, decision: &str) -> Option<Value> {
    match (sources.fresh)("maker_replies") {
        Some(reply) if reply["kind"] == "cant_meet" => {
            let mut found = reported("maker", reply["report"].as_str()?, decision)?;
            found["outcome"] = json!("cant_meet");
            Some(found)
        }
        _ => failures(sources, decision),
    }
}

/// Only the findings this step appended; earlier rounds were already resolved.
fn failures(sources: &Sources, decision: &str) -> Option<Value> {
    let fresh = (sources.fresh)("maker_findings")?;
    let fresh = fresh.as_array()?;
    let before = (sources.previous)("maker_findings")
        .and_then(|value| value.as_array().map(Vec::len))
        .filter(|len| *len <= fresh.len())
        .unwrap_or(0);
    let reports: Vec<_> = fresh[before..]
        .iter()
        .filter_map(|finding| finding["report"].as_str())
        .collect();
    let latest = &reports[reports.len().saturating_sub(3)..];
    if latest.is_empty() {
        return None;
    }
    Some(json!({"source": "checks_and_review",
        "report": clip(&latest.join("\n\n"), REPORT_CHARS), "decision": decision}))
}

fn incident(incidents: &[Value]) -> Option<Value> {
    let open = items(
        incidents
            .iter()
            .filter(|incident| incident["open"] == true)
            .filter_map(|incident| incident["summary"].as_str()),
    );
    if open.is_empty() {
        return None;
    }
    Some(
        json!({"source": "incident", "report": clip(&open.join("\n"), REPORT_CHARS), "decision": EFFECT}),
    )
}

fn reported(source: &str, report: &str, fallback: &str) -> Option<Value> {
    if report.trim().is_empty() {
        return None;
    }
    let decision = question_in(report).unwrap_or_else(|| fallback.to_owned());
    Some(json!({"source": source, "report": clip(report, REPORT_CHARS), "decision": decision}))
}

/// The worker's own question to Rob: the sentence ending at the report's last
/// question mark that ends a sentence, not one inside a URL or code.
fn question_in(report: &str) -> Option<String> {
    let mark = report
        .char_indices()
        .filter(|&(index, c)| {
            c == '?'
                && report[index + 1..]
                    .chars()
                    .next()
                    .is_none_or(char::is_whitespace)
        })
        .map(|(index, _)| index)
        .next_back()?;
    let head = &report[..mark];
    let start = ["\n", ". ", "? ", "! "]
        .iter()
        .filter_map(|separator| head.rfind(separator).map(|index| index + separator.len()))
        .max()
        .unwrap_or(0);
    let sentence = report[start..=mark].trim_start_matches(['*', '-', '>', '#', ' ']);
    // Drop a lead-in such as "**Question for Rob:**".
    let sentence = match sentence.split_once(':') {
        Some((label, rest)) if label.to_lowercase().contains("question") => {
            rest.trim_start_matches(['*', ' '])
        }
        _ => sentence,
    };
    (sentence.len() > 1 && sentence.chars().count() <= ITEM_CHARS).then(|| sentence.to_owned())
}

fn items<'a>(texts: impl Iterator<Item = &'a str>) -> Vec<String> {
    texts
        .take(ITEMS)
        .map(|text| clip(text, ITEM_CHARS))
        .collect()
}

fn clip(text: &str, limit: usize) -> String {
    const SHORTENED: &str = " [shortened]";
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let kept: String = text.chars().take(limit - SHORTENED.len()).collect();
    kept + SHORTENED
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tl3_kernel::{NodeId, Resume};

    fn fixture(name: &str) -> Value {
        // Bazel declares these through //workers/tests:fixtures as compile data.
        macro_rules! fixture {
            ($($file:literal),*) => {
                match name {
                    $($file => include_str!(concat!(
                        "../../../workers/tests/fixtures/question-findings/", $file
                    )),)*
                    _ => panic!("unknown fixture {name}"),
                }
            };
        }
        serde_json::from_str(fixture!(
            "prismaquant-2662-planner-cant-meet-a.json",
            "prismaquant-2662-planner-cant-meet-b.json",
            "tessera-1464-planner-split.json",
            "audit-disagreement.json",
            "maker-cant-meet.json",
            "out-of-credit.json",
            "effect-failed.json"
        ))
        .unwrap()
    }

    fn question(kind: QKind, cont: Cont) -> Question {
        Question {
            id: 10,
            kind,
            cont,
            cost: 1,
            clarified: false,
        }
    }

    /// `fresh` documents were written by the step that raised the question;
    /// `stored` ones by earlier steps.
    fn run(
        question: &Question,
        fresh: &Value,
        stored: &Value,
        incidents: &[Value],
        prior: Phase,
    ) -> Option<Value> {
        let written = |table: &str| fresh.get(table).cloned();
        let before = |table: &str| stored.get(table).cloned();
        findings(
            question,
            &Sources {
                fresh: &written,
                previous: &before,
                incidents,
                prior,
            },
        )
    }

    fn text(value: &Value, field: &str) -> String {
        value[field]
            .as_str()
            .unwrap_or_else(|| panic!("findings need {field}: {value}"))
            .to_owned()
    }

    fn assert_one_question(decision: &str) {
        assert!(decision.trim_end().ends_with('?'), "{decision}");
        assert_eq!(decision.matches('?').count(), 1, "{decision}");
    }

    const PLANNING: Cont = Cont::Planning { must_split: false };
    const MAKING: Cont = Cont::Ready(Resume::Making);

    #[test]
    fn real_2636_report_identifies_cant_meet_for_planner_and_maker() {
        let reply: Value = serde_json::from_str(include_str!(
            "../../../workers/tests/fixtures/question-findings/prismabuild-2636-planner-cant-meet.json"
        )).unwrap();
        for (source, cont, fresh, prior) in [
            (
                "planner",
                PLANNING,
                json!({"plans": reply}),
                Phase::Planning { must_split: false },
            ),
            (
                "maker",
                MAKING,
                json!({"maker_replies": reply}),
                Phase::Making,
            ),
        ] {
            let found = run(
                &question(QKind::CantMeet, cont),
                &fresh,
                &json!({}),
                &[],
                prior,
            )
            .unwrap();
            assert_eq!(found["source"], source);
            assert_eq!(
                found["outcome"], "cant_meet",
                "the writer must distinguish an explicit refusal"
            );
            assert_eq!(found["report"], reply["report"]);
        }
    }

    #[test]
    fn planner_cant_meet_reports_carry_their_cause_and_question() {
        // prismaquant#2662, question 10: both planner reports from MLflow, redacted.
        for name in [
            "prismaquant-2662-planner-cant-meet-a.json",
            "prismaquant-2662-planner-cant-meet-b.json",
        ] {
            let fresh = json!({"plans": fixture(name)});
            let found = run(
                &question(QKind::CantMeet, PLANNING),
                &fresh,
                &json!({}),
                &[],
                Phase::Planning { must_split: false },
            )
            .unwrap();
            assert_eq!(found["source"], "planner");
            let report = text(&found, "report");
            assert!(report.contains("CURRENT_SOURCE_BLOBS"), "{name}: {report}");
            assert!(report.chars().count() <= REPORT_CHARS);
            let decision = text(&found, "decision");
            assert!(
                decision.starts_with("may this leaf also edit")
                    && decision.contains("CURRENT_SOURCE_BLOBS"),
                "{name}: the planner's own question must reach Rob: {decision}"
            );
            assert_one_question(&decision);
        }
    }

    #[test]
    fn maker_cant_meet_report_carries_its_question() {
        let found = run(
            &question(QKind::CantMeet, MAKING),
            &fixture("maker-cant-meet.json"),
            &json!({}),
            &[],
            Phase::Making,
        )
        .unwrap();
        assert_eq!(found["source"], "maker");
        assert!(text(&found, "report").contains("read at import time"));
        assert_eq!(
            text(&found, "decision"),
            "Should the cache directory become a constructor argument instead of a module-level setting?"
        );
    }

    #[test]
    fn making_cant_meet_ignores_a_report_rob_already_answered() {
        // An earlier cant_meet was answered; the run after it failed checks instead.
        let stale = fixture("maker-cant-meet.json");
        let fresh = fixture("out-of-credit.json");
        let found = run(
            &question(QKind::CantMeet, MAKING),
            &fresh,
            &stale,
            &[],
            Phase::Checking,
        )
        .unwrap();
        assert_eq!(found["source"], "checks_and_review");
        assert!(!text(&found, "report").contains("import time"));
        assert!(
            run(
                &question(QKind::CantMeet, MAKING),
                &json!({}),
                &stale,
                &[],
                Phase::Making
            )
            .is_none(),
            "a run that failed without a reply has no report to show"
        );
    }

    #[test]
    fn split_carries_the_size_cause_and_each_proposed_piece() {
        // tessera#1464, question 9: the planner's split from MLflow, redacted.
        let fresh = json!({"plans": fixture("tessera-1464-planner-split.json")});
        let found = run(
            &question(QKind::NeedsSplitting, Cont::Nowhere),
            &fresh,
            &json!({}),
            &[],
            Phase::Planning { must_split: false },
        )
        .unwrap();
        assert_eq!(found["source"], "planner");
        let report = text(&found, "report");
        for part in ["docs/ARCHITECTURE.md", "170542", "100000"] {
            assert!(
                report.contains(part),
                "the size cause names {part}: {report}"
            );
        }
        let pieces = found["pieces"].as_array().unwrap();
        assert_eq!(pieces.len(), 2);
        assert!(pieces.iter().all(|piece| {
            piece["brief"].as_str().unwrap().chars().count() <= PIECE_CHARS
                && piece["files"].is_array()
        }));
        // The options are only "Change the request" and "Stop".
        assert_eq!(
            text(&found, "decision"),
            "How would you like to narrow the request?"
        );
    }

    #[test]
    fn split_pieces_clip_their_file_lists() {
        let files: Vec<_> = (0..40)
            .map(|index| format!("src/{}/module_{index}.py", "deep/".repeat(200)))
            .collect();
        let fresh = json!({"plans": {"kind": "split", "reason": "does_not_fit",
            "pieces": [{"brief": "One.", "goal_ids": [0], "files": files},
                {"brief": "Two.", "goal_ids": [1], "files": []}]}});
        let found = run(
            &question(QKind::NeedsSplitting, Cont::Nowhere),
            &fresh,
            &json!({}),
            &[],
            Phase::Planning { must_split: false },
        )
        .unwrap();
        let files = found["pieces"][0]["files"].as_array().unwrap();
        assert!(files.len() <= ITEMS, "{}", files.len());
        assert!(
            files
                .iter()
                .all(|file| file.as_str().unwrap().chars().count() <= ITEM_CHARS)
        );
    }

    #[test]
    fn splitting_without_a_split_plan_has_no_findings() {
        let fresh = json!({"plans": {"kind": "leaf", "files": ["a.py"], "checks": [],
            "no_design_questions": true, "lands_on_its_own": true, "violations": []}});
        assert!(
            run(
                &question(QKind::NeedsSplitting, Cont::Nowhere),
                &fresh,
                &json!({}),
                &[],
                Phase::Planning { must_split: false }
            )
            .is_none()
        );
    }

    #[test]
    fn planning_cant_meet_for_an_oversized_leaf_reports_its_size() {
        let fresh = json!({"plans": {"kind": "leaf", "files": ["docs/big.md"], "checks": [],
            "no_design_questions": true, "lands_on_its_own": true, "violations": [],
            "forecast": {"tokens": 230121, "fit_tokens": 100000, "fits": false,
                "file_tokens": {"docs/big.md": 170542, "src/a.py": 10}}}});
        let found = run(
            &question(QKind::CantMeet, PLANNING),
            &fresh,
            &json!({}),
            &[],
            Phase::Planning { must_split: false },
        )
        .unwrap();
        let report = text(&found, "report");
        for part in ["230121", "100000", "docs/big.md"] {
            assert!(report.contains(part), "{part}: {report}");
        }
        assert_one_question(&text(&found, "decision"));
    }

    #[test]
    fn audit_disagreement_carries_the_reading_and_open_objections() {
        let documents = fixture("audit-disagreement.json");
        let fresh = json!({"audits": documents["audits"]});
        let stored = json!({"briefs": documents["briefs"]});
        let found = run(
            &question(QKind::AuditDisagreement, Cont::Unplanned),
            &fresh,
            &stored,
            &[],
            Phase::Auditing,
        )
        .unwrap();
        assert_eq!(found["source"], "audit");
        assert!(text(&found, "reading").contains("compact JSON export"));
        assert_eq!(found["goals"].as_array().unwrap().len(), 2);
        let objections = found["objections"].as_array().unwrap();
        assert_eq!(objections.len(), 2);
        assert!(objections[0].as_str().unwrap().contains("never says where"));
        assert_one_question(&text(&found, "decision"));
    }

    #[test]
    fn audit_disagreement_on_an_early_end_carries_its_reason() {
        let fresh = json!({"audits": {"verdict": "disagree",
            "objections": ["The earlier issue covered a different export."]}});
        let stored = json!({"briefs": {"kind": "early_end", "outcome": "duplicate",
            "reason": "The same export was requested and merged under the earlier issue."}});
        let found = run(
            &question(
                QKind::AuditDisagreement,
                Cont::Stop(tl3_kernel::Outcome::Duplicate),
            ),
            &fresh,
            &stored,
            &[],
            Phase::Auditing,
        )
        .unwrap();
        assert_eq!(
            text(&found, "reading"),
            "The same export was requested and merged under the earlier issue."
        );
        assert_eq!(found["outcome"], "duplicate");
        assert_eq!(found["objections"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn out_of_credit_after_a_failed_round_carries_the_latest_failures() {
        let found = run(
            &question(QKind::OutOfCredit, MAKING),
            &fixture("out-of-credit.json"),
            &json!({}),
            &[],
            Phase::Reviewing,
        )
        .unwrap();
        assert_eq!(found["source"], "checks_and_review");
        let report = text(&found, "report");
        assert!(report.contains("fixed 5 seconds") && report.contains("base passed"));
        assert_one_question(&text(&found, "decision"));
    }

    #[test]
    fn out_of_credit_names_its_own_cause_and_never_old_failures() {
        let old = fixture("out-of-credit.json");
        for (cont, prior, cause) in [
            (
                Cont::Planning { must_split: true },
                Phase::Making,
                "ran out of room",
            ),
            (MAKING, Phase::Rebasing, "main branch"),
            (
                Cont::Ready(Resume::Reviewing),
                Phase::Reviewing,
                "free rechecks",
            ),
            (
                Cont::Ready(Resume::Checking),
                Phase::Rebasing,
                "free rechecks",
            ),
        ] {
            let found = run(
                &question(QKind::OutOfCredit, cont),
                &json!({}),
                &old,
                &[],
                prior,
            )
            .unwrap_or_else(|| panic!("{cont:?} needs its cause"));
            let report = text(&found, "report");
            assert!(report.contains(cause), "{cont:?}: {report}");
            assert!(
                !report.contains("fixed 5 seconds") && !report.contains("base passed"),
                "{cont:?} must not show resolved failures: {report}"
            );
            assert_one_question(&text(&found, "decision"));
        }
        assert!(
            run(
                &question(QKind::OutOfCredit, MAKING),
                &json!({}),
                &old,
                &[],
                Phase::Reviewing
            )
            .is_none(),
            "a failed round shows only failures from that round"
        );
    }

    #[test]
    fn effect_failed_carries_this_node_open_incident() {
        let incidents: Vec<_> = fixture("effect-failed.json")["incidents"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|incident| incident["node"] == json!(NodeId(0)))
            .cloned()
            .collect();
        let found = run(
            &question(QKind::EffectFailed, MAKING),
            &json!({}),
            &json!({}),
            &incidents,
            Phase::Publishing,
        )
        .unwrap();
        assert_eq!(found["source"], "incident");
        let report = text(&found, "report");
        assert!(report.contains("signed commits"), "{report}");
        assert!(
            !report.contains("later succeeded"),
            "closed incidents are history"
        );
        assert_one_question(&text(&found, "decision"));
    }

    #[test]
    fn missing_documents_give_no_findings() {
        for kind in [
            QKind::AuditDisagreement,
            QKind::NeedsSplitting,
            QKind::CantMeet,
            QKind::EffectFailed,
        ] {
            assert!(
                run(
                    &question(kind, Cont::Nowhere),
                    &json!({}),
                    &json!({}),
                    &[],
                    Phase::Making
                )
                .is_none(),
                "{kind:?}"
            );
        }
    }

    #[test]
    fn the_decision_is_the_last_real_question_in_the_report() {
        for (report, expected) in [
            (
                "See https://example.test/run?id=4 for the log.\n\nShould I keep the old export?",
                Some("Should I keep the old export?"),
            ),
            (
                "The pattern `.*?` matches lazily. May I widen it?",
                Some("May I widen it?"),
            ),
            (
                "It returns `Option<T>?` today. May I return a Result instead?",
                Some("May I return a Result instead?"),
            ),
            (
                "Why does the test fail? Because the pin is stale.\n\nQuestion for Rob: may I update the pin?",
                Some("may I update the pin?"),
            ),
            (
                "The pin is stale.\n\n**Question for Rob:** may I update the pin?",
                Some("may I update the pin?"),
            ),
            ("See https://example.test/run?id=4 for the log.", None),
        ] {
            assert_eq!(question_in(report).as_deref(), expected, "{report}");
        }
    }
}
