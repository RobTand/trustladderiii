use crate::{
    Bootstrap, Controller, Error, LabelledIssue, Result, adapters, executor::Observation, status,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, net::SocketAddr, path::PathBuf};
use tl3_store::Store;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Input {
    LabelledIssue(LabelledIssue),
    Observation {
        key: String,
        observation: Observation,
    },
}

struct Arguments {
    config: tl3_config::Config,
    simulation: Option<PathBuf>,
    drain: bool,
    github_listen: Option<SocketAddr>,
    github_api: Option<String>,
}

fn arguments(daemon: bool) -> Result<Arguments> {
    let mut args = std::env::args().skip(1);
    if !daemon && args.next().as_deref() != Some("status") {
        return Err(Error::Invalid("usage: tl3 status --config FILE"));
    }
    let mut path = None;
    let mut simulation = None;
    let mut drain = false;
    let mut github_listen = None;
    let mut github_api = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" => path = args.next().map(PathBuf::from),
            "--simulate" if daemon => simulation = args.next().map(PathBuf::from),
            "--drain" if daemon => drain = true,
            "--github-listen" if daemon => {
                github_listen = Some(
                    args.next()
                        .ok_or(Error::Invalid("--github-listen needs an address"))?
                        .parse()
                        .map_err(|_| Error::Invalid("invalid webhook listen address"))?,
                )
            }
            "--github-api" if daemon => github_api = args.next(),
            _ => return Err(Error::Invalid("unknown or incomplete argument")),
        }
    }
    let path = path.ok_or(Error::Invalid("--config FILE is required"))?;
    let config = tl3_config::Config::parse(&fs::read_to_string(path)?)?;
    Ok(Arguments {
        config,
        simulation,
        drain,
        github_listen,
        github_api,
    })
}

pub fn status() -> Result<()> {
    let config = arguments(false)?.config;
    print!("{}", status::render(&mut Store::open(&config.store)?)?);
    Ok(())
}

pub fn daemon() -> Result<()> {
    let Arguments {
        config,
        simulation,
        drain,
        github_listen,
        github_api,
    } = arguments(true)?;
    let simulation =
        simulation.ok_or(Error::Invalid("this release requires --simulate DIRECTORY"))?;
    let bootstrap = Bootstrap::from_config(&config)?;
    let mut adapters = adapters::simulation(&simulation)?;
    let mut sources = crate::ingress::Sources::default();
    let listen = github_listen.or(config.github.webhook_listen);
    if let Some(listen) = listen {
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
        let mut secret = Vec::new();
        for repo in &bootstrap.repos {
            if clients.contains_key(&repo.installation) {
                continue;
            }
            let app: tl3_github::AppConfig =
                serde_json::from_slice(&fs::read(&config.github.app_credentials_file)?)?;
            secret = app.webhook_secret.as_bytes().to_vec();
            clients.insert(
                repo.installation,
                tl3_github::GitHub::new(app, repo.installation, options.clone())?,
            );
        }
        adapters.register(
            crate::executor::Upstream::GitHub,
            adapters::github::Adapter::new(
                clients.clone(),
                adapters::workers::FakeWriter,
                epoch,
                tl3_github::message::Policy::default(),
                &simulation,
            )?,
        );
        sources.register(crate::ingress::Webhooks::bind(
            listen,
            Store::open(&config.store)?,
            bootstrap.clone(),
            secret,
            Some(crate::ingress::CatchUp { clients }),
        )?);
    }
    let mut controller = Controller::open(Store::open(&config.store)?, adapters, bootstrap)?;
    crate::ingress::drain(&mut controller)?;
    while controller.drive_once()? {}
    if drain {
        return Ok(());
    }
    sources.register(crate::ingress::Stdin);
    sources.run(&mut controller)
}
