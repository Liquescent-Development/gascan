use std::{
    fs,
    io::Write,
    os::unix::fs::{symlink, PermissionsExt},
    process::{Command, Stdio},
};

const OMITTED: &[u8] = b"\n[... middle diagnostic output omitted ...]\n";

fn run_with_limit(
    input: &[u8],
    output: &std::path::Path,
    env: Option<(&str, &str)>,
    limit: usize,
) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sanitize-build-output"));
    command.arg(output).arg(limit.to_string()).stdin(Stdio::piped());
    if let Some((name, value)) = env {
        command.env(name, value);
    }
    let mut child = command.spawn().unwrap();
    let _ = child.stdin.take().unwrap().write_all(input);
    child.wait_with_output().unwrap()
}

fn run(input: &[u8], output: &std::path::Path, env: Option<(&str, &str)>) -> std::process::Output {
    run_with_limit(input, output, env, 131_073)
}

#[test]
fn writes_only_bounded_safe_output_to_a_private_new_file() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("diagnostic");
    let input = vec![b'x'; 131073];
    let result = run(&input, &path, None);
    assert!(result.status.success(), "{:?}", result);
    assert_eq!(fs::read(&path).unwrap(), input);
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn oversized_safe_output_preserves_the_terminal_failure_with_an_omission_marker() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("diagnostic");
    let terminal = b"terminal build failure: executable was not found\n";
    let input = [vec![b'x'; 400_000], terminal.to_vec()].concat();

    let result = run(&input, &path, None);

    assert!(result.status.success(), "{:?}", result);
    let diagnostic = fs::read(&path).unwrap();
    assert!(diagnostic.len() <= 131_073);
    assert!(diagnostic
        .windows(terminal.len())
        .any(|window| window == terminal));
    assert!(
        String::from_utf8_lossy(&diagnostic).contains("[... middle diagnostic output omitted ...]")
    );
}

#[test]
fn rejects_limits_too_small_for_truthful_truncation_and_accepts_the_marker_boundary() {
    let temp = tempfile::tempdir().unwrap();
    for (index, limit) in [0, OMITTED.len() - 1].into_iter().enumerate() {
        let path = temp.path().join(format!("undersized-{index}"));
        let result = run_with_limit(b"oversized safe output", &path, None, limit);
        assert!(!result.status.success());
        assert!(!path.exists());
    }

    let path = temp.path().join("marker-boundary");
    let result = run_with_limit(&vec![b'x'; OMITTED.len() + 1], &path, None, OMITTED.len());
    assert!(result.status.success(), "{:?}", result);
    assert_eq!(fs::read(path).unwrap(), OMITTED);
}

#[test]
fn rejects_a_sensitive_keyword_split_across_input_chunks_without_artifact() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("diagnostic");
    let input = [vec![b'x'; 8190], b"secret=value\n".to_vec()].concat();

    let result = run(&input, &path, None);

    assert_eq!(result.status.code(), Some(42));
    assert!(!path.exists());
}

#[test]
fn refuses_existing_files_and_symlinks_without_modifying_them() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("target");
    fs::write(&target, b"owned").unwrap();
    for path in [temp.path().join("existing"), temp.path().join("link")] {
        if path.ends_with("link") {
            symlink(&target, &path).unwrap();
        } else {
            fs::write(&path, b"old").unwrap();
        }
        assert!(!run(b"safe", &path, None).status.success());
        assert_eq!(fs::read(&target).unwrap(), b"owned");
    }
}

#[test]
fn rejects_early_late_and_keyword_free_known_credentials_without_artifact() {
    let temp = tempfile::tempdir().unwrap();
    let cases = [
        (
            [
                b"Authorization: Bearer abc\n".to_vec(),
                vec![b'x'; 1_000_000],
            ]
            .concat(),
            None,
        ),
        (
            [vec![b'x'; 140_000], b"\nsecret=value\n".to_vec()].concat(),
            None,
        ),
        (
            b"prefix opaque-7391-value suffix\n".to_vec(),
            Some(("CUSTOM_BUILD_CREDENTIAL", "opaque-7391-value")),
        ),
    ];
    for (index, (input, env)) in cases.into_iter().enumerate() {
        let path = temp.path().join(format!("diagnostic-{index}"));
        let result = run(&input, &path, env);
        assert_eq!(result.status.code(), Some(42));
        assert!(!path.exists());
        assert!(result.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&result.stderr).contains("opaque-7391-value"));
    }
}

#[test]
fn recognizes_every_credential_policy_name_family_by_opaque_value() {
    let temp = tempfile::tempdir().unwrap();
    for (index, name) in [
        "GASCAMP_READ_TOKEN_FILE",
        "GITHUB_TOKEN",
        "GH_TOKEN",
        "GITLAB_TOKEN",
        "DOCKER_AUTH_CONFIG",
        "HTTP_AUTHORIZATION",
        "AUTHORIZATION",
        "AWS_ACCESS_KEY_ID",
        "AWS_SECRET_ACCESS_KEY",
        "AWS_SESSION_TOKEN",
        "BUILD_TOKEN",
        "BUILD_X_TOKEN",
        "X_BUILD_TOKEN",
        "X_BUILD_Y_TOKEN",
        "BUILD_CREDENTIAL",
        "X_BUILD_CREDENTIAL",
        "BUILD_PASSWORD",
        "X_BUILD_PASSWORD",
        "BUILD_SECRET",
        "X_BUILD_SECRET",
    ]
    .into_iter()
    .enumerate()
    {
        let value = format!("opaque-{index}-7391");
        let path = temp.path().join(format!("diagnostic-policy-{index}"));
        let result = run(
            format!("safe {value} safe").as_bytes(),
            &path,
            Some((name, &value)),
        );
        assert_eq!(result.status.code(), Some(42), "missed {name}");
        assert!(!path.exists());
    }
}
