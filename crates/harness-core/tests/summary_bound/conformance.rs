//! #823: independent document adapter for the Verus reduction reference.
//! It reconstructs every candidate using immutable JSON paths, then compares
//! the complete value, exact byte count, and typed removal report to bounded().
use super::{reference, value};
use proptest::prelude::*;
use tl3_harness_core::summary::{
    BoundError, BoundedSummary, DropReport, Dropped, DroppedKind, PathPart, PythonEnvelope,
    PythonText, SummarySerializer, SummaryValue, bounded,
};

type V = SummaryValue;
const NOTICE: &str =
    "Truncated: selectors were compacted or entries were cut. Totals include all results.";
fn text(s: &str) -> V {
    V::String(s.into())
}
fn set(v: &mut V, key: &str, x: V) {
    let V::Object(fields) = v else {
        panic!("expected summary object")
    };
    if let Some((_, old)) = fields
        .iter_mut()
        .find(|(name, _)| name == &PythonText::from(key))
    {
        *old = x;
    } else {
        fields.push((key.into(), x));
    }
}
fn remove(v: &mut V, key: &str) {
    let V::Object(fields) = v else {
        panic!("expected summary object")
    };
    fields.retain(|(name, _)| name != &PythonText::from(key));
}
fn entries(v: &V, key: &str) -> Vec<V> {
    match v.field(key) {
        Some(V::Array(items)) => items.clone(),
        _ => vec![],
    }
}
fn size(v: &V) -> u64 {
    PythonEnvelope.serialize(v).unwrap().len() as u64
}
fn selects(selector: &PythonText, node: &PythonText) -> bool {
    let s = selector.codepoints();
    let n = node.codepoints();
    s == n
        || (!s.windows(2).any(|v| v == [58, 58]) && n.starts_with(&[s, &[58, 58]].concat()))
        || (!s.contains(&91) && n.starts_with(&[s, &[91]].concat()) && n.last() == Some(&93))
}
fn red(summary: &V, item: &V) -> bool {
    let proof = summary.field("proof");
    let observer = proof.and_then(|p| p.field("mode")) == Some(&text("head-observer"))
        || proof.is_some() && summary.field("forcing").is_some();
    let outcome = item.field("outcome");
    let planned = match (
        proof.and_then(|p| p.field("observer_sha256")),
        item.field("test"),
    ) {
        (Some(V::Object(fields)), Some(V::String(node))) => {
            fields.iter().any(|(name, _)| selects(name, node))
        }
        _ => false,
    };
    let symptom = summary
        .field("declared_symptom")
        .and_then(|v| v.field("node"));
    let required = (outcome == Some(&text("failed")) && observer)
        || (outcome == Some(&text("error"))
            && (observer && planned || symptom.is_some() && symptom == item.field("test")));
    required
        || ((outcome == Some(&text("failed")) || outcome == Some(&text("error")))
            && entries(summary, "groups")
                .iter()
                .any(|g| entries(g, "tests").contains(item) && red(g, item)))
}
fn compact(summary: &mut V) {
    let Some(V::Array(selectors)) = summary.field("selectors") else {
        return;
    };
    let selectors = selectors
        .iter()
        .map(|v| match v {
            V::String(t) => t.clone(),
            _ => panic!("selector"),
        })
        .collect::<Vec<_>>();
    // Identity itself has separate byte-exact conformance and equality proofs.
    let id = tl3_harness_core::identity::selector_identity(&selectors);
    let points = id
        .prefix
        .iter()
        .map(|t| V::String(t.clone()))
        .collect::<Vec<_>>();
    let candidate = V::Object(vec![
        ("schema".into(), text(&id.schema)),
        ("count".into(), value(&id.count.to_string())),
        ("sha256".into(), text(&id.sha256)),
        ("prefix".into(), V::Array(points)),
    ]);
    if size(&candidate) < size(summary.field("selectors").unwrap()) {
        set(summary, "selectors", candidate);
    }
}
fn mandatory(input: &V) -> V {
    let mut out = input.clone();
    compact(&mut out);
    if input.field("tests").is_some() {
        set(
            &mut out,
            "tests",
            V::Array(
                entries(input, "tests")
                    .into_iter()
                    .filter(|v| red(input, v))
                    .collect(),
            ),
        );
    }
    if input.field("note").is_some() {
        set(&mut out, "note", text(""));
    }
    if input.field("runs").is_some() {
        let runs = entries(input, "runs")
            .into_iter()
            .map(|mut run| {
                if run.field("failed").is_some() {
                    set(&mut run, "failed", V::Array(vec![]));
                }
                run
            })
            .collect();
        set(&mut out, "runs", V::Array(runs));
    }
    if input.field("groups").is_some() {
        set(
            &mut out,
            "groups",
            V::Array(entries(input, "groups").iter().map(mandatory).collect()),
        );
    }
    if let Some(base) = input.field("base") {
        set(&mut out, "base", mandatory(base));
    }
    out
}
#[derive(Clone)]
struct Atom {
    path: Vec<PathPart>,
    rank: u8,
    order: usize,
    kind: DroppedKind,
}
fn atoms(input: &V, path: Vec<PathPart>, out: &mut Vec<Atom>) {
    let mut push = |path, rank, kind| {
        out.push(Atom {
            path,
            rank,
            order: out.len(),
            kind,
        });
    };
    if matches!(input.field("selectors"), Some(V::Array(_))) {
        push(path.clone(), 0, DroppedKind::SelectorCompaction);
    }
    for (i, item) in entries(input, "tests").iter().enumerate().rev() {
        if red(input, item) {
            continue;
        }
        let passing = item.field("outcome") == Some(&text("passed"));
        let kind = if passing {
            DroppedKind::PassingEntry
        } else if item.field("outcome") == Some(&text("error")) {
            DroppedKind::ErrorEntry
        } else {
            DroppedKind::FailureEntry
        };
        let mut at = path.clone();
        at.extend([PathPart::Field("tests".into()), PathPart::Index(i)]);
        push(at, if passing { 1 } else { 2 }, kind);
    }
    for (i, run) in entries(input, "runs").iter().enumerate().rev() {
        for j in (0..entries(run, "failed").len()).rev() {
            let mut at = path.clone();
            at.extend([
                PathPart::Field("runs".into()),
                PathPart::Index(i),
                PathPart::Field("failed".into()),
                PathPart::Index(j),
            ]);
            push(at, 4, DroppedKind::RunFailure);
        }
    }
    for (i, g) in entries(input, "groups").iter().enumerate() {
        let mut at = path.clone();
        at.extend([PathPart::Field("groups".into()), PathPart::Index(i)]);
        atoms(g, at, out);
    }
    if let Some(base) = input.field("base") {
        let mut at = path;
        at.push(PathPart::Field("base".into()));
        atoms(base, at, out);
    }
}
fn at<'a>(v: &'a V, path: &[PathPart]) -> &'a V {
    path.iter().fold(v, |v, part| match part {
        PathPart::Field(s) => v.field(s).unwrap(),
        PathPart::Index(i) => match v {
            V::Array(a) => &a[*i],
            _ => panic!("array"),
        },
    })
}
// A generic immutable JSON reconstruction, independent of production's walker.
fn candidate(v: &V, path: &mut Vec<PathPart>, removed: &[Atom]) -> V {
    let mut result = match v {
        V::Array(items) => V::Array(
            items
                .iter()
                .enumerate()
                .filter_map(|(i, item)| {
                    path.push(PathPart::Index(i));
                    let dropped = removed
                        .iter()
                        .any(|a| a.kind != DroppedKind::SelectorCompaction && a.path == *path);
                    let next = if dropped {
                        None
                    } else {
                        Some(candidate(item, path, removed))
                    };
                    path.pop();
                    next
                })
                .collect(),
        ),
        V::Object(fields) => V::Object(
            fields
                .iter()
                .map(|(key, item)| {
                    let name = key
                        .codepoints()
                        .iter()
                        .map(|p| char::from_u32(*p).unwrap())
                        .collect::<String>();
                    path.push(PathPart::Field(name));
                    let next = candidate(item, path, removed);
                    path.pop();
                    (key.clone(), next)
                })
                .collect(),
        ),
        _ => v.clone(),
    };
    if removed
        .iter()
        .any(|a| a.kind == DroppedKind::SelectorCompaction && a.path == *path)
    {
        compact(&mut result);
    }
    result
}
fn notices(v: &mut V) {
    set(v, "note", text(NOTICE));
    let groups = entries(v, "groups");
    if v.field("groups").is_some() {
        set(
            v,
            "groups",
            V::Array(
                groups
                    .into_iter()
                    .map(|mut g| {
                        notices(&mut g);
                        g
                    })
                    .collect(),
            ),
        );
    }
    if let Some(base) = v.field("base") {
        let mut base = base.clone();
        notices(&mut base);
        set(v, "base", base);
    }
}
fn diagnostic(input: &V) -> V {
    let failure = value(
        r#"{"outcome":"transport_failure","phase":"summary","termination_reason":"identity_exceeds_summary_limit","test_started":false}"#,
    );
    let mut result = value(
        r#"{"status":"infrastructure_error","message":"Test execution transport_failure in summary: identity_exceeds_summary_limit. No test verdict is available; inspect this action, not fleet availability.","tests":[],"totals":{},"note":"Truncated: summary identity exceeds the envelope limit. No proof is available."}"#,
    );
    if let Some(totals) = input.field("totals") {
        set(&mut result, "totals", totals.clone());
    }
    if let Some(origin) = input.field("origin").filter(|v| **v != V::Null) {
        set(&mut result, "origin", origin.clone());
    }
    set(
        &mut result,
        "execution",
        input.field("execution").cloned().unwrap_or(failure),
    );
    for key in ["action_key", "receipts"] {
        if let Some(v) = input.field(key) {
            set(&mut result, key, v.clone());
        }
    }
    result
}
fn expected(input: &V, limit: usize) -> Result<BoundedSummary, BoundError> {
    let core = mandatory(input);
    let mut diag = diagnostic(input);
    if size(&diag) > limit as u64 {
        remove(&mut diag, "receipts");
    }
    let mut marked = input.clone();
    notices(&mut marked);
    let mut order = vec![];
    atoms(input, vec![], &mut order);
    order.sort_by(|a, b| {
        if reference::precedes(a.rank, a.order, b.rank, b.order) {
            std::cmp::Ordering::Less
        } else if a.order == b.order {
            std::cmp::Ordering::Equal
        } else {
            std::cmp::Ordering::Greater
        }
    });
    let candidates = (0..=order.len())
        .map(|i| {
            let plan = reference::Plan {
                kind: 1,
                removed: i,
                bytes: 0,
            };
            let removed = order
                .iter()
                .enumerate()
                .filter(|(index, _)| !reference::keeps(plan, *index, false))
                .map(|(_, atom)| atom.clone())
                .collect::<Vec<_>>();
            candidate(&marked, &mut vec![], &removed)
        })
        .collect::<Vec<_>>();
    let sizes = candidates.iter().map(size).collect::<Vec<_>>();
    let plan = reference::reduce(size(input), size(&core), size(&diag), &sizes, limit as u64);
    let (result, removed) = match plan.kind {
        0 => (input.clone(), 0),
        1 => (candidates[plan.removed].clone(), plan.removed),
        2 => (core, order.len()),
        3 => (diag, 0),
        4 => {
            return Err(BoundError::DiagnosticTooLarge {
                required: plan.bytes as usize,
                limit,
            });
        }
        _ => unreachable!(),
    };
    for (i, atom) in order.iter().enumerate() {
        assert!(reference::priority_transitive(
            atom.rank, i, atom.rank, i, atom.rank, i
        ));
        assert!(reference::prefix_preserves_order(plan, i, order.len()));
        if plan.kind <= 2 {
            assert!(reference::keeps(plan, i, true));
        }
    }
    let dropped = order[..removed]
        .iter()
        .filter(|a| {
            a.kind != DroppedKind::SelectorCompaction
                || at(&result, &a.path).field("selectors")
                    != at(&marked, &a.path).field("selectors")
        })
        .map(|a| Dropped {
            path: a.path.clone(),
            kind: a.kind,
            count: 1,
        })
        .collect();
    Ok(BoundedSummary {
        value: result,
        serialized_bytes: plan.bytes as usize,
        report: DropReport {
            dropped,
            diagnostic: plan.kind == 3,
        },
    })
}
fn conform(input: V, limit: usize) {
    assert_eq!(
        bounded(input.clone(), limit, &PythonEnvelope),
        expected(&input, limit),
        "limit={limit}, input={input:?}"
    );
}
// Root, grouped, nested-group and base envelopes use independently generated
// facts. Include planned errors, failed RED, symptom RED, run lists, selectors,
// immutable identities, UTF-8/escapes and optional display of every priority.
fn node(mode: u8, outcomes: &[u8], width: usize, proof_width: usize, selected: usize) -> V {
    let tests=outcomes.iter().enumerate().map(|(i,o)| serde_json::json!({"test":format!("t.py::t{i}"),"outcome":(["passed","failed","error","skipped"][*o as usize%4]),"message":"é\\\"".repeat(width)})).collect::<Vec<_>>();
    let mut v=value(&serde_json::json!({"status":"completed","tests":tests,"totals":{"failed":outcomes.len()},"note":"old note","origin":"test_run","selectors":(0..selected).map(|i|format!("t.py::{}-{i}","s".repeat(width))).collect::<Vec<_>>(),"runs":[{"run":1,"failed":["t.py::x","t.py::y"]}],"identity":"i".repeat(proof_width/2)}).to_string());
    if mode != 0 {
        set(&mut v,"proof",value(&serde_json::json!({"mode":if mode==1 || mode==2 {"head-observer"} else {"pytest-outcome"},"payload":"p".repeat(proof_width),"observer_sha256":{"t.py":"digest"}}).to_string()));
    }
    if mode == 3 {
        set(&mut v, "forcing", value("{}"));
    }
    if mode == 4 {
        set(&mut v, "declared_symptom", value(r#"{"node":"t.py::t0"}"#));
    }
    v
}
proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]
    /// #823(a-d): full-document conformance connects the verified reference to
    /// production traversal, removals, serializer measurements and fast paths.
    #[test]
    fn bounded_matches_the_verified_document_reduction(
        nodes in proptest::collection::vec((0u8..5,proptest::collection::vec(0u8..4,0..9),0usize..80,0usize..1600,0usize..5),1..5),
        nested in any::<bool>(), base in any::<bool>(), limit in 0usize..12_000,
    ) {
        let mut values=nodes.iter().map(|(m,o,w,p,s)|node(*m,o,*w,*p,*s)).collect::<Vec<_>>();
        let mut root=values.remove(0);
        if base && !values.is_empty() {set(&mut root,"base",values.remove(0));}
        if nested && values.len()>1 {let child=values.pop().unwrap();set(&mut values[0],"groups",V::Array(vec![child]));}
        if !values.is_empty() {set(&mut root,"groups",V::Array(values));}
        conform(root,limit);
    }
}
#[test]
fn incident_documents_match_the_verified_reduction() {
    let corpus = super::fixture("core/summary-bound");
    for case in corpus["cases"].as_array().unwrap() {
        if let Some(input) = case["input"].get("result") {
            conform(
                super::vector_value(input),
                case["input"]["limit"].as_u64().unwrap_or(14_336) as usize,
            );
        }
    }
}

/// #823 review: fixed boundaries kill the oversized-success, reversed-priority,
/// lost-proof and premature-diagnostic production mutants independently of RNG.
#[test]
fn production_contract_boundaries_match_the_verified_document_reference() {
    let input = value(&format!(
        r#"{{"status":"completed","tests":[{{"test":"RED","outcome":"failed","message":"{}"}},{{"test":"PASS","outcome":"passed","message":"{}"}},{{"test":"ERROR","outcome":"error","message":"{}"}}],"totals":{{"failed":1,"passed":1,"error":1}},"proof":{{"mode":"head-observer","payload":"{}"}},"note":"","origin":"test_run"}}"#,
        "r".repeat(400),
        "x".repeat(800),
        "x".repeat(800),
        "p".repeat(200)
    ));
    let full = size(&input) as usize;
    let core = size(&mandatory(&input)) as usize;
    for limit in [
        0,
        512,
        core - 1,
        core,
        core + 100,
        1600,
        2000,
        full - 1,
        full,
    ] {
        conform(input.clone(), limit);
    }
}
