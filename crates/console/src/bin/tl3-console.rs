use std::{io::Write, path::PathBuf, process::ExitCode};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("tl3-console: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> tl3_console::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = match (args.next().as_deref(), args.next(), args.next()) {
        (Some("--config"), Some(path), None) => PathBuf::from(path),
        _ => {
            return Err(tl3_console::Error::Invalid(
                "usage: tl3-console --config FILE",
            ));
        }
    };
    // The console needs only its store credential, like the status page.
    let config = tl3_config::Config::load_status(path)?;
    config.validate()?;
    let console = config.console.ok_or(tl3_console::Error::Invalid(
        "the config has no [console] section",
    ))?;
    let listener = std::net::TcpListener::bind(console.listen)?;
    let running = tl3_console::start(
        listener,
        tl3_console::Options {
            store: config.store,
            assets: console.assets,
            models: Some(config.models),
        },
    )?;
    println!("Console listening on http://{}", running.address());
    std::io::stdout().flush()?;
    running.wait()
}
