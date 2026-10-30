use crate::{
    Bootstrap, Controller, Error, LabelledIssue, Result, adapters, executor::Observation, status,
    status_web,
};
use serde::Deserialize;
use std::{
    io::{self, BufRead},
    path::PathBuf,
};
use tl3_store::{Reader, Store};

#[derive(Deserialize)]
pub enum Input {
    LabelledIssue(LabelledIssue),
    Observation {
        key: String,
        observation: Observation,
    },
}

enum Command {
    Status,
    ServeStatus,
    Daemon { simulation: PathBuf, drain: bool },
}

fn arguments(daemon: bool) -> Result<(tl3_config::Config, Command)> {
    let mut args = std::env::args().skip(1);
    let serve = if daemon {
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
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" => path = args.next().map(PathBuf::from),
            "--simulate" if daemon => simulation = args.next().map(PathBuf::from),
            "--drain" if daemon => drain = true,
            _ => return Err(Error::Invalid("unknown or incomplete argument")),
        }
    }
    let path = path.ok_or(Error::Invalid("--config FILE is required"))?;
    let config = tl3_config::Config::load(path)?;
    let command = if daemon {
        Command::Daemon {
            simulation: simulation
                .ok_or(Error::Invalid("this release requires --simulate DIRECTORY"))?,
            drain,
        }
    } else if serve {
        Command::ServeStatus
    } else {
        Command::Status
    };
    Ok((config, command))
}

pub fn status() -> Result<()> {
    let (config, command) = arguments(false)?;
    match command {
        Command::Status => {
            let model = status::load(&mut Reader::open(&config.store)?)?;
            print!("{}", status::render_text(&model));
            Ok(())
        }
        Command::ServeStatus => status_web::serve(&config),
        Command::Daemon { .. } => unreachable!(),
    }
}

pub fn daemon() -> Result<()> {
    let (config, command) = arguments(true)?;
    let Command::Daemon { simulation, drain } = command else {
        unreachable!()
    };
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
