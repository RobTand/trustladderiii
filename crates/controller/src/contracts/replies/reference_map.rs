// Test-only reference copied verbatim from cdacce8f99c28ca8dba8f5f76831b77b4dfcd836.
use super::*;
pub(crate) fn map_reply(role: &str, reply: &Value) -> Result<MappedReply> {
    let validator = validator(role, "reply")?;
    if !validator.is_valid(reply) {
        return Err(Error::Invalid("worker reply breaks role schema"));
    }
    if role == "maker" {
        return Ok(match reply["kind"].as_str().unwrap() {
            "done"
                if reply["self_check"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|check| check["status"] == "not_met")
                    || crate::budget::decision::reject_done(
                        true,
                        reply["budget_exhaustion"]["unfinished_obligations"]
                            .as_array()
                            .is_some_and(|items| !items.is_empty()),
                    ) =>
            {
                MappedReply::Kernel(Reply::MakerCantMeet)
            }
            "done" => MappedReply::Made(reply["head"].as_str().unwrap().into()),
            "needs_files" => {
                let paths = reply["files"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|item| item["path"].as_str().unwrap().to_owned())
                    .collect::<Vec<_>>();
                if paths.iter().any(|path| !relative_file(path)) {
                    return Err(Error::Invalid("maker requested an invalid path"));
                }
                MappedReply::NeedsFiles {
                    paths,
                    fits: reply["fits"].as_bool().unwrap_or(false),
                }
            }
            "cant_meet" => MappedReply::Kernel(Reply::MakerCantMeet),
            // Q50: an environment blocker is an infrastructure failure. The
            // worker checked its evidence; the incident text cites it.
            "blocked" => MappedReply::Kernel(Reply::RunFailed),
            "out_of_room" => MappedReply::Kernel(Reply::MakerOutOfRoom),
            _ => unreachable!(),
        });
    }
    Ok(MappedReply::Kernel(match role {
        "ingest" if reply["kind"] == "brief" => {
            let forecast = forecast(reply)?;
            let size = forecast
                .0
                .div_ceil(forecast.1)
                .max(1)
                .try_into()
                .map_err(|_| Error::Invalid("request size exceeds kernel range"))?;
            Reply::IngestDone { size }
        }
        "ingest" => Reply::IngestEarlyEnd(match reply["outcome"].as_str() {
            Some("duplicate") => EarlyEnd::Duplicate,
            Some("already_done") => EarlyEnd::AlreadyDone,
            Some("not_actionable") => EarlyEnd::NotActionable,
            _ => unreachable!("validated early outcome"),
        }),
        "audit" if reply["verdict"] == "agree" => Reply::AuditAgrees,
        "audit" => Reply::AuditDisagrees,
        "reviewer" if reply["verdict"] == "approve" => Reply::Approved {
            head: tl3_kernel::HeadId(0),
        },
        "reviewer" if reply["verdict"] == "out_of_room" => Reply::ReviewerOutOfRoom,
        "reviewer" => Reply::ChangesRequested,
        "planner" => {
            forecast(reply)?;
            let violations = reply["violations"]
                .as_array()
                .ok_or(Error::Invalid("planner violations missing"))?;
            if !violations.is_empty() || reply["kind"] == "cant_meet" {
                Reply::PlanCantMeet
            } else if reply["kind"] == "split" {
                Reply::PlanSplit
            } else {
                Reply::PlanLeaf {
                    footprint: tl3_kernel::Footprint::default(),
                    fits: reply["forecast"]["fits"] == true,
                }
            }
        }
        _ => return Err(Error::Invalid("unsupported worker role")),
    }))
}

use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig { cases:512, ..ProptestConfig::default() })]
    /// Q50/Q54/#730: the mapper has no I/O, clock reads, or durable writes.
    /// Both implementations therefore have the same empty I/O schedule. Vary
    /// each sampled terminal reply, including malformed endings and amendments.
    #[test]
    fn typed_mapping_matches_the_released_decider(index in 0usize..1000, amendment in 0u8..5) {
        let rows=include_str!("../../../tests/fixtures/worker-replies-d8e4f2a8.jsonl")
            .lines().map(|line|serde_json::from_str::<Value>(line).unwrap()).collect::<Vec<_>>();
        let row=&rows[index % rows.len()];
        for event in row["stdout"].as_array().unwrap() {
            if event["event"]!="reply" {continue;}
            let role=event["role"].as_str().unwrap();
            let mut reply=event["reply"].clone();
            match amendment {
                1=> {reply["unexpected"]=serde_json::json!(true);},
                2=> { if let Some(files)=reply.get_mut("files").and_then(Value::as_array_mut)
                    && let Some(first)=files.first_mut() {
                        if first.is_string() {*first=serde_json::json!("../bad.py");}
                        else {first["path"]=serde_json::json!("../bad.py");}
                }},
                3=> {if reply["kind"]=="done" {reply["self_check"]=serde_json::json!([{"goal_id":"g","status":"not_met","why":"unfinished"}]);}},
                4 if reply["forecast"].is_object()=> {reply["forecast"]["tokens"]=serde_json::json!(0);},
                _=>{},
            }
            let before=map_reply(role,&reply);
            let after=super::map_reply(role,&reply);
            match (before,after) {
                (Ok(before),Ok(after))=>prop_assert_eq!(before,after),
                (Err(before),Err(after))=>prop_assert_eq!(before.to_string(),after.to_string()),
                (before,after)=>prop_assert!(false,"{role} {reply}: {before:?} != {after:?}"),
            }
        }
    }
}
