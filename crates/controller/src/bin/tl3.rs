fn main() {
    if let Err(error) = tl3_controller::cli::status() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
