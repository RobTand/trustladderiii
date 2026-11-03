use crate::{
    Bootstrap, Controller, Error, LabelledIssue, Result, adapters, executor::Observation, status,
    status_web,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, net::SocketAddr, path::PathBuf};
use tl3_store::{Reader, Store};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Input {
    Wake,
    GitHubWake,
    Look {
        key: String,
    },
    LabelledIssue(LabelledIssue),
    IssueChanged(crate::IssueChange),
    ProposedIssue {
        issue: LabelledIssue,
        proposed: crate::adoption::ProposedPullRequest,
    },
    AdoptionClarification(crate::adoption::Clarification),
    Observation {
        key: String,
        observation: Observation,
    },
}

struct Arguments {
    config: tl3_config::Config,
    simulation: Option<PathBuf>,
    drain: bool,
    workers: bool,
    github_listen: Option<SocketAddr>,
    github_api: Option<String>,
    serve_status: bool,
    /// Simulation only: wake on console commands, as production always does.
    console: bool,
}

fn arguments(daemon: bool) -> Result<Arguments> {
    let mut args = std::env::args().skip(1);
    let serve_status = if daemon {
        false
    } else {
        match args.next().as_deref() {
            Some("status") => false,
            Some("status-serve") => true,
            _ => {
                return Err(Error::Invalid(
                    "usage: tl3 {status|status-serve} --config FILE",
                ));
            }
        }
    };
    let mut path = None;
    let mut simulation = None;
    let mut drain = false;
    let mut workers = false;
    let mut github_listen = None;
    let mut github_api = None;
    let mut console = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" => path = args.next().map(PathBuf::from),
            "--simulate" if daemon => simulation = args.next().map(PathBuf::from),
            "--drain" if daemon => drain = true,
            "--workers" if daemon => workers = true,
            "--github-listen" if daemon => {
                github_listen = Some(
                    args.next()
                        .ok_or(Error::Invalid("--github-listen needs an address"))?
                        .parse()
                        .map_err(|_| Error::Invalid("invalid webhook listen address"))?,
                )
            }
            "--github-api" if daemon => github_api = args.next(),
            "--console" if daemon => console = true,
            _ => return Err(Error::Invalid("unknown or incomplete argument")),
        }
    }
    let path = path.ok_or(Error::Invalid("--config FILE is required"))?;
    let config = if daemon {
        tl3_config::Config::load(path)?
    } else {
        tl3_config::Config::load_status(path)?
    };
    Ok(Arguments {
        config,
        simulation,
        drain,
        workers,
        github_listen,
        github_api,
        serve_status,
        console,
    })
}

pub fn status() -> Result<()> {
    let Arguments {
        config,
        serve_status,
        ..
    } = arguments(false)?;
    if serve_status {
        status_web::serve(&config)
    } else {
        let model = status::load(&mut Reader::open(&config.store)?)?;
        print!("{}", status::render_text(&model));
        print!("{}", status::render_protected_paths(&config));
        Ok(())
    }
}

/// `tl3 memory {list | confirm ID | invalidate ID | revert ID} [--reason TEXT] --config FILE`:
/// the audit trail of TL3's memory curations, and Rob's own decisions (#422).
pub fn memory() -> Result<()> {
    const USAGE: &str = "usage: tl3 memory {list | confirm ID | invalidate ID | revert ID} [--reason TEXT] --config FILE";
    let mut args = std::env::args().skip(2);
    let action = args.next().ok_or(Error::Invalid(USAGE))?;
    let state = match action.as_str() {
        "list" => None,
        "confirm" => Some(crate::memory::RobCuration::Confirm),
        "invalidate" => Some(crate::memory::RobCuration::Invalidate),
        "revert" => Some(crate::memory::RobCuration::Revert),
        _ => return Err(Error::Invalid(USAGE)),
    };
    let id = match state {
        Some(_) => Some(args.next().ok_or(Error::Invalid(USAGE))?),
        None => None,
    };
    let (mut path, mut reason) = (None, None);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" => path = args.next().map(PathBuf::from),
            "--reason" if state.is_some() => reason = args.next(),
            _ => return Err(Error::Invalid(USAGE)),
        }
    }
    let config = tl3_config::Config::load(path.ok_or(Error::Invalid(USAGE))?)?;
    let mut store = Store::open(&config.store)?;
    let (Some(state), Some(id)) = (state, id) else {
        for curation in store.memory_curations()? {
            println!("{curation}");
        }
        return Ok(());
    };
    let reason = reason.ok_or(Error::Invalid("--reason TEXT is required"))?;
    crate::memory::rob_curation(&mut store, &id, state, &reason)?;
    match crate::memory::flush(&mut store, &config.hindsight) {
        Ok(_) => println!("Recorded and sent to Hindsight."),
        Err(error) => println!("Recorded. tl3d sends it when Hindsight answers ({error})."),
    }
    Ok(())
}

pub fn daemon() -> Result<()> {
    let Arguments {
        config,
        simulation,
        drain,
        workers,
        github_listen,
        github_api,
        console,
        ..
    } = arguments(true)?;
    let unlisted = config.planning.defaults_not_listed();
    if !unlisted.is_empty() {
        eprintln!(
            "note: planning.protected_paths omits the default protected paths {}; they stay protected",
            unlisted.join(", ")
        );
    }
    let Some(simulation) = simulation else {
        if workers || github_listen.is_some() || github_api.is_some() || drain || console {
            return Err(Error::Invalid(
                "simulation options require --simulate DIRECTORY",
            ));
        }
        return production(config);
    };
    let listen = github_listen.or(config.github.webhook_listen).or_else(|| {
        github_api
            .as_ref()
            .map(|_| SocketAddr::from(([127, 0, 0, 1], 8081)))
    });
    if listen.is_some() && config.github.operations.is_none() {
        return Err(Error::Invalid(
            "real GitHub adapter requires [github.operations]",
        ));
    }
    let bootstrap = Bootstrap::from_config(&config)?;
    let mut sources = crate::ingress::Sources::default();
    let mut adapters = if workers {
        let (adapters, source) = adapters::simulation_with_workers(&simulation, &config)?;
        sources.register(source);
        adapters
    } else {
        adapters::simulation(&simulation)?
    };
    let mut adoption = None;
    if let Some(listen) = listen {
        if !listen.ip().is_loopback() {
            return Err(Error::Invalid("webhook listen address must be loopback"));
        }
        let api = github_api.ok_or(Error::Invalid(
            "simulation webhooks require --github-api with a loopback mock server",
        ))?;
        let options = tl3_github::ClientOptions {
            api_url: api
                .parse()
                .map_err(|_| Error::Invalid("invalid GitHub API URL"))?,
            ..tl3_github::ClientOptions::default()
        };
        if options.api_url.scheme() != "http"
            || !matches!(
                options.api_url.host_str(),
                Some("127.0.0.1" | "[::1]" | "localhost")
            )
        {
            return Err(Error::Invalid(
                "simulation GitHub API must be loopback HTTP",
            ));
        }
        let epoch = Store::open(&config.store)?.load()?.epoch.get();
        let mut clients = BTreeMap::new();
        let mut catch_up_clients = BTreeMap::new();
        let mut secret = Vec::new();
        for repo in &bootstrap.github_repositories() {
            if clients.contains_key(&repo.installation) {
                continue;
            }
            let credentials = fs::read(&config.github.app_credentials_file)?;
            let app: tl3_github::AppConfig = serde_json::from_slice(&credentials)?;
            catch_up_clients.insert(
                repo.installation,
                tl3_github::GitHub::new(
                    serde_json::from_slice(&credentials)?,
                    repo.installation,
                    options.clone(),
                )?,
            );
            secret = app.webhook_secret.as_bytes().to_vec();
            clients.insert(
                repo.installation,
                tl3_github::GitHub::new(app, repo.installation, options.clone())?,
            );
        }
        adoption = Some(crate::adoption::Translator::new(
            clients.clone(),
            bootstrap.clone(),
        )?);
        adapters.register(
            crate::executor::Upstream::GitHub,
            adapters::github::Adapter::new(
                clients,
                adapters::workers::FakeWriter,
                epoch,
                tl3_github::message::Policy::default(),
                &simulation,
                config.github.operations.clone(),
            )?
            .with_repositories(&config.github.repos)?,
        );
        let webhooks = crate::ingress::Webhooks::bind(
            listen,
            Store::open(&config.store)?,
            bootstrap.clone(),
            secret,
            Some(crate::ingress::CatchUp::new(catch_up_clients)?),
        )?;
        eprintln!("GitHub webhooks listening on {}", webhooks.address());
        sources.register(webhooks);
    }
    let mut controller = Controller::open(Store::open(&config.store)?, adapters, bootstrap)?;
    if let Some(adoption) = adoption {
        controller.register_github_translator("adoption", adoption);
    }
    controller.set_allowances(config.allowances.clone());
    controller.set_priority_credit_reserve(&config.limits);
    controller.set_request_protection(&config);
    controller.set_delegated_attributions(
        config
            .operations
            .as_ref()
            .map_or(&[], |o| o.delegated_attributions.as_slice()),
    );
    controller.start_release(&crate::release::id())?;
    controller.run_postmortems(crate::PostmortemPace::from_config(&config));
    crate::ingress::drain(&mut controller)?;
    while controller.drive_once()? {}
    if drain {
        return Ok(());
    }
    if console {
        sources.register(crate::console::Wakes::new(config.store.clone()));
    }
    sources.register(crate::ingress::Stdin);
    sources.run(&mut controller)
}

fn production(config: tl3_config::Config) -> Result<()> {
    let registration = crate::production::register(&config)?;
    let mut controller = Controller::open(
        Store::open(&config.store)?,
        registration.adapters,
        Bootstrap::from_config(&config)?,
    )?;
    controller.set_allowances(config.allowances.clone());
    controller.set_priority_credit_reserve(&config.limits);
    controller.set_request_protection(&config);
    controller.set_delegated_attributions(
        config
            .operations
            .as_ref()
            .map_or(&[], |o| o.delegated_attributions.as_slice()),
    );
    controller.start_release(&crate::release::id())?;
    controller.run_postmortems(crate::PostmortemPace::from_config(&config));
    controller.register_github_translator("mergify", registration.mergify);
    controller.register_github_translator("adoption", registration.adoption);
    eprintln!(
        "GitHub webhooks listening on {}",
        registration.webhooks.address()
    );
    eprintln!(
        "production adapters registered: GitHub, worktrees, PrismaBuild, Mergify, workers; looks and incidents enabled"
    );
    let mut sources = crate::ingress::Sources::default();
    sources.register(registration.webhooks);
    sources.register(registration.workers);
    sources.register(registration.bell);
    sources.register(crate::console::Wakes::new(config.store.clone()));
    sources.run(&mut controller)
}
