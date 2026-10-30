fn main() {
    if let Err(error) = tl3_controller::cli::daemon() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
