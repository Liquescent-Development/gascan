#![forbid(unsafe_code)]
#![deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

mod cli;
mod client;
mod presentation;
mod terminal;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let code = match cli::execute().await {
        Ok(code) => code,
        Err(error) => {
            let code = error.exit_code();
            eprint!("{}", cli::render_error(&error));
            code
        }
    };
    std::process::exit(code);
}
