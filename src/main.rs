fn main() {
    env_logger::init();
    eprintln!(
        "kabi-lock {}: locking is not implemented yet",
        env!("CARGO_PKG_VERSION")
    );
    std::process::exit(1);
}
