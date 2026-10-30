use super::{
    ActiveRuns, inspect, latest,
    process::{self, read},
    protocol, remove_ready, run_root, watched_deliveries,
};
use crate::{
    Error, Result,
    cli::Input,
    executor::{Delivery, Observation, Reconciliation},
};
use rustix::{
    event::{PollFd, PollFlags, poll},
    fs::inotify,
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{BufRead, BufReader, Seek, SeekFrom},
    mem::MaybeUninit,
    os::fd::OwnedFd,
    path::PathBuf,
    sync::mpsc::Sender,
};
use tl3_kernel::{EffectKind, Out};
use tl3_store::Store;

struct Run {
    root: PathBuf,
    delivery: Delivery,
    attempt: Option<(i64, PathBuf)>,
    watches: Vec<i32>,
    offset: u64,
    processes: BTreeMap<&'static str, OwnedFd>,
}

/// Sources report evidence through ingress. Only the receiver owns Controller.
pub struct WorkerOutputSource {
    root: PathBuf,
    settings: tl3_config::Store,
    active: ActiveRuns,
    inotify: Option<OwnedFd>,
    root_watch: i32,
    overflows: u64,
    runs: BTreeMap<PathBuf, Run>,
    watches: BTreeMap<i32, PathBuf>,
}

impl crate::ingress::Source for WorkerOutputSource {
    fn run(self, sender: Sender<Input>) -> Result<()> {
        WorkerOutputSource::run(self, sender)
    }
}

fn io<T>(result: rustix::io::Result<T>) -> Result<T> {
    result.map_err(|error| std::io::Error::from(error).into())
}

impl WorkerOutputSource {
    pub(super) fn new(root: PathBuf, settings: tl3_config::Store, active: ActiveRuns) -> Self {
        Self {
            root,
            settings,
            active,
            inotify: None,
            root_watch: -1,
            overflows: 0,
            runs: BTreeMap::new(),
            watches: BTreeMap::new(),
        }
    }

    pub fn active_runs(&self) -> usize {
        self.runs.len()
    }

    /// Number of kernel queue overflows recovered by a rescan.
    pub fn recovered_overflows(&self) -> u64 {
        self.overflows
    }

    fn watch(&self, path: &std::path::Path) -> Result<i32> {
        io(inotify::add_watch(
            self.inotify.as_ref().unwrap(),
            path,
            inotify::WatchFlags::CREATE
                | inotify::WatchFlags::MOVED_TO
                | inotify::WatchFlags::MODIFY
                | inotify::WatchFlags::CLOSE_WRITE,
        ))
    }

    fn add(&mut self, root: PathBuf, delivery: Delivery) -> Result<()> {
        if !self.runs.contains_key(&root) {
            let wd = self.watch(&root)?;
            self.watches.insert(wd, root.clone());
            self.runs.insert(
                root.clone(),
                Run {
                    root,
                    delivery,
                    attempt: None,
                    watches: vec![wd],
                    offset: 0,
                    processes: BTreeMap::new(),
                },
            );
        }
        Ok(())
    }

    fn retire(&mut self, run: Run) -> Result<()> {
        if let Some((number, _)) = &run.attempt {
            self.active.remove(&run.delivery, *number);
        }
        for wd in run.watches {
            self.watches.remove(&wd);
            match inotify::remove_watch(self.inotify.as_ref().unwrap(), wd) {
                Ok(()) | Err(rustix::io::Errno::INVAL) => {}
                Err(error) => return Err(std::io::Error::from(error).into()),
            }
        }
        Ok(())
    }

    fn initialize(&mut self, store: &mut Store) -> Result<BTreeSet<PathBuf>> {
        if self.inotify.is_some() {
            return Ok(BTreeSet::new());
        }
        self.inotify = Some(io(inotify::init(
            inotify::CreateFlags::CLOEXEC | inotify::CreateFlags::NONBLOCK,
        ))?);
        self.root_watch = self.watch(&self.root.join(".ready"))?;
        self.rescan(store, true)?;
        Ok(self.runs.keys().cloned().collect())
    }

    pub fn run(mut self, sender: Sender<Input>) -> Result<()> {
        let mut store = Store::open(&self.settings)?;
        loop {
            self.poll_once(&mut store, &sender)?;
            // inotify and pidfds are the only wakeups. There is no timeout.
            self.wait(None)?;
        }
    }

    fn wait(&self, timeout: Option<&rustix::event::Timespec>) -> Result<BTreeSet<PathBuf>> {
        let mut descriptors = vec![PollFd::new(self.inotify.as_ref().unwrap(), PollFlags::IN)];
        let mut owners = Vec::new();
        for (root, run) in &self.runs {
            for fd in run.processes.values() {
                descriptors.push(PollFd::new(fd, PollFlags::IN));
                owners.push(root.clone());
            }
        }
        loop {
            match poll(&mut descriptors, timeout) {
                Ok(_) => break,
                Err(rustix::io::Errno::INTR) => continue,
                Err(error) => return Err(std::io::Error::from(error).into()),
            }
        }
        Ok(owners
            .into_iter()
            .zip(descriptors.iter().skip(1))
            .filter(|(_, fd)| !fd.revents().is_empty())
            .map(|(root, _)| root)
            .collect())
    }

    /// Nonblocking event drain for simulations which drive Controller directly.
    pub fn poll_once(&mut self, store: &mut Store, sender: &Sender<Input>) -> Result<()> {
        let startup = self.inotify.is_none();
        let mut dirty = self.initialize(store)?;
        dirty.extend(self.wait(Some(&rustix::event::Timespec {
            tv_sec: 0,
            tv_nsec: 0,
        }))?);
        let mut ready_names = BTreeSet::new();
        let mut overflow = false;
        let mut buffer = [MaybeUninit::uninit(); 16384];
        let mut reader = inotify::Reader::new(self.inotify.as_ref().unwrap(), &mut buffer);
        loop {
            match reader.next() {
                Ok(event) => {
                    overflow |= event.events().contains(inotify::ReadFlags::QUEUE_OVERFLOW);
                    if event.events().contains(inotify::ReadFlags::IGNORED)
                        && let Some(root) = self.watches.get(&event.wd())
                    {
                        dirty.insert(root.clone());
                    }
                    let Some(name) = event.file_name() else {
                        continue;
                    };
                    let name = name.to_string_lossy();
                    if event.wd() == self.root_watch {
                        if ready_name(&name) {
                            ready_names.insert(name.into_owned());
                        }
                    } else if relevant_file(&name)
                        && let Some(root) = self.watches.get(&event.wd())
                    {
                        dirty.insert(root.clone());
                    }
                }
                Err(rustix::io::Errno::AGAIN) => break,
                Err(rustix::io::Errno::INTR) => continue,
                Err(error) => return Err(std::io::Error::from(error).into()),
            }
        }
        if overflow {
            self.overflows += 1;
            self.rescan(store, false)?;
            dirty.extend(self.runs.keys().cloned());
        }
        for name in ready_names {
            if let Some(root) = self.ready(&name, store)? {
                dirty.insert(root);
            }
        }
        for root in dirty {
            let Some(mut run) = self.runs.remove(&root) else {
                continue;
            };
            let keep = match self.refresh(&mut run, store, sender) {
                Ok(keep) => keep,
                Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => false,
                Err(error @ Error::Json(_))
                | Err(error @ Error::Invalid("worker record belongs to another machine"))
                    if startup =>
                {
                    eprintln!("worker source skipped {}: {error}", root.display());
                    false
                }
                Err(error) => return Err(error),
            };
            if keep {
                self.runs.insert(root, run);
            } else {
                self.retire(run)?;
            }
        }
        Ok(())
    }

    fn ready(&mut self, name: &str, store: &mut Store) -> Result<Option<PathBuf>> {
        if !ready_name(name) {
            return Ok(None);
        }
        let path = self.root.join(".ready").join(name);
        let result: Result<Option<PathBuf>> = (|| {
            let Some(value) = read(&path)? else {
                return Ok(None);
            };
            let delivery: Delivery = serde_json::from_value(value)?;
            if !store.worker_reconcilable(&delivery.key)? {
                return Ok(None);
            }
            let root = run_root(&self.root, &delivery.key);
            if !root.try_exists()? {
                return Ok(None);
            }
            if let Some(previous) = self.runs.remove(&root) {
                self.retire(previous)?;
            }
            self.add(root.clone(), delivery)?;
            Ok(Some(root))
        })();
        let root = result?;
        remove_ready(&path)?;
        Ok(root)
    }

    fn rescan(&mut self, store: &mut Store, startup: bool) -> Result<()> {
        for delivery in watched_deliveries(store)? {
            let root = run_root(&self.root, &delivery.key);
            let name = root.file_name().unwrap().to_string_lossy();
            remove_ready(&self.root.join(format!(".ready-{name}")))?;
            if root.try_exists()? {
                self.add(root, delivery)?;
            }
        }
        // Read only the notification inbox, never the historical run directories.
        for entry in fs::read_dir(self.root.join(".ready"))? {
            let name = entry?.file_name();
            let name = name.to_string_lossy();
            match self.ready(&name, store) {
                Err(error @ Error::Json(_)) if startup => {
                    eprintln!("worker source skipped notification {name}: {error}");
                    remove_ready(&self.root.join(".ready").join(name.as_ref()))?;
                }
                result => {
                    result?;
                }
            }
        }
        Ok(())
    }

    fn refresh(
        &mut self,
        run: &mut Run,
        store: &mut Store,
        sender: &Sender<Input>,
    ) -> Result<bool> {
        if !store.worker_reconcilable(&run.delivery.key)? {
            return Ok(false);
        }
        if run.attempt.is_none() {
            run.attempt = latest(&run.root)?;
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
        let stdout = match File::open(attempt.join("stdout.jsonl")) {
            Ok(file) => Some(file),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        let recorded = store
            .worker_progress(Some(&delivery.node().0.to_string()))?
            .into_iter()
            .find(|row| row.key == delivery.key && row.attempt == *number)
            .map_or(0, |row| row.output_offset as u64);
        if let Some(file) = stdout {
            let mut reader = BufReader::new(file);
            reader.seek(SeekFrom::Start(run.offset))?;
            loop {
                let mut line = Vec::new();
                let n = reader.read_until(b'\n', &mut line)?;
                if n == 0 || !line.ends_with(b"\n") {
                    break;
                }
                run.offset += n as u64;
                if let Ok(value) = super::json::parse(&line)
                    && progress(&value)
                    && run.offset > recorded
                {
                    store.record_worker_progress(
                        &delivery.key,
                        *number,
                        run.offset
                            .try_into()
                            .map_err(|_| Error::Invalid("worker output offset overflow"))?,
                        value["unit"].as_str().unwrap(),
                        external_wait(&value).as_ref(),
                    )?;
                    // Reattachments declare liveness, not new agent work.
                    if external_wait(&value).is_some() {
                        sender
                            .send(Input::Look {
                                key: delivery.key.clone(),
                            })
                            .map_err(|_| Error::Invalid("worker ingress receiver closed"))?;
                        continue;
                    }
                    sender
                        .send(Input::Observation {
                            key: format!("{}:progress:{number}:{}", delivery.key, run.offset),
                            observation: Observation::Progress {
                                key: delivery.key.clone(),
                                checkpoint: format!("{number}:{}", run.offset),
                            },
                        })
                        .map_err(|_| Error::Invalid("worker ingress receiver closed"))?;
                }
            }
        }
        match inspect(attempt, delivery)? {
            Reconciliation::Found(completion) => {
                for (index, observation) in completion.observations.into_iter().enumerate() {
                    sender
                        .send(Input::Observation {
                            key: format!("{}:worker:{number}:{index}", delivery.key),
                            observation,
                        })
                        .map_err(|_| Error::Invalid("worker ingress receiver closed"))?;
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
        if value.as_object().is_some_and(|o| o.len() == 3)
            && value["event"] == "quota"
            && value["role"] == role
        {
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
            });
        }
        if !value.as_object().is_some_and(|o| o.len() == 3)
            || value["event"] != "reply"
            || value["role"] != role
        {
            return Err(Error::Invalid("invalid worker event"));
        }
        protocol::map_reply(role, &value["reply"])?;
        protocol::validate_forecast_settings(&value["reply"], &inputs["forecast"])?;
        reply = Some(value["reply"].clone());
    }
    Ok(Observation::WorkerReply {
        node: delivery.node(),
        generation: delivery.generation,
        role: role.into(),
        reply: reply.ok_or(Error::Invalid("worker reply is missing"))?,
    })
}

fn relevant_file(name: &str) -> bool {
    matches!(
        name,
        "claim.json" | "process.json" | "stdout.jsonl" | "exit.json"
    )
}

fn ready_name(name: &str) -> bool {
    name.strip_prefix(".ready-")
        .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_hexdigit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declared_wait_protocol_accepts_only_readable_owners() {
        let event = serde_json::json!({"event":"progress", "unit":"external_wait",
            "wait":{"upstream":"prismabuild", "key":"a".repeat(64)}});
        assert!(progress(&event));
        let mut token = event.clone();
        token["wait"] = serde_json::json!({"upstream":"token_grant", "key":"request:7"});
        assert!(!progress(&token));
        for change in [
            serde_json::json!({"upstream":"prismabuild", "key":"invalid"}),
            serde_json::json!({"upstream":"token_grant", "key":""}),
            serde_json::json!({"upstream":"token_grant", "key":"valid", "extra":1}),
        ] {
            let mut invalid = event.clone();
            invalid["wait"] = change;
            assert!(!progress(&invalid));
        }
        let mut invalid = event.clone();
        invalid["unexpected"] = serde_json::json!(true);
        assert!(!progress(&invalid));
        let mut checkpoint = event;
        checkpoint["checkpoint"] = serde_json::json!("7 tests");
        assert!(!progress(&checkpoint));
    }

    #[test]
    fn only_evidence_files_dirty_an_attempt() {
        for name in ["claim.json", "process.json", "stdout.jsonl", "exit.json"] {
            assert!(relevant_file(name));
        }
        for name in [
            "request.json",
            "stderr.log",
            "supervisor.py",
            "exit.json.tmp",
            "noise",
        ] {
            assert!(!relevant_file(name));
        }
        assert!(ready_name(".ready-0123abcdef"));
        assert!(!ready_name(".ready-"));
        assert!(!ready_name(".ready-0123.42-1.tmp"));
    }
}
