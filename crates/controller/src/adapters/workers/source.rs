use super::{failed, inspect, latest, process::read, protocol};
use crate::{
    Error, Result,
    cli::Input,
    executor::{Delivery, Observation, Reconciliation},
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{BufRead, BufReader, Seek, SeekFrom},
    path::PathBuf,
    sync::mpsc::Sender,
    thread,
    time::Duration,
};
use tl3_kernel::{EffectKind, Out};
use tl3_store::{Store, WorkerProgress};

/// An ingress source. It writes liveness through Store and sends Input values.
/// The controller remains owned by its receiving thread.
pub struct WorkerOutputSource {
    root: PathBuf,
    settings: tl3_config::Store,
    offsets: BTreeMap<PathBuf, u64>,
    sent: BTreeSet<(String, i64)>,
}

impl WorkerOutputSource {
    pub(super) fn new(root: PathBuf, settings: tl3_config::Store) -> Self {
        Self {
            root,
            settings,
            offsets: BTreeMap::new(),
            sent: BTreeSet::new(),
        }
    }

    pub fn run(mut self, sender: Sender<Input>) -> Result<()> {
        let mut store = Store::open(&self.settings)?;
        loop {
            if !self.poll_once(&mut store, &sender)? {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(25));
        }
    }

    /// Also usable by a simulation that drives Controller directly before #10.
    pub fn poll_once(&mut self, store: &mut Store, sender: &Sender<Input>) -> Result<bool> {
        for entry in fs::read_dir(&self.root)? {
            let root = entry?.path();
            let Some(value) = read(&root.join("delivery.json"))? else {
                continue;
            };
            let delivery: Delivery = serde_json::from_value(value)?;
            let Some((number, attempt)) = latest(&root)? else {
                continue;
            };
            let path = attempt.join("stdout.jsonl");
            if let Ok(file) = File::open(&path) {
                let offset = self.offsets.entry(path).or_default();
                let mut reader = BufReader::new(file);
                reader.seek(SeekFrom::Start(*offset))?;
                loop {
                    let mut line = Vec::new();
                    let n = reader.read_until(b'\n', &mut line)?;
                    if n == 0 || !line.ends_with(b"\n") {
                        break;
                    }
                    *offset += n as u64;
                    if let Ok(value) = super::json::parse(&line)
                        && progress(&value)
                    {
                        store.record_worker_progress(&WorkerProgress {
                            key: delivery.key.clone(),
                            node: delivery.node().0.to_string(),
                            epoch: delivery.epoch,
                            attempt: number,
                            output_offset: (*offset)
                                .try_into()
                                .map_err(|_| Error::Invalid("worker output offset overflow"))?,
                            unit: value["unit"].as_str().unwrap().into(),
                            last_progress_ms: 0,
                        })?;
                    }
                }
            }
            if self.sent.contains(&(delivery.key.clone(), number))
                || read(&attempt.join("claim.json"))?.is_none()
            {
                continue;
            }
            let completion = match inspect(&root, &delivery)? {
                Reconciliation::Found(completion) => completion,
                Reconciliation::Absent => failed(&delivery),
                Reconciliation::Pending => continue,
            };
            for (index, observation) in completion.observations.into_iter().enumerate() {
                if sender
                    .send(Input::Observation {
                        key: format!("{}:worker:{number}:{index}", delivery.key),
                        observation,
                    })
                    .is_err()
                {
                    return Ok(false);
                }
            }
            self.sent.insert((delivery.key, number));
        }
        Ok(true)
    }
}

fn progress(value: &Value) -> bool {
    value.as_object().is_some_and(|o| o.len() == 2)
        && value["event"] == "progress"
        && matches!(value["unit"].as_str(), Some("model_turn" | "tool_result"))
}

pub(super) fn parse_reply(bytes: &[u8], delivery: &Delivery) -> Result<Observation> {
    if !bytes.ends_with(b"\n") {
        return Err(Error::Invalid("incomplete worker output"));
    }
    let role = match delivery.output {
        Out::Effect {
            kind: EffectKind::StartRun(role),
            ..
        } => protocol::role(role).ok_or(Error::Invalid("unsupported worker role"))?,
        _ => return Err(Error::Invalid("worker output without start")),
    };
    let mut reply = None;
    for line in bytes.split_inclusive(|b| *b == b'\n') {
        let value: Value = super::json::parse(line)?;
        if reply.is_some() {
            return Err(Error::Invalid("event after worker reply"));
        }
        if progress(&value) {
            continue;
        }
        if !value.as_object().is_some_and(|o| o.len() == 3)
            || value["event"] != "reply"
            || value["role"] != role
        {
            return Err(Error::Invalid("invalid worker event"));
        }
        protocol::map_reply(role, &value["reply"])?;
        reply = Some(value["reply"].clone());
    }
    Ok(Observation::WorkerReply {
        node: delivery.node(),
        generation: delivery.generation,
        role: role.into(),
        reply: reply.ok_or(Error::Invalid("worker reply is missing"))?,
    })
}
