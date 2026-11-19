//! Thin CLI entry point. All logic lives in the library (`notchd::...`).

fn main() {
    std::process::exit(notchd::cli::run(std::env::args().collect()));
}
