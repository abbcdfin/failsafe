fn main() {
    eprintln!(
        "failsafe {} ({}): monitoring is not implemented yet",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::ARCH
    );
    std::process::exit(1);
}
