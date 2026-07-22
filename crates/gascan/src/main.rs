#![forbid(unsafe_code)]
#![deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

mod cli;
mod client;
mod presentation;
mod terminal;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let code = cli::execute().await.unwrap_or_else(|error| {
        eprintln!("{error}");
        error.exit_code()
    });
    std::process::exit(code);
}
