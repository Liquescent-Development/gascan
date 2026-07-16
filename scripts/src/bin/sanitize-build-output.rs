use std::{
    env,
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    os::unix::{ffi::OsStrExt, fs::OpenOptionsExt},
    path::Path,
    process::ExitCode,
};

const SENSITIVE: u8 = 42;

fn credential_name(name: &str) -> bool {
    matches!(
        name,
        "GITHUB_TOKEN"
            | "GH_TOKEN"
            | "GITLAB_TOKEN"
            | "DOCKER_AUTH_CONFIG"
            | "HTTP_AUTHORIZATION"
            | "AUTHORIZATION"
            | "AWS_ACCESS_KEY_ID"
            | "AWS_SECRET_ACCESS_KEY"
            | "AWS_SESSION_TOKEN"
    ) || (name.starts_with("GASCAMP_") && name.contains("TOKEN"))
        || ["TOKEN", "CREDENTIAL", "PASSWORD", "SECRET"]
            .iter()
            .any(|kind| {
                name == format!("BUILD_{kind}")
                    || (name.contains("BUILD_") && name.ends_with(&format!("_{kind}")))
            })
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

fn run(output: &Path, limit: usize) -> io::Result<bool> {
    let mut needles: Vec<Vec<u8>> = [
        "authorization",
        "bearer",
        "token",
        "secret",
        "password",
        "credential",
    ]
    .into_iter()
    .map(|value| value.as_bytes().to_vec())
    .collect();
    needles.extend(env::vars_os().filter_map(|(name, value)| {
        let name = name.to_str()?;
        let value = value.as_bytes();
        (credential_name(name) && !value.is_empty()).then(|| value.to_vec())
    }));
    let overlap = needles
        .iter()
        .map(Vec::len)
        .max()
        .unwrap_or(1)
        .saturating_sub(1);
    let mut captured = Vec::with_capacity(limit.min(131_073));
    let mut tail = Vec::new();
    let mut sensitive = false;
    let mut chunk = [0_u8; 8192];
    loop {
        let count = io::stdin().read(&mut chunk)?;
        if count == 0 {
            break;
        }
        if captured.len() < limit {
            captured.extend_from_slice(&chunk[..count.min(limit - captured.len())]);
        }
        let mut scan = tail;
        scan.extend_from_slice(&chunk[..count]);
        let lower = scan.iter().map(u8::to_ascii_lowercase).collect::<Vec<_>>();
        sensitive |= needles.iter().any(|needle| {
            contains(&scan, needle)
                || contains(
                    &lower,
                    &needle
                        .iter()
                        .map(u8::to_ascii_lowercase)
                        .collect::<Vec<_>>(),
                )
        });
        tail = scan[scan.len().saturating_sub(overlap)..].to_vec();
    }

    if sensitive {
        return Ok(false);
    }

    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(output)?;
    if let Err(error) = file.write_all(&captured).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = fs::remove_file(output);
        return Err(error);
    }
    Ok(true)
}

fn main() -> ExitCode {
    let args = env::args_os().collect::<Vec<_>>();
    if args.len() != 3 {
        return ExitCode::FAILURE;
    }
    let Ok(limit) = args[2].to_string_lossy().parse::<usize>() else {
        return ExitCode::FAILURE;
    };
    match run(Path::new(&args[1]), limit) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(SENSITIVE),
        Err(_) => ExitCode::FAILURE,
    }
}
