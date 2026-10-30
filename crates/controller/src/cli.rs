use crate::{
    Bootstrap, Controller, Error, LabelledIssue, Result, adapters, executor::Observation, status,
};
use serde::Deserialize;
use std::{
    fs,
    io::{self, BufRead},
    path::PathBuf,
};
use tl3_store::Store;

#[derive(Deserialize)]
pub enum Input {
    LabelledIssue(LabelledIssue),
    Observation {
        key: String,
        observation: Observation,
    },
}

fn arguments(daemon: bool) -> Result<(tl3_config::Config, Option<PathBuf>, bool)> {
    let mut args = std::env::args().skip(1);
    if !daemon && args.next().as_deref() != Some("status") {
        return Err(Error::Invalid("usage: tl3 status --config FILE"));
    }
    let mut path = None;
    let mut simulation = None;
    let mut drain = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" => path = args.next().map(PathBuf::from),
            "--simulate" if daemon => simulation = args.next().map(PathBuf::from),
            "--drain" if daemon => drain = true,
            _ => return Err(Error::Invalid("unknown or incomplete argument")),
        }
    }
    let path = path.ok_or(Error::Invalid("--config FILE is required"))?;
    let config = tl3_config::Config::parse(&fs::read_to_string(path)?)?;
    Ok((config, simulation, drain))
}

pub fn status() -> Result<()> {
    let (config, _, _) = arguments(false)?;
    print!("{}", status::render(&mut Store::open(&config.store)?)?);
    Ok(())
}

pub fn daemon() -> Result<()> {
    let (config, simulation, drain) = arguments(true)?;
    let simulation =
        simulation.ok_or(Error::Invalid("this release requires --simulate DIRECTORY"))?;
    let adapters = adapters::simulation(&simulation)?;
    let mut controller = Controller::open(
        Store::open(&config.store)?,
        adapters,
        Bootstrap::from_config(&config)?,
    )?;
    while controller.drive_once()? {}
    if drain {
        return Ok(());
    }
    for line in io::stdin().lock().lines() {
        let input: Input = serde_json::from_str(&line?)?;
        match input {
            Input::LabelledIssue(issue) => {
                controller.intake(issue)?;
            }
            Input::Observation { key, observation } => controller.observe(&key, observation)?,
        }
        while controller.drive_once()? {}
    }
    Ok(())
}
