fn main() {
    if std::env::args().nth(1).as_deref() == Some("check-reply") {
        println!("{{\"violations\":[\"classifier validator not implemented\"]}}");
        return;
    }
    let result = if std::env::args().nth(1).as_deref() == Some("memory") {
        tl3_controller::cli::memory()
    } else {
        tl3_controller::cli::status()
    };
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
