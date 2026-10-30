use crate::{
    Bootstrap, Controller, Error, LabelledIssue, Repository, Result, cli::Input,
    executor::Observation,
};
use chrono::{DateTime, Utc};
use serde_json::json;
use std::{
    collections::BTreeMap,
    io::{self, BufRead, Read},
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Sender},
    },
    thread,
};
use tiny_http::{Method, Response, Server};
use tl3_github::{
    GitHub, ServedRepo,
    webhook::{self, Event},
};
use tl3_store::Store;

pub trait Source: Send + 'static {
    fn run(self, sender: Sender<Input>) -> Result<()>;
}

pub struct Sources {
    sender: Sender<Input>,
    receiver: mpsc::Receiver<Input>,
    threads: Vec<thread::JoinHandle<Result<()>>>,
}
impl Default for Sources {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            sender,
            receiver,
            threads: Vec::new(),
        }
    }
}
impl Sources {
    pub fn register(&mut self, source: impl Source) {
        let sender = self.sender.clone();
        self.threads.push(thread::spawn(move || source.run(sender)));
    }
    pub fn run(self, controller: &mut Controller) -> Result<()> {
        drop(self.sender);
        run(controller, self.receiver)?;
        for handle in self.threads {
            handle
                .join()
                .map_err(|_| Error::Invalid("ingress source panicked"))??;
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
        Input::LabelledIssue(issue) => {
            controller.intake(issue)?;
        }
        Input::Observation { key, observation } => controller.observe(&key, observation)?,
    }
    Ok(())
}

/// Replays use the same intake and observation keys after a crash before consumption.
pub fn drain(controller: &mut Controller) -> Result<()> {
    loop {
        let deliveries = controller.store.pending_ingress()?;
        if deliveries.is_empty() {
            return Ok(());
        }
        for (sequence, payload) in deliveries {
            for input in serde_json::from_value::<Vec<Input>>(payload)? {
                apply(controller, input)?;
            }
            controller.store.consume_ingress(sequence)?;
        }
    }
}

pub fn run(controller: &mut Controller, receiver: mpsc::Receiver<Input>) -> Result<()> {
    drain(controller)?;
    while controller.drive_once()? {}
    for input in receiver {
        drain(controller)?;
        apply(controller, input)?;
        while controller.drive_once()? {}
    }
    Ok(())
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
                actor: actor.id,
                label: label.name,
            })]
        }
        Some(Event::CommentCreated {
            context,
            issue,
            comment,
            ..
        }) if issue.pull_request.is_none() => {
            let Some(repo) = bootstrap.repos.iter().find(|repo| {
                repo.owner
                    .eq_ignore_ascii_case(&context.repository.owner.login)
                    && repo.name.eq_ignore_ascii_case(&context.repository.name)
                    && repo.installation == context.installation_id
            }) else {
                return vec![];
            };
            comment_input(repo, issue.number, comment, bootstrap.rob)
                .into_iter()
                .collect()
        }
        _ => vec![],
    }
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

struct Inbox<'a> {
    store: &'a mut Store,
    bootstrap: &'a Bootstrap,
    inputs: Vec<Input>,
}
impl webhook::Inbox for Inbox<'_> {
    type Error = tl3_store::Error;
    fn insert(&mut self, delivery: webhook::Delivery) -> std::result::Result<bool, Self::Error> {
        self.inputs = inputs(&delivery.id, delivery.event, self.bootstrap);
        self.store
            .receive_ingress("github-webhook", &delivery.id, &json!(&self.inputs))
    }
}

pub struct CatchUp {
    clients: BTreeMap<u64, GitHub>,
    runtime: tokio::runtime::Runtime,
}
impl CatchUp {
    pub fn new(clients: BTreeMap<u64, GitHub>) -> Result<Self> {
        Ok(Self {
            clients,
            runtime: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?,
        })
    }
    pub fn pass(&self, store: &mut Store, bootstrap: &Bootstrap) -> Result<Vec<Input>> {
        let runtime = &self.runtime;
        let since: DateTime<Utc> = store
            .ingress_cursor("github-catch-up")?
            .map(serde_json::from_value)
            .transpose()?
            .unwrap_or(DateTime::UNIX_EPOCH);
        let snapshot = store.load()?;
        let mut deliveries = Vec::new();
        let mut notifications = Vec::new();
        let mut cursor = None;
        for repo in &bootstrap.repos {
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
                .filter(|(_, request)| request.repo == *repo)
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
            for changes in result.repositories {
                for (number, change) in changes.labels {
                    if change.event != "labeled" {
                        continue;
                    }
                    let input = Input::LabelledIssue(LabelledIssue {
                        delivery: format!("catch-up-label:{name}:{}", change.id),
                        owner: repo.owner.clone(),
                        repo: repo.name.clone(),
                        installation: repo.installation,
                        number,
                        actor: change.actor.id,
                        label: change.label.name,
                    });
                    // Repository event feeds also include PR labels. Never authorize those.
                    let issue = match changes.issues.iter().find(|issue| issue.number == number) {
                        Some(issue) => issue.clone(),
                        None => runtime.block_on(client.issue(&name, number))?,
                    };
                    if issue.pull_request.is_none() {
                        deliveries.push((
                            format!("label:{name}:{}", change.id),
                            json!([input.clone()]),
                        ));
                        notifications.push(input);
                    }
                }
                for comment in changes.comments {
                    let prefix = format!("/repos/{name}/issues/");
                    let Some(number) = comment
                        .issue_url
                        .split_once(&prefix)
                        .and_then(|(_, number)| number.parse::<u64>().ok())
                    else {
                        return Err(Error::Invalid("catch-up comment has an invalid issue URL"));
                    };
                    if let Some(input) = comment_input(repo, number, comment, bootstrap.rob) {
                        let Input::Observation { ref key, .. } = input else {
                            unreachable!()
                        };
                        deliveries.push((key.clone(), json!([input.clone()])));
                        notifications.push(input);
                    }
                }
            }
        }
        if let Some(cursor) = cursor {
            store.receive_catch_up("github-catch-up", &deliveries, &json!(cursor))?;
        }
        Ok(notifications)
    }
}

struct Control {
    gap: AtomicBool,
    stop: AtomicBool,
    server: Arc<Server>,
}
#[derive(Clone)]
pub struct WebhookControl(Arc<Control>);
impl WebhookControl {
    /// Call when the tunnel or upstream connection reports a gap, never on a timer.
    pub fn gap(&self) {
        self.0.gap.store(true, Ordering::Release);
        self.0.server.unblock();
    }
    pub fn stop(&self) {
        self.0.stop.store(true, Ordering::Release);
        self.0.server.unblock();
    }
}

pub struct Webhooks {
    control: WebhookControl,
    store: Store,
    bootstrap: Bootstrap,
    secret: Vec<u8>,
    catch_up: Option<CatchUp>,
}
impl Webhooks {
    pub fn bind(
        address: SocketAddr,
        store: Store,
        bootstrap: Bootstrap,
        secret: Vec<u8>,
        catch_up: Option<CatchUp>,
    ) -> Result<Self> {
        if secret.is_empty() {
            return Err(Error::Invalid("webhook secret is empty"));
        }
        let server = Arc::new(
            Server::http(address).map_err(|_| Error::Invalid("cannot bind webhook receiver"))?,
        );
        Ok(Self {
            control: WebhookControl(Arc::new(Control {
                server,
                gap: AtomicBool::new(true),
                stop: AtomicBool::new(false),
            })),
            store,
            bootstrap,
            secret,
            catch_up,
        })
    }
    pub fn address(&self) -> SocketAddr {
        self.control
            .0
            .server
            .server_addr()
            .to_ip()
            .expect("TCP receiver")
    }
    pub fn control(&self) -> WebhookControl {
        self.control.clone()
    }
    fn catch_up(&mut self, sender: &Sender<Input>) {
        if let Some(catch_up) = &self.catch_up {
            match catch_up.pass(&mut self.store, &self.bootstrap) {
                Ok(inputs) => {
                    for input in inputs {
                        let _ = sender.send(input);
                    }
                }
                Err(error) => {
                    eprintln!("GitHub catch-up failed: {error}");
                    self.control.0.gap.store(true, Ordering::Release);
                }
            }
        }
    }
}
impl Source for Webhooks {
    fn run(mut self, sender: Sender<Input>) -> Result<()> {
        self.control.0.gap.store(false, Ordering::Release);
        self.catch_up(&sender);
        while !self.control.0.stop.load(Ordering::Acquire) {
            let request = self.control.0.server.recv();
            let mut request = match request {
                Ok(request) => request,
                Err(_)
                    if self.control.0.stop.load(Ordering::Acquire)
                        || self.control.0.gap.load(Ordering::Acquire) =>
                {
                    if self.control.0.stop.load(Ordering::Acquire) {
                        break;
                    }
                    if self.control.0.gap.swap(false, Ordering::AcqRel) {
                        self.catch_up(&sender);
                    }
                    continue;
                }
                Err(error) => {
                    self.control.0.gap.store(true, Ordering::Release);
                    return Err(error.into());
                }
            };
            if request.method() != &Method::Post || request.url() != "/webhooks/github" {
                request.respond(Response::empty(404))?;
                continue;
            }
            let header = |name: &'static str| {
                request
                    .headers()
                    .iter()
                    .find(|h| h.field.equiv(name))
                    .map(|h| h.value.as_str().to_owned())
                    .unwrap_or_default()
            };
            let signature = header("X-Hub-Signature-256");
            let delivery = header("X-GitHub-Delivery");
            let event = header("X-GitHub-Event");
            let mut body = Vec::new();
            // Bound untrusted request bodies before signature verification.
            if request
                .as_reader()
                .take(25 * 1024 * 1024 + 1)
                .read_to_end(&mut body)
                .is_err()
            {
                self.control.0.gap.store(true, Ordering::Release);
                continue;
            }
            if body.len() > 25 * 1024 * 1024 {
                request.respond(Response::empty(413))?;
                continue;
            }
            let mut inbox = Inbox {
                store: &mut self.store,
                bootstrap: &self.bootstrap,
                inputs: vec![],
            };
            let result = webhook::receive(
                &self.secret,
                &signature,
                &delivery,
                &event,
                &body,
                &mut inbox,
            );
            let status = match result {
                Ok(inserted) => {
                    if inserted {
                        for input in inbox.inputs {
                            let _ = sender.send(input);
                        }
                    }
                    202
                }
                Err(webhook::ReceiveError::Webhook(_)) => 400,
                Err(webhook::ReceiveError::Inbox(_)) => {
                    self.control.0.gap.store(true, Ordering::Release);
                    503
                }
            };
            if request.respond(Response::empty(status)).is_err() {
                self.control.0.gap.store(true, Ordering::Release);
            }
            if self.control.0.gap.swap(false, Ordering::AcqRel) {
                self.catch_up(&sender);
            }
        }
        Ok(())
    }
}
