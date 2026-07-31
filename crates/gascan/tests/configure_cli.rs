use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::process::{Command, Stdio};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const SENTINEL: &str = "task7-process-token-never-print-41fd8b";

struct ProcessFixture {
    _temporary: tempfile::TempDir,
    runtime: std::path::PathBuf,
    home: std::path::PathBuf,
}

impl ProcessFixture {
    fn new() -> TestResult<Self> {
        let temporary = tempfile::tempdir()?;
        let runtime = temporary.path().join("runtime");
        let home = temporary.path().join("home");
        fs::create_dir(&runtime)?;
        fs::create_dir(&home)?;
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700))?;
        Ok(Self {
            _temporary: temporary,
            runtime,
            home,
        })
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_gascan"));
        command
            .env("XDG_RUNTIME_DIR", &self.runtime)
            .env("HOME", &self.home)
            .env("GASCAN_DAEMON", "/definitely/missing/gascand")
            .env("NO_COLOR", "1")
            .env("TERM", "dumb")
            .env_remove("GH_TOKEN")
            .env_remove("GITHUB_TOKEN")
            .env_remove("GH_ENTERPRISE_TOKEN")
            .env_remove("GITHUB_ENTERPRISE_TOKEN")
            .env_remove("GITLAB_TOKEN")
            .env_remove("GITLAB_ACCESS_TOKEN")
            .env_remove("OAUTH_TOKEN");
        command
    }
}

#[test]
fn help_exposes_aggregate_and_focused_configure_forms() -> TestResult {
    for (arguments, expected) in [
        (vec!["configure", "--help"], "git"),
        (
            vec!["configure", "git", "--help"],
            "Usage: gascan configure git",
        ),
        (vec!["configure", "gh", "--help"], "--token-stdin"),
        (vec!["configure", "glab", "--help"], "--git-protocol"),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_gascan"))
            .args(arguments)
            .output()?;
        assert!(output.status.success(), "{output:?}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains(expected), "missing {expected:?}: {stdout}");
        assert!(output.stderr.is_empty(), "{output:?}");
    }
    Ok(())
}

#[test]
fn clap_rejects_token_argv_invalid_protocol_and_unexpected_values() -> TestResult {
    for arguments in [
        vec!["configure", "gh", "--token", SENTINEL],
        vec!["configure", "glab", "--token", SENTINEL],
        vec!["configure", "gh", "--git-protocol", "file"],
        vec!["configure", "git", "unexpected"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_gascan"))
            .args(arguments)
            .output()?;
        assert_eq!(output.status.code(), Some(64), "{output:?}");
        let rendered = [output.stdout, output.stderr].concat();
        assert!(
            !rendered
                .windows(SENTINEL.len())
                .any(|bytes| bytes == SENTINEL.as_bytes())
        );
    }
    Ok(())
}

#[test]
fn aggregate_refuses_redirected_input_before_daemon_connection() -> TestResult {
    let fixture = ProcessFixture::new()?;
    let output = fixture
        .command()
        .args(["--sandbox", "demo-0123456789ab", "configure"])
        .stdin(Stdio::null())
        .output()?;
    assert_eq!(output.status.code(), Some(64), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("interactive terminal"),
        "{output:?}"
    );
    Ok(())
}

#[test]
fn token_stdin_refuses_a_terminal_without_reading_it() -> TestResult {
    let fixture = ProcessFixture::new()?;
    let pty = rustix_openpty::openpty(None, None)?;
    let stdin = std::fs::File::from(rustix::io::dup(&pty.user)?);
    let mut controller = std::fs::File::from(rustix::io::dup(&pty.controller)?);
    use std::io::Write as _;
    controller.write_all(SENTINEL.as_bytes())?;
    let output = fixture
        .command()
        .args([
            "--sandbox",
            "demo-0123456789ab",
            "configure",
            "gh",
            "--token-stdin",
        ])
        .stdin(Stdio::from(stdin))
        .output()?;
    assert_eq!(output.status.code(), Some(64), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("omit --token-stdin"),
        "{output:?}"
    );
    let rendered = [output.stdout, output.stderr].concat();
    assert!(
        !rendered
            .windows(SENTINEL.len())
            .any(|bytes| bytes == SENTINEL.as_bytes())
    );
    Ok(())
}

#[test]
fn token_stdin_pipe_passes_input_validation_and_never_echoes_the_token() -> TestResult {
    let fixture = ProcessFixture::new()?;
    let mut child = fixture
        .command()
        .args([
            "--sandbox",
            "demo-0123456789ab",
            "configure",
            "glab",
            "--hostname",
            "gitlab.enterprise.test",
            "--token-stdin",
            "--git-protocol",
            "https",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    use std::io::Write as _;
    child
        .stdin
        .take()
        .ok_or("child stdin was unavailable")?
        .write_all(SENTINEL.as_bytes())?;
    let output = child.wait_with_output()?;
    assert_ne!(output.status.code(), Some(64), "{output:?}");
    let rendered = [output.stdout, output.stderr].concat();
    assert!(
        !rendered
            .windows(SENTINEL.len())
            .any(|bytes| bytes == SENTINEL.as_bytes())
    );
    Ok(())
}

#[test]
fn token_stdin_with_default_ssh_refuses_before_daemon_or_secret_forwarding() -> TestResult {
    let fixture = ProcessFixture::new()?;
    let mut child = fixture
        .command()
        .args([
            "--sandbox",
            "demo-0123456789ab",
            "configure",
            "gh",
            "--token-stdin",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    use std::io::Write as _;
    child
        .stdin
        .take()
        .ok_or("child stdin was unavailable")?
        .write_all(SENTINEL.as_bytes())?;
    let output = child.wait_with_output()?;
    assert_eq!(output.status.code(), Some(64), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("--git-protocol https"),
        "{output:?}"
    );
    let rendered = [output.stdout, output.stderr].concat();
    assert!(
        !rendered
            .windows(SENTINEL.len())
            .any(|bytes| bytes == SENTINEL.as_bytes())
    );
    Ok(())
}

#[test]
fn first_up_failed_operation_never_prints_or_runs_the_developer_offer() -> TestResult {
    let fixture = ProcessFixture::new()?;
    let root = fixture._temporary.path().join("failed-up-project");
    fs::create_dir(&root)?;
    fs::write(root.join("gascan.toml"), "version = 1\n")?;
    let root = root.to_str().ok_or("project root was not UTF-8")?;

    for arguments in [vec!["up", root], vec!["up", root, "--json"]] {
        let output = fixture
            .command()
            .args(arguments)
            .stdin(Stdio::null())
            .output()?;

        assert!(!output.status.success(), "{output:?}");
        let rendered = String::from_utf8([output.stdout, output.stderr].concat())?;
        assert!(!rendered.contains("Set up Git, GitHub, and GitLab"));
        assert!(!rendered.contains("developer setup was not completed"));
    }
    Ok(())
}
