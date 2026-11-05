// Function bodies are checked against their pinned source by the harness.
use super::io::*;
use super::*;
use tl3_controller_core::worker_events::*;
struct Shell<'a>(&'a mut WorkerOutputSource);
impl std::ops::Deref for Shell<'_> {
    type Target = WorkerOutputSource;
    fn deref(&self) -> &Self::Target {
        self.0
    }
}
impl std::ops::DerefMut for Shell<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0
    }
}
impl Shell<'_> {
    fn refresh(
        &mut self,
        run: &mut Run,
        store: &mut Store,
        sender: &Sender<Input>,
    ) -> Result<bool> {
        if refresh_gate(at("store", store.worker_reconcilable(&run.delivery.key))?)
            == RefreshGate::Retire
        {
            return Ok(false);
        }
        if run.attempt.is_none() {
            run.attempt = at("metadata/stat", latest(&run.root))?;
            if let Some((_, attempt)) = &run.attempt {
                let wd = self.watch(attempt)?;
                self.watches.insert(wd, run.root.clone());
                run.watches.push(wd);
            }
        }
        if let Some((_, attempt)) = &run.attempt
            && read(&attempt.join("claim.json"))?.is_some_and(|c| {
                claim_decision(if c["cancelled"] == true {
                    Claim::Cancelled
                } else {
                    Claim::Live
                }) == ClaimDecision::Retire
            })
        {
            return Ok(false);
        }
        let Some((number, attempt)) = &run.attempt else {
            return Ok(true);
        };
        let delivery = &run.delivery;
        // A directory or cancelled claim is not evidence of a run that started.
        if claim_decision(if read(&attempt.join("claim.json"))?.is_none() {
            Claim::Missing
        } else {
            Claim::Live
        }) == ClaimDecision::AwaitClaim
        {
            return Ok(true);
        }
        for name in ["claim.json", "process.json"] {
            if let Some(fd) = process::pidfd(&attempt.join(name))? {
                run.processes.insert(name, fd);
            } else {
                run.processes.remove(name);
            }
        }
        let stdout = match at("read", File::open(attempt.join("stdout.jsonl"))) {
            Ok(file) => Some(file),
            Err(error) if matches!(cause(&error), Error::Io(io) if io.kind() == std::io::ErrorKind::NotFound) => {
                None
            }
            Err(error) => return Err(error),
        };
        let prior = at(
            "store",
            store.worker_progress(Some(&delivery.node().0.to_string())),
        )?
        .into_iter()
        .find(|row| row.key == delivery.key && row.attempt == *number);
        let recorded = prior.as_ref().map_or(0, |row| row.output_offset as u64);
        let mut previous_wait = prior.and_then(|row| row.external_wait);
        if let Some(file) = stdout {
            let mut reader = BufReader::new(file);
            at("seek", reader.seek(SeekFrom::Start(run.offset)))?;
            loop {
                let mut line = Vec::new();
                let n = at("read", reader.read_until(b'\n', &mut line))?;
                if n == 0 || !line.ends_with(b"\n") {
                    break;
                }
                run.offset += n as u64;
                if let Ok(parsed) = parse_event(&line, None, false, &Value::Null)
                    && progress_due(parsed.event, run.offset, recorded)
                {
                    let wait = parsed.wait;
                    let changed = previous_wait != wait;
                    let signal = progress_signal(wait.is_some(), changed);
                    at(
                        "store",
                        store.record_worker_progress(
                            &delivery.key,
                            *number,
                            run.offset
                                .try_into()
                                .map_err(|_| Error::Invalid("worker output offset overflow"))?,
                            match parsed.event {
                                WorkerEvent::Progress {
                                    unit: ProgressUnit::ModelTurn,
                                } => "model_turn",
                                WorkerEvent::Progress {
                                    unit: ProgressUnit::ToolResult,
                                } => "tool_result",
                                WorkerEvent::Progress {
                                    unit: ProgressUnit::ExternalWait,
                                } => "external_wait",
                                _ => unreachable!("progress_due only admits progress"),
                            },
                            wait.as_ref(),
                        ),
                    )?;
                    previous_wait = wait.clone();
                    // Reattachments declare liveness, not new agent work.
                    if signal != ProgressSignal::Progress {
                        if signal == ProgressSignal::Look {
                            send(
                                sender,
                                Input::Look {
                                    key: delivery.key.clone(),
                                },
                            )?;
                        }
                        continue;
                    }
                    send(
                        sender,
                        Input::Observation {
                            key: format!("{}:progress:{number}:{}", delivery.key, run.offset),
                            observation: Observation::Progress {
                                key: delivery.key.clone(),
                                checkpoint: format!("{number}:{}", run.offset),
                            },
                        },
                    )?;
                }
            }
        }
        let ended = at(
            "inspect_recorded",
            inspect_recorded(attempt, delivery, store),
        )?;
        let decision = end_decision(match &ended {
            Reconciliation::Found(_) => Ending::Found,
            Reconciliation::Absent => Ending::Absent,
            Reconciliation::Pending => Ending::Pending,
        });
        if decision != EndDecision::KeepActive {
            self.ended.push(attempt.clone());
        }
        match decision {
            EndDecision::Complete => {
                let Reconciliation::Found(completion) = ended else {
                    unreachable!("completion decision")
                };
                for (index, observation) in completion.observations.into_iter().enumerate() {
                    send(
                        sender,
                        Input::Observation {
                            key: format!("{}:worker:{number}:{index}", delivery.key),
                            observation,
                        },
                    )?;
                }
                Ok(false)
            }
            EndDecision::Retire => Ok(false),
            EndDecision::KeepActive => {
                self.active.add(delivery, *number);
                Ok(true)
            }
        }
    }
}
fn parse_event(
    line: &[u8],
    role: Option<&str>,
    postmortem: bool,
    inputs: &Value,
) -> std::result::Result<ParsedEvent, serde_json::Error> {
    let value = super::json::parse(line)?;
    Ok(decode_event(value, role, postmortem, inputs))
}
fn decode_event(value: Value, role: Option<&str>, postmortem: bool, inputs: &Value) -> ParsedEvent {
    let mut wait = None;
    let mut model = None;
    let size = value.as_object().map_or(0, |o| o.len());
    let event = if value["event"] == "progress" && size == 2 && value["unit"] == "model_turn" {
        WorkerEvent::Progress {
            unit: ProgressUnit::ModelTurn,
        }
    } else if value["event"] == "progress" && size == 2 && value["unit"] == "tool_result" {
        WorkerEvent::Progress {
            unit: ProgressUnit::ToolResult,
        }
    } else if value["event"] == "progress"
        && size == 3
        && value["unit"] == "external_wait"
        && external_wait(&value).is_some()
    {
        wait = external_wait(&value);
        WorkerEvent::Progress {
            unit: ProgressUnit::ExternalWait,
        }
    } else if size == 3 + usize::from(value.get("model").is_some())
        && value["event"] == "quota"
        && value["role"].as_str() == role
        && role.is_some()
    {
        let field = match value.get("model") {
            None => ModelField::Missing,
            Some(name) if *name == inputs["grade_model"] && name.is_string() => {
                model = name.as_str().map(str::to_owned);
                ModelField::GradeModel
            }
            Some(_) => ModelField::Other,
        };
        WorkerEvent::Quota {
            model: quota_model(postmortem, field),
            reset_ms: value["reset_ms"].as_u64(),
        }
    } else if size == 3
        && value["event"] == "reply"
        && value["role"].as_str() == role
        && role.is_some()
    {
        WorkerEvent::Reply
    } else {
        WorkerEvent::Invalid
    };
    ParsedEvent {
        event,
        value,
        wait,
        model,
    }
}
fn external_wait(value: &Value) -> Option<tl3_store::ExternalWait> {
    let wait: tl3_store::ExternalWait = serde_json::from_value(value.get("wait")?.clone()).ok()?;
    let owner_valid = wait.upstream == "prismabuild";
    let key_valid =
        !wait.key.is_empty() && wait.key.len() <= 256 && !wait.key.chars().any(char::is_control);
    (owner_valid
        && key_valid
        && (wait.upstream != "prismabuild"
            || wait.key.parse::<tl3_prismabuild::ActionKey>().is_ok()))
    .then_some(wait)
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

pub(super) fn refresh_run(source: &mut WorkerOutputSource, run: &mut Run) -> Result<bool> {
    Shell(source).refresh(run, &mut Store, &Sender(std::marker::PhantomData))
}
