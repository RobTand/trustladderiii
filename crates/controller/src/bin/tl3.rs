fn main() {
    let result = if std::env::args().nth(1).as_deref() == Some("check-reply") {
        tl3_controller::classifier::check_reply(std::env::args().nth(2).as_deref())
    } else if std::env::args().nth(1).as_deref() == Some("replay-check") {
        tl3_controller::replay::command()
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
