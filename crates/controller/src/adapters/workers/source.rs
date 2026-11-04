use super::{
    ActiveRuns, inspect_recorded, latest, process, protocol, remove_ready, run_root,
    source_error::{at, cause},
    trace, watched_deliveries,
};
use crate::sync::mpsc::Sender;
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
};
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
    /// #430: span logs of ended attempts, and held logs of retired runs, are
    /// flushed on events: tl3d opening, attempts ending, and writes to the
    /// held directory, which a shipper makes when MLflow answers again.
    launcher: Vec<String>,
    python: PathBuf,
    held_watch: i32,
    held_dirty: bool,
    ended: Vec<PathBuf>,
}

impl crate::ingress::Source for WorkerOutputSource {
    fn run(self, sender: Sender<Input>) -> Result<()> {
        WorkerOutputSource::run(self, sender)
    }
}

fn io<T>(boundary: &'static str, result: rustix::io::Result<T>) -> Result<T> {
    at(boundary, result.map_err(std::io::Error::from))
}

fn read(path: &std::path::Path) -> Result<Option<Value>> {
    at("read", process::read(path))
}

fn send(sender: &Sender<Input>, input: Input) -> Result<()> {
    at(
        "ingress",
        sender
            .send(input)
            .map_err(|_| Error::Invalid("worker ingress receiver closed")),
    )
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
            launcher: Vec::new(),
            python: PathBuf::new(),
            held_watch: -1,
            held_dirty: false,
            ended: Vec::new(),
        }
    }

    pub(super) fn with_flusher(mut self, launcher: Vec<String>, python: PathBuf) -> Self {
        self.launcher = launcher;
        self.python = python;
        self
    }

    pub fn active_runs(&self) -> usize {
        self.runs.len()
    }

    /// Number of kernel queue overflows recovered by a rescan.
    pub fn recovered_overflows(&self) -> u64 {
        self.overflows
    }

    fn watch(&self, path: &std::path::Path) -> Result<i32> {
        io(
            "inotify_add_watch",
            inotify::add_watch(
                self.inotify.as_ref().unwrap(),
                path,
                inotify::WatchFlags::CREATE
                    | inotify::WatchFlags::MOVED_TO
                    | inotify::WatchFlags::MODIFY
                    | inotify::WatchFlags::CLOSE_WRITE,
            ),
        )
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
            match io(
                "inotify_remove_watch",
                inotify::remove_watch(self.inotify.as_ref().unwrap(), wd),
            ) {
                Ok(()) => {}
                Err(error)
                    if matches!(cause(&error), Error::Io(io)
                    if io.raw_os_error() == Some(rustix::io::Errno::INVAL.raw_os_error())) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    fn initialize(&mut self, store: &mut Store) -> Result<BTreeSet<PathBuf>> {
        if self.inotify.is_some() {
            return Ok(BTreeSet::new());
        }
        self.inotify = Some(io(
            "inotify_init",
            inotify::init(inotify::CreateFlags::CLOEXEC | inotify::CreateFlags::NONBLOCK),
        )?);
        self.root_watch = self.watch(&self.root.join(".ready"))?;
        at("mkdir", fs::create_dir_all(self.root.join(trace::HELD)))?;
        self.held_watch = self.watch(&self.root.join(trace::HELD))?;
        self.held_dirty = true;
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
            match io("ppoll", poll(&mut descriptors, timeout)) {
                Ok(_) => break,
                Err(error)
                    if matches!(cause(&error), Error::Io(io)
                    if io.kind() == std::io::ErrorKind::Interrupted) =>
                {
                    continue;
                }
                Err(error) => return Err(error),
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
        at("poll_once", self.poll_once_inner(store, sender))
    }

    fn poll_once_inner(&mut self, store: &mut Store, sender: &Sender<Input>) -> Result<()> {
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
            match io("inotify read", reader.next()) {
                Ok(event) => {
                    overflow |= event.events().contains(inotify::ReadFlags::QUEUE_OVERFLOW);
                    if event.events().contains(inotify::ReadFlags::IGNORED)
                        && let Some(root) = self.watches.get(&event.wd())
                    {
                        dirty.insert(root.clone());
                    }
                    if event.wd() == self.held_watch {
                        self.held_dirty = true;
                        continue;
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
                Err(error)
                    if matches!(cause(&error), Error::Io(io)
                    if io.raw_os_error() == Some(rustix::io::Errno::AGAIN.raw_os_error())) =>
                {
                    break;
                }
                Err(error)
                    if matches!(cause(&error), Error::Io(io)
                    if io.kind() == std::io::ErrorKind::Interrupted) =>
                {
                    continue;
                }
                Err(error) => return Err(error),
            }
        }
        if overflow {
            self.held_dirty = true;
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
                Err(error)
                    if matches!(cause(&error), Error::Io(io)
                    if io.kind() == std::io::ErrorKind::NotFound) =>
                {
                    false
                }
                Err(error)
                    if startup
                        && matches!(
                            cause(&error),
                            Error::Json(_)
                                | Error::Invalid("worker record belongs to another machine")
                        ) =>
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
        self.sweep_traces(sender)
    }

    fn sweep_traces(&mut self, sender: &Sender<Input>) -> Result<()> {
        if !self.held_dirty && self.ended.is_empty() {
            return Ok(());
        }
        self.held_dirty = false;
        let ended = std::mem::take(&mut self.ended);
        let flusher = trace::Flusher {
            launcher: &self.launcher,
            python: &self.python,
        };
        match trace::sweep(&self.root, &flusher, &ended, trace::HELD_BYTES) {
            Ok(0) => {}
            Ok(evicted) => send(
                sender,
                Input::Observation {
                    key: format!(
                        "trace-logs-capped:{}",
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_nanos()
                    ),
                    observation: Observation::TraceLogsCapped { evicted },
                },
            )?,
            // Tracing never stops the source; the next event sweeps again.
            Err(error) => eprintln!("held span logs were not swept: {error}"),
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
            let delivery: Delivery = at("read", serde_json::from_value(value))?;
            if !at("store", store.worker_reconcilable(&delivery.key))? {
                return Ok(None);
            }
            let root = run_root(&self.root, &delivery.key);
            if !at("metadata/stat", root.try_exists())? {
                return Ok(None);
            }
            if let Some(previous) = self.runs.remove(&root) {
                self.retire(previous)?;
            }
            self.add(root.clone(), delivery)?;
            Ok(Some(root))
        })();
        let root = result?;
        at("ready_remove", remove_ready(&path))?;
        Ok(root)
    }

    fn rescan(&mut self, store: &mut Store, startup: bool) -> Result<()> {
        for delivery in at("store", watched_deliveries(store))? {
            let root = run_root(&self.root, &delivery.key);
            let name = root.file_name().unwrap().to_string_lossy();
            at(
                "ready_remove",
                remove_ready(&self.root.join(format!(".ready-{name}"))),
            )?;
            if at("metadata/stat", root.try_exists())? {
                self.add(root, delivery)?;
            }
        }
        // Read only the notification inbox, never the historical run directories.
        for entry in at("metadata/stat", fs::read_dir(self.root.join(".ready")))? {
            let name = at("metadata/stat", entry)?.file_name();
            let name = name.to_string_lossy();
            match self.ready(&name, store) {
                Err(error) if startup && matches!(cause(&error), Error::Json(_)) => {
                    eprintln!("worker source skipped notification {name}: {error}");
                    at(
                        "ready_remove",
                        remove_ready(&self.root.join(".ready").join(name.as_ref())),
                    )?;
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

    fn boundary_failure(boundary: &'static str) {
        use crate::adapters::workers::source_error::fail_next;
        use crate::git_tests::support::Database;

        let root = tempfile::tempdir().unwrap();
        let mut database = Database::start();
        let settings = database.create("source_boundary");
        let mut store = Store::open(&settings).unwrap();
        let delivery: Delivery = serde_json::from_value(serde_json::json!({
            "key":"source-boundary", "epoch":1, "generation":0,
            "output":{"Effect":{"node":0,"kind":{"StartRun":"Ingest"},"seq":1,"head":null}},
            "head_sha":null,"repository":{"owner":"RobTand","name":"tl3-sandbox","installation":1},
            "issue":42,"pull_request":null,"paths":[],"attempt":1,"round":0,"checks":null
        }))
        .unwrap();
        let run = run_root(root.path(), &delivery.key);
        let attempt = run.join("attempt-00000000");
        fs::create_dir_all(&attempt).unwrap();
        fs::create_dir(root.path().join(".ready")).unwrap();
        let stat = fs::read_to_string("/proc/self/stat").unwrap();
        let start = stat
            .rsplit_once(')')
            .unwrap()
            .1
            .split_whitespace()
            .nth(19)
            .unwrap();
        let claim = if boundary == "inotify_remove_watch" {
            serde_json::json!({"cancelled":true})
        } else {
            serde_json::json!({"pid":std::process::id(), "start":start.parse::<u64>().unwrap(),
                "machine":fs::read_to_string("/etc/machine-id").unwrap().trim(),
                "boot":fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap().trim()})
        };
        fs::write(attempt.join("claim.json"), claim.to_string()).unwrap();
        fs::write(
            attempt.join("stdout.jsonl"),
            b"{\"event\":\"progress\",\"unit\":\"model_turn\"}\n",
        )
        .unwrap();
        let name = format!(".ready-{}", run.file_name().unwrap().to_str().unwrap());
        fs::write(
            root.path().join(".ready").join(name),
            serde_json::to_vec(&delivery).unwrap(),
        )
        .unwrap();
        let mut source =
            WorkerOutputSource::new(root.path().into(), settings, ActiveRuns::default());
        let (sender, _receiver) = crate::sync::mpsc::channel();
        let _failure = fail_next(boundary, 5);
        let error = source.poll_once(&mut store, &sender).unwrap_err();
        let Error::WorkerSource(context) = &error else {
            panic!("{boundary} lost its typed context: {error:?}");
        };
        assert_eq!(context.boundary, boundary);
        assert_eq!(context.raw_os_error(), Some(5));
        assert!(error.to_string().contains(boundary));
        assert!(error.to_string().contains("errno Some(5)"));
        assert!(matches!(cause(&error), Error::Io(io) if io.raw_os_error() == Some(5)));
        drop(source);
        drop(store);
        database.stop();
    }

    macro_rules! boundary_test {
        ($name:ident, $boundary:literal) => {
            #[test]
            fn $name() {
                boundary_failure($boundary);
            }
        };
    }

    boundary_test!(source_names_inotify_init_failure, "inotify_init");
    boundary_test!(source_names_inotify_add_watch_failure, "inotify_add_watch");
    boundary_test!(
        source_names_inotify_remove_watch_failure,
        "inotify_remove_watch"
    );
    boundary_test!(source_names_directory_creation_failure, "mkdir");
    boundary_test!(source_names_store_failure, "store");
    boundary_test!(source_names_metadata_failure, "metadata/stat");
    boundary_test!(source_names_ppoll_failure, "ppoll");
    boundary_test!(source_names_inotify_read_failure, "inotify read");
    boundary_test!(source_names_read_failure, "read");
    boundary_test!(source_names_identity_read_failure, "identity_read");
    boundary_test!(source_names_pidfd_open_failure, "pidfd_open");
    boundary_test!(source_names_seek_failure, "seek");
    boundary_test!(source_names_reconciliation_failure, "inspect_recorded");
    boundary_test!(source_names_ready_remove_failure, "ready_remove");
    boundary_test!(source_names_ingress_failure, "ingress");

    #[test]
    fn source_names_real_ppoll_invalid_timeout() {
        let config =
            tl3_config::Config::parse(include_str!("../../../../config/example.toml")).unwrap();
        let mut source =
            WorkerOutputSource::new(PathBuf::new(), config.store, ActiveRuns::default());
        source.inotify = Some(inotify::init(inotify::CreateFlags::NONBLOCK).unwrap());
        let error = source
            .wait(Some(&rustix::event::Timespec {
                tv_sec: 0,
                tv_nsec: 1_000_000_000,
            }))
            .unwrap_err();
        let Error::WorkerSource(context) = &error else {
            panic!("ppoll lost its typed context: {error:?}");
        };
        assert_eq!(context.boundary, "ppoll");
        assert_eq!(context.raw_os_error(), Some(22));
    }

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

    /// #372: a run outlives the tl3d that started it, so a worker from release N
    /// may reply to the controller of release N+1. These lines are pinned as
    /// release d8e4f2a8's workers emit them; a change that rejects them breaks
    /// every run in flight across a deploy.
    #[test]
    fn replies_from_the_previous_release_still_parse() {
        let fixture = include_str!("../../../tests/fixtures/worker-replies-d8e4f2a8.jsonl");
        let mut roles = std::collections::BTreeSet::new();
        for case in fixture.lines() {
            let case: Value = serde_json::from_str(case).unwrap();
            let output = if case["start"] == "Classify" {
                serde_json::json!({"Comment": {"node": 0, "kind": "Classify"}})
            } else {
                serde_json::json!({"Effect": {"node": 0, "kind": {"StartRun": case["start"]}, "seq": 1, "head": null}})
            };
            let delivery: Delivery = serde_json::from_value(serde_json::json!({
                "key": "release-n-minus-one", "epoch": 1, "generation": 3, "output": output,
                "head_sha": null, "repository": {"owner": "RobTand", "name": "tl3-sandbox", "installation": 1},
                "issue": 42, "pull_request": null, "paths": ["greeting.py"], "attempt": 1, "round": 0, "checks": null,
            }))
            .unwrap();
            let mut stdout = String::new();
            for event in case["stdout"].as_array().unwrap() {
                stdout.push_str(&event.to_string());
                stdout.push('\n');
            }
            let observation = parse_reply(stdout.as_bytes(), &delivery, &case["inputs"])
                .unwrap_or_else(|error| panic!("{case}: {error}"));
            let last = case["stdout"].as_array().unwrap().last().unwrap();
            roles.insert(format!(
                "{}:{}",
                case["start"].as_str().unwrap(),
                last["reply"]["kind"]
                    .as_str()
                    .or(last["reply"]["verdict"].as_str())
                    .unwrap_or("quota")
            ));
            match observation {
                Observation::WorkerReply {
                    reply, generation, ..
                } => {
                    assert_eq!(reply, last["reply"]);
                    assert_eq!(generation, 3);
                }
                Observation::Classified { reply, .. } => {
                    assert_eq!(reply.as_ref(), Some(&last["reply"]))
                }
                Observation::Quota { reset_ms, .. } => {
                    assert_eq!(Some(reset_ms), last["reset_ms"].as_u64())
                }
                other => panic!("{case}: unexpected {other:?}"),
            }
        }
        // Every role and every reply kind or verdict a worker can end with.
        for expected in [
            "Make:done",
            "Make:cant_meet",
            "Make:needs_files",
            "Make:out_of_room",
            "Make:blocked",
            "Make:quota",
            "Ingest:brief",
            "Ingest:early_end",
            "Audit:agree",
            "Audit:disagree",
            "Plan:leaf",
            "Plan:split",
            "Plan:cant_meet",
            "Review:approve",
            "Review:changes",
            "Review:out_of_room",
            "Classify:remark",
            "Classify:pause",
            "Classify:resume",
            "Classify:stop",
            "Classify:restart",
            "Classify:change_request",
            "Classify:unsure",
            "Classify:complaint",
            "Classify:answer",
        ] {
            assert!(roles.contains(expected), "fixture lacks {expected}");
        }
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

#[cfg(all(test, loom))]
#[path = "../../../tests/loom/workers.rs"]
mod loom_tests;
