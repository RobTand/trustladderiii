use super::{
    ActiveRuns, inspect, latest,
    process::{self, read},
    protocol, run_root,
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
    delivery: Option<Delivery>,
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
    runs: BTreeMap<PathBuf, Run>,
    watches: BTreeMap<i32, PathBuf>,
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
            runs: BTreeMap::new(),
            watches: BTreeMap::new(),
        }
    }

    pub fn active_runs(&self) -> usize {
        self.runs.len()
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

    fn add(&mut self, root: PathBuf) -> Result<()> {
        if !self.runs.contains_key(&root) {
            let wd = self.watch(&root)?;
            self.watches.insert(wd, root.clone());
            self.runs.insert(
                root.clone(),
                Run {
                    root,
                    delivery: None,
                    attempt: None,
                    watches: vec![wd],
                    offset: 0,
                    processes: BTreeMap::new(),
                },
            );
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
        self.root_watch = self.watch(&self.root)?;
        let snapshot = store.load()?;
        let watched: BTreeSet<_> = snapshot
            .rows
            .iter()
            .filter(|r| r.table == "watches")
            .map(|r| r.id.as_str())
            .collect();
        // Rebuild once. Finished history never enters the steady-state watched set.
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let root = entry.path();
            if let Some(value) = read(&root.join("delivery.json"))? {
                let delivery: Delivery = serde_json::from_value(value)?;
                if !watched.contains(delivery.key.as_str())
                    && store
                        .effect(&delivery.key)?
                        .is_some_and(|e| !e.state.is_pending())
                {
                    continue;
                }
            }
            self.add(root)?;
        }
        Ok(self.runs.keys().cloned().collect())
    }

    pub fn run(mut self, sender: Sender<Input>) -> Result<()> {
        let mut store = Store::open(&self.settings)?;
        loop {
            if !self.poll_once(&mut store, &sender)? {
                return Ok(());
            }
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
    pub fn poll_once(&mut self, store: &mut Store, sender: &Sender<Input>) -> Result<bool> {
        let mut dirty = self.initialize(store)?;
        dirty.extend(self.wait(Some(&rustix::event::Timespec {
            tv_sec: 0,
            tv_nsec: 0,
        }))?);
        let mut events = Vec::new();
        let mut buffer = [MaybeUninit::uninit(); 16384];
        let mut reader = inotify::Reader::new(self.inotify.as_ref().unwrap(), &mut buffer);
        loop {
            match reader.next() {
                Ok(event) => events.push((
                    event.wd(),
                    event.events(),
                    event.file_name().map(|n| n.to_string_lossy().into_owned()),
                )),
                Err(rustix::io::Errno::AGAIN) => break,
                Err(rustix::io::Errno::INTR) => continue,
                Err(error) => return Err(std::io::Error::from(error).into()),
            }
        }
        for (wd, flags, name) in events {
            if flags.contains(inotify::ReadFlags::QUEUE_OVERFLOW) {
                return Err(Error::Invalid(
                    "worker inotify queue overflow; restart source to rebuild active runs",
                ));
            }
            if wd == self.root_watch {
                if let Some(name) = name {
                    let path = self.root.join(&name);
                    if name.starts_with(".ready-")
                        && name[7..].bytes().all(|b| b.is_ascii_hexdigit())
                    {
                        if let Some(value) = read(&path)? {
                            let delivery: Delivery = serde_json::from_value(value)?;
                            let root = run_root(&self.root, &delivery.key);
                            self.add(root.clone())?;
                            dirty.insert(root);
                            fs::remove_file(path)?;
                        }
                    } else if !name.starts_with('.') && path.is_dir() {
                        self.add(path.clone())?;
                        dirty.insert(path);
                    }
                }
            } else if let Some(root) = self.watches.get(&wd) {
                dirty.insert(root.clone());
            }
        }
        for root in dirty {
            let Some(mut run) = self.runs.remove(&root) else {
                continue;
            };
            let keep = self.refresh(&mut run, store, sender)?;
            if keep {
                self.runs.insert(root, run);
            } else {
                if let Some(delivery) = &run.delivery {
                    self.active.remove(delivery);
                }
                for wd in run.watches {
                    self.watches.remove(&wd);
                    io(inotify::remove_watch(self.inotify.as_ref().unwrap(), wd))?;
                }
            }
        }
        Ok(true)
    }

    fn refresh(
        &mut self,
        run: &mut Run,
        store: &mut Store,
        sender: &Sender<Input>,
    ) -> Result<bool> {
        if run.delivery.is_none() {
            run.delivery = read(&run.root.join("delivery.json"))?
                .map(serde_json::from_value)
                .transpose()?;
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
        let (Some(delivery), Some((number, attempt))) = (&run.delivery, &run.attempt) else {
            return Ok(true);
        };
        // A directory or cancelled claim is not evidence of a run that started.
        let claim = read(&attempt.join("claim.json"))?;
        if claim.as_ref().is_some_and(|c| c["cancelled"] == true) {
            return Ok(false);
        }
        if claim.is_none() {
            return Ok(true);
        }
        for name in ["claim.json", "process.json"] {
            if let Some(fd) = process::pidfd(&attempt.join(name))? {
                run.processes.insert(name, fd);
            } else {
                run.processes.remove(name);
            }
        }
        if let Ok(file) = File::open(attempt.join("stdout.jsonl")) {
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
                {
                    store.record_worker_progress(
                        &delivery.key,
                        *number,
                        run.offset
                            .try_into()
                            .map_err(|_| Error::Invalid("worker output offset overflow"))?,
                        value["unit"].as_str().unwrap(),
                    )?;
                }
            }
        }
        match inspect(&run.root, delivery)? {
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
                self.active.add(delivery);
                Ok(true)
            }
        }
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
