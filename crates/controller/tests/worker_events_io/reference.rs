// Function bodies are checked against their pinned source by the harness.
use super::io::*;
use super::*;
impl WorkerOutputSource {
    fn refresh(
        &mut self,
        run: &mut Run,
        store: &mut Store,
        sender: &Sender<Input>,
    ) -> Result<bool> {
        if !at("store", store.worker_reconcilable(&run.delivery.key))? {
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
            && read(&attempt.join("claim.json"))?.is_some_and(|c| c["cancelled"] == true)
        {
            return Ok(false);
        }
        let Some((number, attempt)) = &run.attempt else {
            return Ok(true);
        };
        let delivery = &run.delivery;
        // A directory or cancelled claim is not evidence of a run that started.
        if read(&attempt.join("claim.json"))?.is_none() {
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
                if let Ok(value) = super::json::parse(&line)
                    && progress(&value)
                    && run.offset > recorded
                {
                    let wait = external_wait(&value);
                    let changed = previous_wait != wait;
                    at(
                        "store",
                        store.record_worker_progress(
                            &delivery.key,
                            *number,
                            run.offset
                                .try_into()
                                .map_err(|_| Error::Invalid("worker output offset overflow"))?,
                            value["unit"].as_str().unwrap(),
                            wait.as_ref(),
                        ),
                    )?;
                    previous_wait = wait.clone();
                    // Reattachments declare liveness, not new agent work.
                    if wait.is_some() {
                        if changed {
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
        if !matches!(ended, Reconciliation::Pending) {
            self.ended.push(attempt.clone());
        }
        match ended {
            Reconciliation::Found(completion) => {
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
            Reconciliation::Absent => Ok(false),
            Reconciliation::Pending => {
                self.active.add(delivery, *number);
                Ok(true)
            }
        }
    }
}
fn progress(value: &Value) -> bool {
    value["event"] == "progress"
        && ((value.as_object().is_some_and(|o| o.len() == 2)
            && matches!(value["unit"].as_str(), Some("model_turn" | "tool_result")))
            || (value.as_object().is_some_and(|o| o.len() == 3)
                && value["unit"] == "external_wait"
                && external_wait(value).is_some()))
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
        let value: Value = super::json::parse(line)?;
        if reply.is_some() {
            return Err(Error::Invalid("event after worker reply"));
        }
        if progress(&value) {
            continue;
        }
        // #759: a postmortem's grade judges run on the case file's grade
        // model, and only that model may be named as the one to charge.
        let charged = match value.get("model") {
            None => Some(None),
            Some(model) if delivery.is_postmortem() && *model == inputs["grade_model"] => {
                model.as_str().map(|model| Some(model.to_owned()))
            }
            Some(_) => None,
        };
        if value
            .as_object()
            .is_some_and(|o| o.len() == 3 + usize::from(value.get("model").is_some()))
            && value["event"] == "quota"
            && value["role"] == role
        {
            let model = charged.ok_or(Error::Invalid("invalid quota model"))?;
            let reset_ms = value["reset_ms"]
                .as_u64()
                .filter(|v| *v > 0)
                .ok_or(Error::Invalid("invalid quota reset"))?;
            // A quota ending is terminal and must be the final protocol event.
            if bytes.split_inclusive(|b| *b == b'\n').next_back() != Some(line) {
                return Err(Error::Invalid("event after quota ending"));
            }
            return Ok(Observation::Quota {
                key: delivery.key.clone(),
                reset_ms,
                model,
            });
        }
        if !value.as_object().is_some_and(|o| o.len() == 3)
            || value["event"] != "reply"
            || value["role"] != role
        {
            return Err(Error::Invalid("invalid worker event"));
        }
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
    source.refresh(run, &mut Store, &Sender(std::marker::PhantomData))
}
