#[path = "support/input_resource_world.rs"]
mod resource_world;
mod support;
use resource_world::World;
use serde_json::{Value, json};
use support::Database;
use tl3_controller as controller_under_test;
use tl3_controller::adapters::workers::writer::{WriteRequest, Writer, WriterReply};
use tl3_github::message::Policy;
use tl3_kernel::Phase;
use tl3_store::{Commit, RowChange, Store};

/// Q59/#833: an issue number must never be read as a PR merely because ingest says so.
#[test]
fn mismatch_is_validated_before_the_pull_endpoint_and_uses_bounded_run_failures() {
    let mut w = World::new();
    w.reply("pull_request", 2713);
    let first = w.controller.world().unwrap().nodes[0];
    assert_eq!(
        first.generation, 1,
        "the existing invalid-reply path must rerun ingest"
    );
    w.reply("pull_request", 2713);
    assert!(
        w.controller.world().unwrap().nodes[0].waiting_incident,
        "Q7 must bound repeated validation failures"
    );
    let requests = w.runtime.block_on(w.server.received_requests()).unwrap();
    assert!(
        !requests
            .iter()
            .any(|r| r.url.path().contains("/pulls/2713")),
        "a known issue must not reach the 404 PR endpoint"
    );
    let incidents = w.incidents();
    assert_eq!(incidents.len(), 1);
    assert_eq!(
        incidents[0]["signature"]["class"],
        "prerequisite_kind_mismatch"
    );
    assert_eq!(incidents[0]["cause"], "tl3");
    let facts = &incidents[0]["input_failure"]["resource_failure"];
    assert_eq!(
        facts["requested"],
        json!({"kind":"github_pull_request","repository":"owner/sandbox","number":2713})
    );
    assert_eq!(facts["problem"]["observed"]["kind"], "github_issue");
    let count = requests.len();
    World::drain(&mut w.controller);
    assert_eq!(
        w.runtime
            .block_on(w.server.received_requests())
            .unwrap()
            .len(),
        count,
        "an open incident must halt this operation"
    );
}

/// Q59/#833: the #2713-shaped valid issue prerequisite still reaches independent audit.
#[test]
fn correct_issue_prerequisite_reaches_audit() {
    let mut w = World::new();
    w.reply("issue", 2703);
    assert_eq!(
        w.controller.world().unwrap().nodes[0].phase,
        Phase::Auditing
    );
    assert!(w.incidents().is_empty());
    let snapshot = w.controller.snapshot().unwrap();
    let declarations = &snapshot
        .rows
        .iter()
        .find(|r| r.table == "merge_prerequisites")
        .unwrap()
        .value;
    assert_eq!(declarations[0]["target"]["kind"], "Issue");
}

/// Q7/#833: changing generation and observation content must not create another cause lineage.
#[test]
fn not_found_dedupes_across_generations_for_the_same_request_and_resource() {
    let mut w = World::new();
    w.reply("issue", 4040);
    w.next_generation();
    w.reply("issue", 4040);
    assert_eq!(
        w.incidents().len(),
        1,
        "one open incident per request, cause, and typed resource"
    );
}

/// Q7/#833: distinct missing resources must remain distinct incident causes.
#[test]
fn different_missing_resources_are_not_deduped() {
    let mut w = World::new();
    w.reply("issue", 4040);
    w.next_generation();
    w.reply("issue", 4041);
    assert_eq!(w.incidents().len(), 2);
}

/// Q7/#833: the adapter identity must reach stored diagnostics, writer facts, and the note.
#[test]
fn missing_resource_is_named_in_the_incident_writer_facts_and_posted_note() {
    let mut w = World::new();
    w.reply("issue", 4040);
    let incidents = w.incidents();
    assert_eq!(incidents.len(), 1);
    let failure = &incidents[0]["input_failure"]["resource_failure"];
    assert_eq!(
        failure["requested"],
        json!({"kind":"github_issue","repository":"owner/sandbox","number":4040})
    );
    assert!(
        incidents[0]["summary"]
            .as_str()
            .unwrap()
            .contains("GitHub issue owner/sandbox#4040")
    );
    assert!(
        w.facts
            .lock()
            .unwrap()
            .iter()
            .any(|input| input.facts["input_resource_failure"] == *failure),
        "typed identity must reach the writer"
    );
    let requests = w.runtime.block_on(w.server.received_requests()).unwrap();
    assert!(
        requests
            .iter()
            .filter(|r| r.method.as_str() == "POST" && r.url.path().ends_with("/comments"))
            .any(
                |r| serde_json::from_slice::<Value>(&r.body).unwrap()["body"]
                    .as_str()
                    .unwrap()
                    .contains("GitHub issue owner/sandbox#4040")
            ),
        "the posted note must identify the resource"
    );
}

/// Q7/#833: successful later translation is not an authorized incident-clear event.
#[test]
fn a_corrected_declaration_does_not_clear_an_unprobed_input_incident() {
    let mut w = World::new();
    w.reply("issue", 4040);
    w.next_generation();
    w.reply("issue", 2713);
    assert_eq!(
        w.controller.world().unwrap().nodes[0].phase,
        Phase::Auditing
    );
    assert_eq!(w.incidents().len(), 1);
}

/// Q59/#833: a genuine PR retains merge semantics after the canonical identity check.
#[test]
fn correct_pull_request_preserves_its_declared_kind() {
    let mut w = World::new();
    w.reply("pull_request", 2715);
    assert_eq!(
        w.controller.world().unwrap().nodes[0].phase,
        Phase::Auditing
    );
    assert!(w.incidents().is_empty());
    let snapshot = w.controller.snapshot().unwrap();
    let declarations = &snapshot
        .rows
        .iter()
        .find(|r| r.table == "merge_prerequisites")
        .unwrap()
        .value;
    assert_eq!(declarations[0]["target"]["kind"], "PullRequest");
    assert_eq!(declarations[0]["status"], "Pending");
    let requests = w.runtime.block_on(w.server.received_requests()).unwrap();
    let issue = requests
        .iter()
        .position(|r| r.url.path().ends_with("/issues/2715"))
        .unwrap();
    let pull = requests
        .iter()
        .position(|r| r.url.path().ends_with("/pulls/2715"))
        .unwrap();
    assert!(issue < pull);
}

/// Q7/Q59/#833: the inverse mismatch must not silently change close into merge semantics.
#[test]
fn an_issue_declaration_of_a_pull_request_also_fails_validation() {
    let mut w = World::new();
    w.reply("issue", 2715);
    assert_eq!(w.controller.world().unwrap().nodes[0].generation, 1);
    w.reply("issue", 2715);
    assert!(w.controller.world().unwrap().nodes[0].waiting_incident);
    let incidents = w.incidents();
    assert_eq!(incidents.len(), 1);
    assert_eq!(
        incidents[0]["input_failure"]["resource_failure"]["problem"]["observed"]["kind"],
        "github_pull_request"
    );
}

/// Q7/Q59/#833: a corrected retry can reach audit without accepting the invalid declaration.
#[test]
fn a_corrected_kind_on_the_validation_retry_reaches_audit() {
    let mut w = World::new();
    w.reply("pull_request", 2713);
    let snapshot = w.controller.snapshot().unwrap();
    assert!(
        !snapshot
            .rows
            .iter()
            .any(|r| r.table == "briefs" || r.table == "merge_prerequisites")
    );
    w.reply("issue", 2713);
    assert_eq!(
        w.controller.world().unwrap().nodes[0].phase,
        Phase::Auditing
    );
    assert!(w.incidents().is_empty());
}

/// #833/#432: a trusted resource name must not authorize a different issue number.
#[test]
fn typed_resource_facts_do_not_allow_a_different_number_in_writer_prose() {
    use tl3_controller::adapters::workers::writer::{MessageKind, TextDraft, checked_draft};
    struct WrongNumber;
    impl Writer for WrongNumber {
        fn draft(&mut self, _: &str, _: &WriteRequest) -> tl3_controller::Result<WriterReply> {
            Ok(WriterReply::Text(TextDraft {
                text: "Missing GitHub issue owner/sandbox#40400.".into(),
            }))
        }
    }
    let input = WriteRequest {
        kind: MessageKind::Note,
        facts: json!({"issue_title":"Cover scanned sources","request":"Cover scanned sources",
            "input_resource_failure":{"requested":{"kind":"github_issue","repository":"owner/sandbox","number":4040},"problem":{"kind":"not_found"}}}),
        violations: vec![],
        request: None,
    };
    assert!(
        checked_draft(
            &mut WrongNumber,
            "wrong-resource",
            input,
            &Policy::default()
        )
        .is_err(),
        "the typed resource exception must not hide a different issue number"
    );
}

/// Q7/Q59/#833: a declaration is bounded before any prerequisite API read.
#[test]
fn over_cap_prerequisites_fail_as_a_reply_without_network_reads() {
    let mut w = World::new();
    let mut store = Store::open(&w.settings).unwrap();
    let mut materials = store.worker_materials("0").unwrap().unwrap();
    let numbers: Vec<_> = (2703..2736).collect();
    materials["issue"]["body"] = json!(
        numbers
            .iter()
            .map(|n| format!("#{n}"))
            .collect::<Vec<_>>()
            .join(" ")
    );
    store.put_worker_materials("0", &materials).unwrap();
    let before = w
        .runtime
        .block_on(w.server.received_requests())
        .unwrap()
        .len();
    w.controller.observe("over-cap", tl3_controller::executor::Observation::WorkerReply {
        node:tl3_kernel::NodeId(0), generation:0,role:"ingest".into(),model:None,
        reply:json!({"kind":"brief","brief":"Cover scanned sources","goals":[{"description":"Cover sources","verification":"test"}],"memory_ids":[],"files":[],"forecast":{"tokenizer":"o200k_base","brief_tokens":1,"file_tokens":{},"overhead_tokens":0,"tokens":1,"fit_tokens":100,"context_tokens":150,"fits":true},"merge_prerequisites": numbers.iter().map(|n| json!({"kind":"issue","number":n,"source_quote":format!("#{n}")})).collect::<Vec<_>>()}),
    }).unwrap();
    World::drain(&mut w.controller);
    let requests = w.runtime.block_on(w.server.received_requests()).unwrap();
    assert!(
        !requests[before..]
            .iter()
            .any(|r| r.method.as_str() == "GET" && r.url.path().contains("/issues/")),
        "over-cap declarations must be rejected before the first prerequisite read"
    );
    assert_eq!(w.controller.world().unwrap().nodes[0].generation, 1);
    assert!(w.incidents().is_empty());
}

/// #833 rollout: an unidentified legacy incident cannot be merged into a new resource lineage.
#[test]
fn a_recurring_legacy_fault_starts_one_new_typed_lineage_without_rewriting_history() {
    let mut w = World::new();
    w.reply("issue", 4040);
    let mut legacy = w.incidents().pop().unwrap();
    let id = legacy["id"].as_str().unwrap().to_owned();
    legacy["input_failure"]["scope"] = json!("observation:legacy-generation-zero");
    legacy["input_failure"]
        .as_object_mut()
        .unwrap()
        .remove("resource_failure");
    let snapshot = w.controller.snapshot().unwrap();
    let mut commit = Commit::new("legacy-rollout", json!(null), &snapshot);
    commit.rows.push(RowChange::Put {
        table: "incidents".into(),
        id: id.clone(),
        value: legacy.clone(),
    });
    Store::open(&w.settings).unwrap().commit(&commit).unwrap();
    w.next_generation();
    w.reply("issue", 4040);
    let incidents = w.incidents();
    assert_eq!(incidents.len(), 2);
    assert_eq!(incidents.iter().find(|r| r["id"] == id).unwrap(), &legacy);
    let typed = incidents.iter().find(|r| r["id"] != id).unwrap();
    assert_eq!(
        typed["needs_rob"], false,
        "unidentified legacy incidents do not change the new lineage count"
    );
    assert!(
        typed["input_failure"]["scope"]
            .as_str()
            .unwrap()
            .starts_with("request:0:not_found:")
    );
    w.next_generation();
    w.reply("issue", 4040);
    assert_eq!(
        w.incidents().len(),
        2,
        "subsequent typed faults dedupe within the new lineage"
    );
}

/// Q59/#833: serde and schemars carry the same prerequisite count bound.
#[test]
fn ingest_prerequisite_contract_has_a_32_item_bound() {
    use tl3_controller::contracts::roles::ingest_reply::IngestReply;
    let mut reply = json!({"kind":"brief","brief":"Cover sources","goals":[{"description":"Cover sources","verification":"test"}],"memory_ids":[],"merge_prerequisites":(1..=32).map(|n|json!({"kind":"issue","number":n,"source_quote":format!("#{n}")})).collect::<Vec<_>>()});
    assert!(serde_json::from_value::<IngestReply>(reply.clone()).is_ok());
    reply["merge_prerequisites"]
        .as_array_mut()
        .unwrap()
        .push(json!({"kind":"issue","number":33,"source_quote":"#33"}));
    assert!(
        serde_json::from_value::<IngestReply>(reply).is_err(),
        "the Rust type must reject a 33rd prerequisite"
    );
    let schema = tl3_controller::contracts::roles::ingest_reply::schema_value();
    assert_eq!(
        schema["oneOf"][0]["properties"]["merge_prerequisites"]["maxItems"],
        32
    );
}
