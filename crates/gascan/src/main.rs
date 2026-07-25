#[tokio::main(flavor = "current_thread")]
async fn main() {
    let code = match gascan::cli::execute().await {
        Ok(code) => code,
        Err(error) => {
            let code = error.exit_code();
            eprint!("{}", gascan::cli::render_error(&error));
            code
        }
    };
    std::process::exit(code);
}
