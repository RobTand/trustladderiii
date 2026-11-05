#[path = "tl3/redaction.rs"]
mod redaction_cli;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(command @ ("redact" | "redaction-policy")) = args.first().map(String::as_str) {
        if let Err(error) = redaction_cli::run(command, &args[1..]) {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    let result = if std::env::args().nth(1).as_deref() == Some("check-budget") {
        tl3_controller::budget::check()
    } else if std::env::args().nth(1).as_deref() == Some("check-harness") {
        tl3_controller::check_harness::command(std::env::args().nth(2).as_deref())
    } else if std::env::args().nth(1).as_deref() == Some("check-reply") {
        tl3_controller::classifier::check_reply(std::env::args().nth(2).as_deref())
    } else if std::env::args().nth(1).as_deref() == Some("replay-check") {
        tl3_controller::replay::command()
    } else if std::env::args().nth(1).as_deref() == Some("store-stats") {
        tl3_controller::store_stats::command()
    } else if std::env::args().nth(1).as_deref() == Some("memory") {
        tl3_controller::cli::memory()
    } else {
        tl3_controller::cli::status()
    };
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
