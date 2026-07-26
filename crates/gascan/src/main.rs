#[tokio::main(flavor = "current_thread")]
async fn main() {
    #[cfg(debug_assertions)]
    if option_env!("CARGO_BIN_NAME") == Some("gascan-e2e-cli") {
        let configured = std::env::var_os("GASCAN_E2E_ACCOUNT_HOME")
            .ok_or("GASCAN_E2E_ACCOUNT_HOME is required")
            .and_then(|home| {
                gascan::ssh_config::configure_e2e_account_home(std::path::Path::new(&home))
                    .map_err(|_| "GASCAN_E2E_ACCOUNT_HOME is unsafe")
            });
        if let Err(error) = configured {
            eprintln!("Error: {error}");
            std::process::exit(70);
        }
    }
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
