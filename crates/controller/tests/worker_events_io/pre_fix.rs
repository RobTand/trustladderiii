// Verbatim F2-base functions, SHA256-pinned in the harness.
use super::super::parse_event;
use super::*;
use tl3_controller_core::worker_events::*;
pub fn reply_step(event: WorkerEvent, reply_seen: bool, last_line_equal: bool) -> ReplyDecision {
    if reply_seen {
        return ReplyDecision::AfterReply;
    }
    match event {
        WorkerEvent::Progress { .. } => ReplyDecision::Progress,
        WorkerEvent::Reply => ReplyDecision::Reply,
        WorkerEvent::Invalid => ReplyDecision::InvalidEvent,
        WorkerEvent::Quota { model, reset_ms } => {
            if matches!(model, QuotaModel::Forbidden) {
                return ReplyDecision::InvalidQuotaModel;
            }
            match reset_ms {
                Some(n) if n > 0 => (),
                _ => return ReplyDecision::InvalidQuotaReset,
            }
            if !last_line_equal {
                return ReplyDecision::AfterQuota;
            }
            ReplyDecision::Quota
        }
    }
}
pub(super) fn parse_reply(
    bytes: &[u8],
    delivery: &Delivery,
    inputs: &Value,
) -> Result<Observation> {
    if !bytes.ends_with(b"\n") {
        return Err(Error::Invalid("incomplete worker output"));
    }
    let role = protocol::delivery_role(delivery)
        .ok_or(Error::Invalid("worker output without supported start"))?;
    let mut reply = None;
    for line in bytes.split_inclusive(|b| *b == b'\n') {
        let parsed = parse_event(line, Some(role), delivery.is_postmortem(), inputs)?;
        match reply_step(
            parsed.event,
            reply.is_some(),
            bytes.split_inclusive(|b| *b == b'\n').next_back() == Some(line),
        ) {
            ReplyDecision::Progress => continue,
            ReplyDecision::AfterReply => return Err(Error::Invalid("event after worker reply")),
            ReplyDecision::InvalidEvent => return Err(Error::Invalid("invalid worker event")),
            ReplyDecision::InvalidQuotaModel => return Err(Error::Invalid("invalid quota model")),
            ReplyDecision::InvalidQuotaReset => return Err(Error::Invalid("invalid quota reset")),
            ReplyDecision::AfterQuota => return Err(Error::Invalid("event after quota ending")),
            ReplyDecision::Quota => {
                let WorkerEvent::Quota {
                    reset_ms: Some(reset_ms),
                    ..
                } = parsed.event
                else {
                    unreachable!("quota decision")
                };
                return Ok(Observation::Quota {
                    key: delivery.key.clone(),
                    reset_ms,
                    model: parsed.model,
                });
            }
            ReplyDecision::Reply => (),
        }
        let value = parsed.value;
        if delivery.is_classifier() {
            protocol::validate_classification(inputs, &value["reply"])?;
        } else if delivery.is_postmortem() {
            crate::postmortems::check_reply(&value["reply"])?;
        } else {
            protocol::map_reply(role, &value["reply"])?;
            if role == "reviewer" {
                super::review::validate_reply(&value["reply"], inputs)?;
            }
            if role == "maker" && value["reply"]["kind"] == "done" {
                let current = crate::findings::current_review(
                    &inputs["feedback"],
                    delivery.round,
                    inputs["reviewed_sha"].as_str(),
                );
                super::reproducing::check(&value["reply"], current)?;
            }
            protocol::validate_forecast_settings(&value["reply"], &inputs["forecast"])?;
        }
        let mut checked = value["reply"].clone();
        super::evidence::preflight(role, inputs, &mut checked)?;
        if !delivery.is_classifier() && !delivery.is_postmortem() {
            protocol::map_reply(role, &checked)?;
        }
        reply = Some(checked);
    }
    if delivery.is_classifier() {
        return Ok(Observation::Classified {
            key: delivery.key.clone(),
            reply: Some(reply.ok_or(Error::Invalid("worker reply is missing"))?),
        });
    }
    if delivery.is_postmortem() {
        return Ok(Observation::Postmortem {
            key: delivery.key.clone(),
            reply: Some(reply.ok_or(Error::Invalid("worker reply is missing"))?),
            judge_failure: None,
            failure: None,
        });
    }
    Ok(Observation::WorkerReply {
        node: delivery.node(),
        generation: delivery.generation,
        role: role.into(),
        reply: reply.ok_or(Error::Invalid("worker reply is missing"))?,
        model: None,
    })
}
