//! The deciding worker's findings for a question to Rob (#322). The writer
//! summarises them; the worker process redacts them before any model sees them.

use serde_json::{Value, json};
use tl3_kernel::{Cont, QKind, Question};

/// Longest report passed to the writer; it summarises, and never pastes, the report.
pub(crate) const REPORT_CHARS: usize = 6_000;
const PIECE_CHARS: usize = 600;
const ITEM_CHARS: usize = 600;
const ITEMS: usize = 8;

const AUDIT: &str = "Should I go ahead with this reading of your request even though its independent check still has open objections?";
const SPLIT: &str =
    "Which of these smaller parts should I do first, or how would you like to narrow the request?";
const CANT_MEET: &str = "What should I change about the approach so the request can be met?";
const CREDIT: &str =
    "Should I spend more of the request's allowance on another attempt to fix these problems?";
const EFFECT: &str = "Can you fix this cause so that I can continue?";

/// Findings for `question`, read from the node's stored worker documents and incidents.
pub(crate) fn findings(
    question: &Question,
    document: &dyn Fn(&str) -> Option<Value>,
    incidents: &[Value],
) -> Option<Value> {
    match question.kind {
        QKind::AuditDisagreement => audit(document),
        QKind::NeedsSplitting => split(&document("plans")?),
        QKind::CantMeet if matches!(question.cont, Cont::Planning { .. }) => {
            planner(&document("plans")?)
        }
        QKind::CantMeet => maker(document),
        QKind::OutOfCredit => failures(document, CREDIT),
        QKind::EffectFailed => incident(incidents),
        QKind::CleanupFailed => None,
    }
}

fn audit(document: &dyn Fn(&str) -> Option<Value>) -> Option<Value> {
    let brief = document("briefs")?;
    let audit = document("audits")?;
    let objections = items(
        audit["objections"]
            .as_array()?
            .iter()
            .filter_map(Value::as_str),
    );
    if audit["verdict"] != "disagree" || objections.is_empty() {
        return None;
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

fn split(plan: &Value) -> Option<Value> {
    if plan["kind"] != "split" {
        return None;
    }
    let reason = match plan["reason"].as_str()? {
        "does_not_fit" => "is too large to make as one change",
        "multiple_repos" => "spans more than one repository",
        _ => "ran out of room in a single attempt",
    };
    let mut report = format!("The planned change {reason}.");
    let forecast = &plan["forecast"];
    if let (Some(tokens), Some(limit)) =
        (forecast["tokens"].as_u64(), forecast["fit_tokens"].as_u64())
    {
        report.push_str(&format!(
            " Its size estimate is {tokens} tokens against a limit of {limit}."
        ));
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
            .map(|(tokens, path)| format!("{path} ({tokens} tokens)"))
            .collect();
        if !largest.is_empty() {
            report.push_str(&format!(" The largest files are {}.", largest.join(", ")));
        }
    }
    let pieces: Vec<_> = plan["pieces"]
        .as_array()?
        .iter()
        .take(ITEMS)
        .map(|piece| {
            json!({"brief": clip(piece["brief"].as_str().unwrap_or_default(), PIECE_CHARS),
                "files": piece["files"].clone()})
        })
        .collect();
    Some(json!({"source": "planner", "report": report, "pieces": pieces, "decision": SPLIT}))
}

fn planner(plan: &Value) -> Option<Value> {
    let report = match plan["report"].as_str() {
        Some(report) if plan["kind"] == "cant_meet" => report.to_owned(),
        // A rejected plan's reasons are its violations.
        _ => items(
            plan["violations"]
                .as_array()?
                .iter()
                .filter_map(Value::as_str),
        )
        .join("\n"),
    };
    reported("planner", &report, CANT_MEET)
}

fn maker(document: &dyn Fn(&str) -> Option<Value>) -> Option<Value> {
    match document("maker_replies") {
        Some(reply) if reply["kind"] == "cant_meet" => {
            reported("maker", reply["report"].as_str()?, CANT_MEET)
        }
        _ => failures(document, CANT_MEET),
    }
}

fn failures(document: &dyn Fn(&str) -> Option<Value>, decision: &str) -> Option<Value> {
    let findings = document("maker_findings")?;
    let reports: Vec<_> = findings
        .as_array()?
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

/// The worker's own question to Rob: the sentence ending at its first question mark.
fn question_in(report: &str) -> Option<String> {
    let end = report.find('?')? + 1;
    let line = report[..end].rfind('\n').map_or(0, |index| index + 1);
    let start = report[line..end]
        .rfind(". ")
        .map_or(line, |index| line + index + 2);
    let sentence = report[start..end].trim();
    // Drop a lead-in such as "Question for Rob:".
    let sentence = match sentence.split_once(": ") {
        Some((label, rest)) if label.to_lowercase().contains("question") => rest,
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
    use tl3_kernel::{Cont, NodeId, QKind, Resume};

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

    fn from(documents: &Value) -> impl Fn(&str) -> Option<Value> + '_ {
        move |table| documents.get(table).cloned()
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

    #[test]
    fn planner_cant_meet_reports_carry_their_cause_and_question() {
        // prismaquant#2662, question 10: both planner reports from MLflow, redacted.
        for name in [
            "prismaquant-2662-planner-cant-meet-a.json",
            "prismaquant-2662-planner-cant-meet-b.json",
        ] {
            let documents = json!({"plans": fixture(name)});
            let found = findings(
                &question(QKind::CantMeet, Cont::Planning { must_split: false }),
                &from(&documents),
                &[],
            )
            .unwrap();
            assert_eq!(found["source"], "planner");
            let report = text(&found, "report");
            assert!(report.contains("CURRENT_SOURCE_BLOBS"), "{name}: {report}");
            assert!(report.chars().count() <= REPORT_CHARS);
            let decision = text(&found, "decision");
            assert!(
                decision.contains("may this leaf also edit")
                    && decision.contains("CURRENT_SOURCE_BLOBS"),
                "{name}: the planner's own question must reach Rob: {decision}"
            );
            assert_one_question(&decision);
        }
    }

    #[test]
    fn maker_cant_meet_report_carries_its_question() {
        let documents = fixture("maker-cant-meet.json");
        let found = findings(
            &question(QKind::CantMeet, Cont::Ready(Resume::Making)),
            &from(&documents),
            &[],
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
    fn split_carries_the_size_cause_and_each_proposed_piece() {
        // tessera#1464, question 9: the planner's split from MLflow, redacted.
        let documents = json!({"plans": fixture("tessera-1464-planner-split.json")});
        let found = findings(
            &question(QKind::NeedsSplitting, Cont::Nowhere),
            &from(&documents),
            &[],
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
        assert_one_question(&text(&found, "decision"));
    }

    #[test]
    fn audit_disagreement_carries_the_reading_and_open_objections() {
        let documents = fixture("audit-disagreement.json");
        let found = findings(
            &question(QKind::AuditDisagreement, Cont::Unplanned),
            &from(&documents),
            &[],
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
    fn out_of_credit_carries_the_latest_failures() {
        let documents = fixture("out-of-credit.json");
        let found = findings(
            &question(QKind::OutOfCredit, Cont::Ready(Resume::Making)),
            &from(&documents),
            &[],
        )
        .unwrap();
        assert_eq!(found["source"], "checks_and_review");
        let report = text(&found, "report");
        assert!(report.contains("fixed 5 seconds") && report.contains("base passed"));
        assert_one_question(&text(&found, "decision"));
    }

    #[test]
    fn effect_failed_carries_this_node_open_incident() {
        let incidents = fixture("effect-failed.json")["incidents"]
            .as_array()
            .unwrap()
            .clone();
        let found = findings(
            &question(QKind::EffectFailed, Cont::Ready(Resume::Making)),
            &|_| None,
            &incidents
                .into_iter()
                .filter(|incident| incident["node"] == json!(NodeId(0)))
                .collect::<Vec<_>>(),
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
            QKind::OutOfCredit,
            QKind::EffectFailed,
        ] {
            assert!(
                findings(&question(kind, Cont::Nowhere), &|_| None, &[]).is_none(),
                "{kind:?}"
            );
        }
    }
}
