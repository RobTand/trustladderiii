use crate::http;
use crate::{
    Bootstrap, Controller, Error, LabelledIssue, Repository, Result, cli::Input,
    executor::Observation,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::{self, BufRead},
    net::SocketAddr,
    sync::{
        Arc,
        mpsc::{self, Sender},
    },
    thread,
};
use tl3_github::{
    GitHub, ServedRepo,
    webhook::{self, Event},
};
use tl3_store::Store;

mod webhooks;
pub use webhooks::Webhooks;

pub trait Source: Send + 'static {
    fn run(self, sender: Sender<Input>) -> Result<()>;
}

pub struct Sources {
    sender: Sender<Input>,
    receiver: mpsc::Receiver<Input>,
    exits: mpsc::Receiver<Result<()>>,
    exit_sender: Sender<Result<()>>,
}
impl Default for Sources {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        let (exit_sender, exits) = mpsc::channel();
        Self {
            sender,
            receiver,
            exits,
            exit_sender,
        }
    }
}
impl Sources {
    pub fn register(&mut self, source: impl Source) {
        let sender = self.sender.clone();
        let exits = self.exit_sender.clone();
        thread::spawn(move || {
            let name = std::any::type_name_of_val(&source);
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                source.run(sender.clone())
            }))
            .unwrap_or(Err(Error::Invalid("ingress source panicked")));
            match &result {
                Ok(()) => eprintln!("ingress source {name} exited successfully"),
                Err(error) => eprintln!("ingress source {name} failed: {error}"),
            }
            let _ = exits.send(result);
            let _ = sender.send(Input::Wake);
        });
    }
    pub fn run(self, controller: &mut Controller) -> Result<()> {
        drop(self.sender);
        drop(self.exit_sender);
        drain(controller)?;
        while controller.drive_once()? {}
        controller.look_now()?;
        while controller.drive_once()? {}
        loop {
            let input = match controller.next_look_in()? {
                Some(wait) => match self.receiver.recv_timeout(wait) {
                    Ok(input) => input,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        controller.look_due()?;
                        while controller.drive_once()? {}
                        continue;
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                },
                None => match self.receiver.recv() {
                    Ok(input) => input,
                    Err(_) => break,
                },
            };
            for exit in self.exits.try_iter() {
                exit?;
            }
            drain(controller)?;
            apply(controller, input)?;
            controller.look_due()?;
            while controller.drive_once()? {}
        }
        for exit in self.exits {
            exit?;
        }
        Ok(())
    }
}

pub struct Stdin;
impl Source for Stdin {
    fn run(self, sender: Sender<Input>) -> Result<()> {
        for line in io::stdin().lock().lines() {
            if sender.send(serde_json::from_str(&line?)?).is_err() {
                break;
            }
        }
        Ok(())
    }
}

pub fn apply(controller: &mut Controller, input: Input) -> Result<()> {
    match input {
        Input::Wake => {}
        Input::Look { key } => controller.look_key(&key)?,
        Input::LabelledIssue(issue) => {
            controller.intake(issue)?;
        }
        Input::Observation { key, observation } => controller.observe(&key, observation)?,
    }
    Ok(())
}

/// Both paths preserve upstream evidence before translating it into controller inputs.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum GitHubDelivery {
    Webhook(Box<webhook::Delivery>),
    CatchUp {
        repository: Repository,
        changes: Box<tl3_github::RepoChanges>,
    },
}

/// Translators run on the controller thread. Keys must survive duplicate evidence and replay.
pub trait GitHubTranslator {
    fn translate(
        &mut self,
        delivery: &GitHubDelivery,
        snapshot: &tl3_store::Snapshot,
    ) -> Result<Vec<Input>>;
}
pub(crate) struct Translation {
    pub(crate) name: String,
    pub(crate) translator: Box<dyn GitHubTranslator>,
}

fn translate(delivery: GitHubDelivery, bootstrap: &Bootstrap) -> Vec<Input> {
    match delivery {
        GitHubDelivery::Webhook(delivery) => inputs(&delivery.id, delivery.event, bootstrap),
        GitHubDelivery::CatchUp {
            repository,
            changes,
        } => {
            let mut result = Vec::new();
            for (number, change) in changes.labels {
                if change.event == "labeled"
                    && let Some(issue) = changes.issues.iter().find(|issue| issue.number == number)
                    && issue.pull_request.is_none()
                {
                    result.push(Input::LabelledIssue(LabelledIssue {
                        delivery: format!(
                            "catch-up-label:{}/{}:{}",
                            repository.owner, repository.name, change.id
                        ),
                        owner: repository.owner.clone(),
                        repo: repository.name.clone(),
                        installation: repository.installation,
                        number,
                        title: issue.title.clone(),
                        actor: change.actor.id,
                        label: change.label.name,
                    }));
                }
            }
            for comment in changes.comments {
                // URLs were validated before advancing the durable catch-up cursor.
                let number =
                    comment_number(&repository, &comment.issue_url).expect("validated issue URL");
                if accepts_comment(bootstrap, &repository, number) {
                    result.extend(comment_input(&repository, number, comment, bootstrap.rob));
                }
            }
            result
        }
    }
}

/// Replays use the same intake and observation keys after a crash before consumption.
pub fn drain(controller: &mut Controller) -> Result<()> {
    let names: Vec<_> = controller
        .github_translators
        .iter()
        .map(|entry| format!("github-translator:{}", entry.name))
        .collect();
    controller
        .store
        .register_ingress_translators(&names.iter().map(String::as_str).collect::<Vec<_>>())?;
    loop {
        let deliveries = controller.store.pending_ingress()?;
        if deliveries.is_empty() {
            break;
        }
        for stored in deliveries {
            if let Some(delivery) = replay(&stored) {
                for input in translate(delivery, &controller.bootstrap) {
                    apply(controller, input)?;
                }
            }
            controller.store.consume_ingress(stored.sequence)?;
        }
    }
    for index in 0..controller.github_translators.len() {
        let name = format!(
            "github-translator:{}",
            controller.github_translators[index].name
        );
        let mut cursor = controller.store.translator_cursor(&name)?;
        loop {
            let deliveries = controller.store.ingress_after(cursor)?;
            if deliveries.is_empty() {
                break;
            }
            let mut snapshot = None;
            for stored in deliveries {
                if let Some(delivery) = replay(&stored) {
                    if snapshot.is_none() {
                        snapshot = Some(controller.store.load()?);
                    }
                    let inputs = controller.github_translators[index]
                        .translator
                        .translate(&delivery, snapshot.as_ref().expect("batch snapshot"))?;
                    for input in inputs {
                        apply(controller, input)?;
                    }
                }
                controller
                    .store
                    .advance_translator(&name, stored.sequence)?;
                cursor = stored.sequence;
            }
        }
    }
    Ok(())
}

fn replay(stored: &tl3_store::Ingress) -> Option<GitHubDelivery> {
    let result = if stored.source == "github-webhook" {
        webhook::parse(&stored.delivery_key, &stored.event_name, &stored.payload)
            .map(|delivery| GitHubDelivery::Webhook(Box::new(delivery)))
            .map_err(|error| error.to_string())
    } else {
        serde_json::from_slice(&stored.payload).map_err(|error| error.to_string())
    };
    match result {
        Ok(delivery) => Some(delivery),
        Err(error) => {
            eprintln!(
                "GitHub delivery {} skipped during replay: {error}",
                stored.delivery_key
            );
            None
        }
    }
}

fn comment_number(repo: &Repository, url: &str) -> Result<u64> {
    let invalid = || Error::Invalid("catch-up comment has an invalid issue URL");
    let path = url
        .split_once("://")
        .and_then(|(_, rest)| rest.split_once('/'))
        .map(|(_, path)| path)
        .ok_or_else(invalid)?;
    let parts: Vec<_> = path.split('/').collect();
    let ["repos", owner, name, "issues", number] = parts.as_slice() else {
        return Err(invalid());
    };
    if !repo.owner.eq_ignore_ascii_case(owner) || !repo.name.eq_ignore_ascii_case(name) {
        return Err(invalid());
    }
    number
        .parse()
        .ok()
        .filter(|number| *number > 0)
        .ok_or_else(invalid)
}

pub fn run(controller: &mut Controller, receiver: mpsc::Receiver<Input>) -> Result<()> {
    Sources {
        receiver,
        ..Sources::default()
    }
    .run(controller)
}

fn inputs(id: &str, event: Option<Event>, bootstrap: &Bootstrap) -> Vec<Input> {
    match event {
        Some(Event::IssueLabeled {
            context,
            issue,
            label,
            actor,
        }) if issue.pull_request.is_none() => {
            vec![Input::LabelledIssue(LabelledIssue {
                delivery: id.into(),
                owner: context.repository.owner.login,
                repo: context.repository.name,
                installation: context.installation_id,
                number: issue.number,
                title: issue.title,
                actor: actor.id,
                label: label.name,
            })]
        }
        Some(Event::CommentCreated {
            context,
            issue,
            comment,
            ..
        }) => {
            let Some(repo) = bootstrap.github_repositories().into_iter().find(|repo| {
                repo.owner
                    .eq_ignore_ascii_case(&context.repository.owner.login)
                    && repo.name.eq_ignore_ascii_case(&context.repository.name)
                    && repo.installation == context.installation_id
            }) else {
                return vec![];
            };
            if !accepts_comment(bootstrap, &repo, issue.number) {
                return vec![];
            }
            comment_input(&repo, issue.number, comment, bootstrap.rob)
                .into_iter()
                .collect()
        }
        _ => vec![],
    }
}

fn accepts_comment(bootstrap: &Bootstrap, repo: &Repository, issue: u64) -> bool {
    bootstrap.repos.contains(repo)
        || bootstrap.operations.as_ref().is_some_and(|o| {
            o.owner.eq_ignore_ascii_case(&repo.owner)
                && o.name.eq_ignore_ascii_case(&repo.name)
                && o.installation_id == repo.installation
                && o.number == issue
        })
}

fn comment_input(
    repo: &Repository,
    issue: u64,
    comment: tl3_github::Comment,
    rob: u64,
) -> Option<Input> {
    (comment.user.id == rob).then(|| Input::Observation {
        key: format!(
            "github-comment:{}/{}/{}/{}",
            repo.owner.to_ascii_lowercase(),
            repo.name.to_ascii_lowercase(),
            repo.installation,
            comment.id
        ),
        observation: Observation::GitHubComment {
            repository: repo.clone(),
            issue,
            comment,
        },
    })
}

#[derive(Clone)]
pub struct CatchUp {
    clients: BTreeMap<u64, GitHub>,
    runtime: &'static tokio::runtime::Runtime,
}
impl CatchUp {
    pub fn new(clients: BTreeMap<u64, GitHub>) -> Result<Self> {
        Ok(Self {
            clients,
            runtime: crate::executor::runtime()?,
        })
    }
    pub fn pass(&self, store: &mut Store, bootstrap: &Bootstrap) -> Result<Vec<Input>> {
        let since: DateTime<Utc> = store
            .ingress_cursor("github-catch-up")?
            .map(serde_json::from_value)
            .transpose()?
            .unwrap_or(DateTime::UNIX_EPOCH);
        let snapshot = store.load()?;
        self.fetch(since, snapshot, bootstrap)?.persist(store)
    }
    fn fetch(
        &self,
        since: DateTime<Utc>,
        snapshot: tl3_store::Snapshot,
        bootstrap: &Bootstrap,
    ) -> Result<CatchUpBatch> {
        let runtime = &self.runtime;
        // Sources can finish their first catch-up before Controller::open initializes the store.
        let world = (!snapshot.rows.is_empty())
            .then(|| crate::snapshot::decode(&snapshot))
            .transpose()?;
        let mut deliveries = Vec::new();
        let mut cursor = None;
        for repo in &bootstrap.github_repositories() {
            let client = self
                .clients
                .get(&repo.installation)
                .ok_or(Error::Invalid("missing GitHub installation client"))?;
            let name = format!("{}/{}", repo.owner, repo.name);
            let nodes: Vec<_> = snapshot
                .rows
                .iter()
                .filter(|r| r.table == "requests")
                .filter_map(|r| {
                    serde_json::from_value::<crate::Request>(r.value.clone())
                        .ok()
                        .map(|request| (r.id.clone(), request))
                })
                .filter(|(id, request)| {
                    request.repo == *repo
                        && id
                            .parse::<usize>()
                            .ok()
                            .and_then(|index| {
                                world.as_ref().and_then(|world| world.nodes.get(index))
                            })
                            .is_some_and(|node| !matches!(node.phase, tl3_kernel::Phase::Closed(_)))
                })
                .map(|(id, _)| id)
                .collect();
            let tracked_heads = snapshot
                .interned
                .iter()
                .filter(|entry| {
                    entry.kind == tl3_store::InternKind::Commit && nodes.contains(&entry.scope)
                })
                .map(|entry| entry.value.clone())
                .collect();
            let result = runtime.block_on(client.catch_up(
                &[ServedRepo {
                    full_name: name.clone(),
                    tracked_heads,
                }],
                since,
            ))?;
            cursor = Some(cursor.map_or(result.next_cursor, |old: DateTime<Utc>| {
                old.min(result.next_cursor)
            }));
            for mut changes in result.repositories {
                for (number, _) in &changes.labels {
                    if !changes.issues.iter().any(|issue| issue.number == *number) {
                        changes
                            .issues
                            .push(runtime.block_on(client.issue(&name, *number))?);
                    }
                }
                for comment in &changes.comments {
                    comment_number(repo, &comment.issue_url)?;
                }
                let delivery = GitHubDelivery::CatchUp {
                    repository: repo.clone(),
                    changes: Box::new(changes),
                };
                let payload = serde_json::to_vec(&delivery)?;
                let key = format!("{name}:{:x}", Sha256::digest(&payload));
                deliveries.push((key, payload));
            }
        }

        Ok(CatchUpBatch { deliveries, cursor })
    }
}

struct CatchUpBatch {
    deliveries: Vec<(String, Vec<u8>)>,
    cursor: Option<DateTime<Utc>>,
}
impl CatchUpBatch {
    fn persist(self, store: &mut Store) -> Result<Vec<Input>> {
        if let Some(cursor) = self.cursor {
            store.receive_catch_up("github-catch-up", &self.deliveries, &json!(cursor))?;
        }
        Ok(self.deliveries.iter().map(|_| Input::Wake).collect())
    }
}

enum WebhookMessage {
    Request(http::Request, mpsc::SyncSender<u16>),
    Gap,
    Stop,
    ServerExit(Result<()>),
    CaughtUp(Box<tl3_store::Reader>, Result<CatchUpBatch>),
}
struct Control {
    sender: Sender<WebhookMessage>,
    server: http::Control,
}
#[derive(Clone)]
pub struct WebhookControl(Arc<Control>);
impl WebhookControl {
    /// Ticket 19 calls this after a tunnel gap, never on a timer.
    pub fn gap(&self) {
        let _ = self.0.sender.send(WebhookMessage::Gap);
    }
    pub fn stop(&self) {
        let _ = self.0.sender.send(WebhookMessage::Stop);
        self.0.server.stop();
    }
}

struct WebhookReceiver {
    control: WebhookControl,
    server: http::Server,
    receiver: mpsc::Receiver<WebhookMessage>,
    store: Store,
    bootstrap: Bootstrap,
    secret: Vec<u8>,
    catch_up: Option<Arc<dyn webhooks::Fetcher>>,
    catch_up_reader: Option<tl3_store::Reader>,
}
impl WebhookReceiver {
    pub fn bind(
        address: SocketAddr,
        store: Store,
        bootstrap: Bootstrap,
        secret: Vec<u8>,
        catch_up: Option<CatchUp>,
    ) -> Result<Self> {
        if !address.ip().is_loopback() {
            return Err(Error::Invalid("webhook listen address must be loopback"));
        }
        if secret.is_empty() {
            return Err(Error::Invalid("webhook secret is empty"));
        }
        let catch_up_reader = catch_up.as_ref().map(|_| store.reader()).transpose()?;
        let server = http::Server::bind(address)?;
        let (sender, receiver) = mpsc::channel();
        Ok(Self {
            control: WebhookControl(Arc::new(Control {
                sender,
                server: server.control(),
            })),
            server,
            receiver,
            store,
            bootstrap,
            secret,
            catch_up: catch_up.map(|fetcher| Arc::new(fetcher) as Arc<dyn webhooks::Fetcher>),
            catch_up_reader,
        })
    }
    pub fn address(&self) -> SocketAddr {
        self.server.address()
    }
    pub fn control(&self) -> WebhookControl {
        self.control.clone()
    }
}

fn receive_delivery(store: &mut Store, secret: &[u8], request: &http::Request) -> u16 {
    let delivery = match webhook::verify_and_parse(
        secret,
        request.header("X-Hub-Signature-256"),
        request.header("X-GitHub-Delivery"),
        request.header("X-GitHub-Event"),
        &request.body,
    ) {
        Ok(delivery) => delivery,
        Err(_) => return 400,
    };
    let id = delivery.id.clone();
    let event_name = delivery.event_name;
    let payload = &request.body;
    match store.receive_ingress("github-webhook", &id, &event_name, payload) {
        Ok(_) => 202,
        Err(error) => {
            eprintln!("GitHub delivery {id} persistence failed: {error}");
            match store
                .reconnect()
                .and_then(|()| store.receive_ingress("github-webhook", &id, &event_name, payload))
            {
                Ok(_) => 202,
                Err(error) => {
                    eprintln!("GitHub delivery {id} persistence retry failed: {error}");
                    503
                }
            }
        }
    }
}

impl WebhookReceiver {
    fn run(self, notifications: Arc<webhooks::Notifications>) -> Result<()> {
        let Self {
            control,
            server,
            receiver,
            mut store,
            bootstrap,
            secret,
            catch_up,
            mut catch_up_reader,
        } = self;
        let requests = control.0.sender.clone();
        let exits = requests.clone();
        let server_thread = thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                server.serve(move |connection| {
                    let (status, head) = match connection.read(http::MAX_BODY) {
                        Err((status, head)) => (status, head),
                        Ok(request) if request.method != "POST" => (405, request.method == "HEAD"),
                        Ok(request) if request.path != "/github/events" => (404, false),
                        Ok(request) => {
                            let (reply, response) = mpsc::sync_channel(1);
                            if requests
                                .send(WebhookMessage::Request(request, reply))
                                .is_err()
                            {
                                return;
                            }
                            (response.recv().unwrap_or(503), false)
                        }
                    };
                    let headers = if status == 405 { "Allow: POST\r\n" } else { "" };
                    if connection.respond(status, "", head, headers).is_err() && status == 202 {
                        let _ = requests.send(WebhookMessage::Gap);
                    }
                })
            }))
            .unwrap_or(Err(Error::Invalid("webhook HTTP server panicked")));
            let _ = exits.send(WebhookMessage::ServerExit(result));
        });
        let mut state = webhooks::FetchState::Idle;
        let result = (|| {
            loop {
                match receiver
                    .recv()
                    .map_err(|_| Error::Invalid("webhook receiver stopped"))?
                {
                    WebhookMessage::Request(request, reply) => {
                        let status = receive_delivery(&mut store, &secret, &request);
                        if status == 202 {
                            notifications.wake();
                        }
                        let disconnected = reply.send(status).is_err();
                        if (status == 503 || disconnected || state.waiting())
                            && let Some(catch_up) = &catch_up
                        {
                            state.request(|| {
                                webhooks::fetch_catch_up(
                                    Arc::clone(catch_up),
                                    catch_up_reader.take().expect("idle catch-up reader"),
                                    bootstrap.clone(),
                                    control.0.sender.clone(),
                                )
                            });
                        }
                    }
                    WebhookMessage::Gap => {
                        if let Some(catch_up) = &catch_up {
                            state.request(|| {
                                webhooks::fetch_catch_up(
                                    Arc::clone(catch_up),
                                    catch_up_reader.take().expect("idle catch-up reader"),
                                    bootstrap.clone(),
                                    control.0.sender.clone(),
                                )
                            });
                        }
                    }
                    WebhookMessage::CaughtUp(reader, result) => {
                        catch_up_reader = Some(*reader);
                        let previous = std::mem::replace(&mut state, webhooks::FetchState::Idle);
                        let wanted = matches!(&previous, webhooks::FetchState::Wanted(Some(_)));
                        previous.join()?;
                        match result.and_then(|batch| batch.persist(&mut store)) {
                            Ok(_) => notifications.wake(),
                            Err(error) => {
                                eprintln!("GitHub catch-up failed: {error}");
                                state = webhooks::FetchState::Wanted(None);
                            }
                        }
                        if wanted && let Some(catch_up) = &catch_up {
                            state.request(|| {
                                webhooks::fetch_catch_up(
                                    Arc::clone(catch_up),
                                    catch_up_reader.take().expect("idle catch-up reader"),
                                    bootstrap.clone(),
                                    control.0.sender.clone(),
                                )
                            });
                        }
                    }
                    WebhookMessage::Stop => return Ok(()),
                    WebhookMessage::ServerExit(result) => {
                        result?;
                        return Err(Error::Invalid("webhook HTTP server stopped"));
                    }
                }
            }
        })();
        control.0.server.stop();
        drop(receiver);
        state.join()?;
        server_thread
            .join()
            .map_err(|_| Error::Invalid("webhook HTTP server panicked"))?;
        result
    }
}
