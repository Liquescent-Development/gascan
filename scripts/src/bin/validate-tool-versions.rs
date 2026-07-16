use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fs,
    io::Write,
    path::Path,
};

use serde::Deserialize;

type DynError = Box<dyn Error + Send + Sync>;

// Erlang is Elixir's audited implementation dependency, not an eighth
// user-facing runtime.
const TOOLS: [&str; 8] = [
    "elixir", "erlang", "go", "java", "node", "python", "ruby", "rust",
];

#[derive(Deserialize)]
struct ToolFile {
    tools: BTreeMap<String, String>,
}

fn main() -> Result<(), DynError> {
    let mut args = std::env::args_os().skip(1);
    let lock = args.next().ok_or("missing versions lock path")?;
    let config = args.next().ok_or("missing mise config path")?;
    let resolved = args.next();
    if args.next().is_some() {
        return Err("unexpected tool-version validator argument".into());
    }

    let locked = read_toml_tools(Path::new(&lock))?;
    let configured = read_toml_tools(Path::new(&config))?;
    require_exact_keys(&locked)?;
    require_exact_keys(&configured)?;
    if locked != configured {
        return Err("mise config does not exactly match versions lock".into());
    }

    if let Some(path) = resolved {
        let contents = fs::read(path)?;
        let actual: BTreeMap<String, String> = serde_json::from_slice(&contents)?;
        require_exact_keys(&actual)?;
        if actual != locked {
            return Err("resolved tool versions do not exactly match locked defaults".into());
        }
    }

    serde_json::to_writer(std::io::stdout().lock(), &locked)?;
    std::io::stdout().lock().write_all(b"\n")?;
    Ok(())
}

fn read_toml_tools(path: &Path) -> Result<BTreeMap<String, String>, DynError> {
    let contents = fs::read_to_string(path)?;
    Ok(toml::from_str::<ToolFile>(&contents)?.tools)
}

fn require_exact_keys(tools: &BTreeMap<String, String>) -> Result<(), DynError> {
    let expected: BTreeSet<&str> = TOOLS.into_iter().collect();
    let actual: BTreeSet<&str> = tools.keys().map(String::as_str).collect();
    if actual != expected || tools.values().any(String::is_empty) {
        return Err(
            "tool map must contain exactly seven supported runtimes and the Erlang dependency"
                .into(),
        );
    }
    Ok(())
}
