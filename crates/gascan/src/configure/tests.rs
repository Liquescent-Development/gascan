use super::{
    ConfigureError, Forge, HostAccount, HostDiscovery, Prompter, SystemHostDiscovery,
    TerminalPrompter,
};
use std::fs;
use std::io::{Read as _, Write as _};
use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;

const SENTINEL: &str = "gascan-test-secret-7d9f3a";
const FORGE_TOKEN_NAMES: [&str; 7] = [
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GH_ENTERPRISE_TOKEN",
    "GITHUB_ENTERPRISE_TOKEN",
    "GITLAB_TOKEN",
    "GITLAB_ACCESS_TOKEN",
    "OAUTH_TOKEN",
];
type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

struct FakePrograms {
    _directory: tempfile::TempDir,
    bin: PathBuf,
}

impl FakePrograms {
    fn new() -> TestResult<Self> {
        let directory = tempfile::tempdir()?;
        let bin = directory.path().join("bin");
        fs::create_dir(&bin)?;
        Ok(Self {
            _directory: directory,
            bin,
        })
    }

    fn discovery(&self) -> SystemHostDiscovery {
        SystemHostDiscovery::with_program_directory(self.bin.clone())
    }

    fn install(&self, name: &str, body: &str) -> TestResult {
        let path = self.bin.join(name);
        let script = format!(
            concat!(
                "#!/bin/sh\n",
                "record=\"$(dirname \"$0\")/$(basename \"$0\").calls\"\n",
                "{{\n",
                "  /bin/echo \"cwd=$PWD\"\n",
                "  for argument in \"$@\"; do /usr/bin/printf 'arg=%s\\n' \"$argument\"; done\n",
                "  /usr/bin/printf 'GH_TOKEN=%s\\n' \"${{GH_TOKEN-unset}}\"\n",
                "  /usr/bin/printf 'GITHUB_TOKEN=%s\\n' \"${{GITHUB_TOKEN-unset}}\"\n",
                "  /usr/bin/printf 'GH_ENTERPRISE_TOKEN=%s\\n' \"${{GH_ENTERPRISE_TOKEN-unset}}\"\n",
                "  /usr/bin/printf 'GITHUB_ENTERPRISE_TOKEN=%s\\n' \"${{GITHUB_ENTERPRISE_TOKEN-unset}}\"\n",
                "  /usr/bin/printf 'GITLAB_TOKEN=%s\\n' \"${{GITLAB_TOKEN-unset}}\"\n",
                "  /usr/bin/printf 'GITLAB_ACCESS_TOKEN=%s\\n' \"${{GITLAB_ACCESS_TOKEN-unset}}\"\n",
                "  /usr/bin/printf 'OAUTH_TOKEN=%s\\n' \"${{OAUTH_TOKEN-unset}}\"\n",
                "  /bin/echo ---\n",
                "}} >> \"$record\"\n",
                "{body}\n",
            ),
            body = body,
        );
        fs::write(&path, script)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        Ok(())
    }

    fn calls(&self, name: &str) -> TestResult<Vec<Vec<String>>> {
        let contents = fs::read_to_string(self.bin.join(format!("{name}.calls")))?;
        Ok(contents
            .split("---\n")
            .filter(|record| !record.is_empty())
            .map(|record| record.lines().map(ToOwned::to_owned).collect())
            .collect())
    }
}

fn assert_scrubbed(record: &[String]) {
    for name in FORGE_TOKEN_NAMES {
        assert!(record.iter().any(|line| line == &format!("{name}=unset")));
    }
    assert!(!record.iter().any(|line| line.contains(SENTINEL)));
}

fn assert_error_redacted(error: &ConfigureError) {
    assert!(!error.to_string().contains(SENTINEL));
    assert!(!format!("{error:?}").contains(SENTINEL));
}

#[test]
fn git_defaults_use_only_global_config_from_root_and_scrub_forge_tokens() -> TestResult {
    let fake = FakePrograms::new()?;
    fake.install(
        "git",
        concat!(
            "if [ \"$*\" = \"config --global --get user.name\" ]; then /usr/bin/printf 'Ada Lovelace\\n'; exit 0; fi\n",
            "if [ \"$*\" = \"config --global --get user.email\" ]; then /usr/bin/printf 'ada@example.test\\n'; exit 0; fi\n",
            "exit 97",
        ),
    )?;
    let defaults = fake.discovery().git_defaults()?;
    assert_eq!(defaults.name.as_deref(), Some("Ada Lovelace"));
    assert_eq!(defaults.email.as_deref(), Some("ada@example.test"));

    let calls = fake.calls("git")?;
    assert_eq!(calls.len(), 2);
    assert_eq!(
        calls[0][..4],
        ["cwd=/", "arg=config", "arg=--global", "arg=--get"]
    );
    assert_eq!(calls[0][4], "arg=user.name");
    assert_eq!(
        calls[1][..5],
        [
            "cwd=/",
            "arg=config",
            "arg=--global",
            "arg=--get",
            "arg=user.email",
        ]
    );
    assert!(calls.iter().flatten().all(|line| line != "arg=--local"));
    for call in &calls {
        assert_scrubbed(call);
    }
    Ok(())
}

#[test]
fn missing_global_git_values_are_none_and_other_git_failures_are_stable() -> TestResult {
    let fake = FakePrograms::new()?;
    fake.install(
        "git",
        "if [ \"$5\" = \"user.name\" ]; then exit 1; fi\nexit 1",
    )?;
    let defaults = fake.discovery().git_defaults()?;
    assert_eq!(defaults.name, None);
    assert_eq!(defaults.email, None);

    let missing = FakePrograms::new()?;
    let error = missing
        .discovery()
        .git_defaults()
        .err()
        .ok_or("missing git succeeded")?;
    assert_error_redacted(&error);
    assert!(error.to_string().contains("Git"));
    Ok(())
}

#[test]
fn github_accounts_parse_multiple_enterprise_hosts_and_exact_machine_command() -> TestResult {
    let fake = FakePrograms::new()?;
    fake.install(
        "gh",
        concat!(
            "if [ \"$*\" = \"auth status --json hosts\" ]; then\n",
            "  /bin/cat <<'JSON'\n",
            "{\"hosts\":{\"github.com\":[{\"active\":true,\"host\":\"github.com\",\"login\":\"ada\",\"scopes\":\"repo\",\"tokenSource\":\"keyring\"}],\"github.enterprise.test\":[{\"active\":true,\"host\":\"github.enterprise.test\",\"login\":\"grace\",\"scopes\":\"repo\",\"tokenSource\":\"keyring\"}]}}\n",
            "JSON\n",
            "  exit 0\n",
            "fi\nexit 97",
        ),
    )?;
    let accounts = fake.discovery().accounts(Forge::GitHub)?;
    assert_eq!(
        accounts
            .iter()
            .map(|account| (account.hostname.as_str(), account.login.as_deref()))
            .collect::<Vec<_>>(),
        vec![
            ("github.com", Some("ada")),
            ("github.enterprise.test", Some("grace")),
        ]
    );
    let calls = fake.calls("gh")?;
    assert_eq!(
        calls[0][..5],
        ["cwd=/", "arg=auth", "arg=status", "arg=--json", "arg=hosts",]
    );
    assert_scrubbed(&calls[0]);
    Ok(())
}

#[test]
fn github_unauthenticated_is_empty_and_malformed_or_ambiguous_json_fails_closed() -> TestResult {
    for (json, exit, should_succeed) in [
        ("{\"hosts\":{}}", 1, true),
        ("not-json", 0, false),
        (
            "{\"hosts\":{\"github.com\":[{\"active\":true,\"host\":\"other.example\",\"login\":\"ada\"}]}}",
            0,
            false,
        ),
        (
            "{\"hosts\":{\"github.com\":[{\"active\":true,\"host\":\"github.com\",\"login\":\"\"}]}}",
            0,
            false,
        ),
    ] {
        let fake = FakePrograms::new()?;
        fs::write(fake.bin.join("status.json"), json)?;
        fake.install(
            "gh",
            &format!("/bin/cat \"$(dirname \"$0\")/status.json\"; exit {exit}"),
        )?;
        let result = fake.discovery().accounts(Forge::GitHub);
        if should_succeed {
            assert!(result?.is_empty());
        } else {
            let error = result.err().ok_or("malformed GitHub status succeeded")?;
            assert!(matches!(error, ConfigureError::InvalidOutput { .. }));
            assert_error_redacted(&error);
        }
    }
    Ok(())
}

#[test]
fn gitlab_accounts_require_explicit_authenticated_records_for_each_host() -> TestResult {
    let fake = FakePrograms::new()?;
    fs::write(
        fake.bin.join("status.txt"),
        concat!(
            "gitlab.com\n",
            "  ✓ Logged in to gitlab.com as ada (/home/ada/.config/glab-cli/config.yml)\n",
            "  ✓ API calls for gitlab.com are made over https protocol.\n",
            "gitlab.enterprise.test\n",
            "  ✓ Logged in to gitlab.enterprise.test as grace (/home/grace/.config/glab-cli/config.yml)\n",
        ),
    )?;
    fake.install(
        "glab",
        "if [ \"$*\" = \"auth status --all\" ]; then /bin/cat \"$(dirname \"$0\")/status.txt\"; exit 0; fi\nexit 97",
    )?;
    let accounts = fake.discovery().accounts(Forge::GitLab)?;
    assert_eq!(
        accounts
            .iter()
            .map(|account| (account.hostname.as_str(), account.login.as_deref()))
            .collect::<Vec<_>>(),
        vec![
            ("gitlab.com", Some("ada")),
            ("gitlab.enterprise.test", Some("grace")),
        ]
    );
    let calls = fake.calls("glab")?;
    assert_eq!(
        calls[0][..4],
        ["cwd=/", "arg=auth", "arg=status", "arg=--all"]
    );
    assert_scrubbed(&calls[0]);
    Ok(())
}

#[test]
fn gitlab_unauthenticated_is_empty_and_ambiguous_status_fails_closed() -> TestResult {
    for (status, exit, should_succeed) in [
        (
            "You are not logged into any GitLab hosts. Run glab auth login to authenticate.\n",
            1,
            true,
        ),
        ("", 0, false),
        ("gitlab.com\n  Logged in somehow as ada\n", 0, false),
        (
            "gitlab.com\n  ✓ Logged in to other.example as ada (/tmp/config.yml)\n",
            0,
            false,
        ),
        ("unstructured authenticated output\n", 0, false),
    ] {
        let fake = FakePrograms::new()?;
        fs::write(fake.bin.join("status.txt"), status)?;
        fake.install(
            "glab",
            &format!("/bin/cat \"$(dirname \"$0\")/status.txt\"; exit {exit}"),
        )?;
        let result = fake.discovery().accounts(Forge::GitLab);
        if should_succeed {
            assert!(result?.is_empty());
        } else {
            let error = result.err().ok_or("ambiguous GitLab status succeeded")?;
            assert_error_redacted(&error);
        }
    }
    Ok(())
}

#[test]
fn missing_github_and_gitlab_executables_return_redacted_host_errors() -> TestResult {
    for forge in [Forge::GitHub, Forge::GitLab] {
        let fake = FakePrograms::new()?;
        let error = fake
            .discovery()
            .accounts(forge)
            .err()
            .ok_or("missing forge executable succeeded")?;
        assert!(matches!(error, ConfigureError::HostCommand { .. }));
        assert_error_redacted(&error);
    }
    Ok(())
}

#[test]
fn token_retrieval_uses_exact_host_arguments_and_keeps_secret_off_observable_surfaces() -> TestResult
{
    let fake = FakePrograms::new()?;
    fake.install("gh", &format!("/usr/bin/printf '{SENTINEL}\\n'; exit 0"))?;
    fake.install("glab", &format!("/usr/bin/printf '{SENTINEL}\\n'; exit 0"))?;
    let discovery = fake.discovery();
    let github = HostAccount {
        hostname: "github.enterprise.test".to_owned(),
        login: Some("ada".to_owned()),
    };
    let gitlab = HostAccount {
        hostname: "gitlab.enterprise.test".to_owned(),
        login: Some("grace".to_owned()),
    };
    let github_token = discovery.token(Forge::GitHub, &github)?;
    let gitlab_token = discovery.token(Forge::GitLab, &gitlab)?;
    assert_eq!(github_token.expose(), SENTINEL.as_bytes());
    assert_eq!(gitlab_token.expose(), SENTINEL.as_bytes());
    assert_eq!(format!("{github_token:?}"), "Secret([REDACTED])");

    let gh_calls = fake.calls("gh")?;
    assert_eq!(
        gh_calls[0][..6],
        [
            "cwd=/",
            "arg=auth",
            "arg=token",
            "arg=--hostname",
            "arg=github.enterprise.test",
            "GH_TOKEN=unset",
        ]
    );
    let glab_calls = fake.calls("glab")?;
    assert_eq!(
        glab_calls[0][..9],
        [
            "cwd=/",
            "arg=config",
            "arg=get",
            "arg=token",
            "arg=--global",
            "arg=--host",
            "arg=gitlab.enterprise.test",
            "GH_TOKEN=unset",
            "GITHUB_TOKEN=unset",
        ]
    );
    assert_scrubbed(&gh_calls[0]);
    assert_scrubbed(&glab_calls[0]);

    let failing = FakePrograms::new()?;
    failing.install("gh", &format!("/usr/bin/printf '{SENTINEL}' >&2; exit 4"))?;
    let error = failing
        .discovery()
        .token(Forge::GitHub, &github)
        .err()
        .ok_or("failed token command succeeded")?;
    assert_error_redacted(&error);
    Ok(())
}

#[test]
fn ambient_forge_tokens_are_scrubbed_and_retrieved_token_is_not_forwarded() -> TestResult {
    let fake = FakePrograms::new()?;
    fake.install("gh", &format!("/usr/bin/printf '{SENTINEL}\\n'; exit 0"))?;
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command
        .args([
            "--exact",
            "configure::tests::ambient_forge_token_child",
            "--nocapture",
        ])
        .env("GASCAN_CONFIGURE_HOST_CHILD_BIN", &fake.bin);
    for name in FORGE_TOKEN_NAMES {
        command.env(name, SENTINEL);
    }
    let output = command.output()?;
    assert!(output.status.success());
    assert!(
        !output
            .stdout
            .windows(SENTINEL.len())
            .any(|window| window == SENTINEL.as_bytes())
    );
    assert!(
        !output
            .stderr
            .windows(SENTINEL.len())
            .any(|window| window == SENTINEL.as_bytes())
    );
    let calls = fake.calls("gh")?;
    assert_eq!(calls.len(), 1);
    assert_scrubbed(&calls[0]);
    Ok(())
}

#[test]
fn ambient_forge_token_child() -> TestResult {
    let Some(bin) = std::env::var_os("GASCAN_CONFIGURE_HOST_CHILD_BIN") else {
        return Ok(());
    };
    let discovery = SystemHostDiscovery::with_program_directory(PathBuf::from(bin));
    let account = HostAccount {
        hostname: "github.com".to_owned(),
        login: Some("ada".to_owned()),
    };
    let token = discovery.token(Forge::GitHub, &account)?;
    assert_eq!(token.expose().len(), SENTINEL.len());
    Ok(())
}

#[test]
fn command_output_over_one_mibibyte_is_rejected_without_echoing_it() -> TestResult {
    for redirect in ["", " >&2"] {
        let fake = FakePrograms::new()?;
        fs::write(fake.bin.join("large"), vec![b'x'; 1024 * 1024 + 1])?;
        fake.install(
            "gh",
            &format!("/bin/cat \"$(dirname \"$0\")/large\"{redirect}; exit 0"),
        )?;
        let error = fake
            .discovery()
            .accounts(Forge::GitHub)
            .err()
            .ok_or("oversized host output succeeded")?;
        assert!(matches!(error, ConfigureError::InvalidOutput { .. }));
        assert_error_redacted(&error);
    }
    Ok(())
}

fn duplicate_file(fd: impl std::os::fd::AsFd) -> std::io::Result<fs::File> {
    Ok(fs::File::from(rustix::io::dup(fd)?))
}

fn normalized_termios(fd: impl std::os::fd::AsFd) -> std::io::Result<rustix::termios::Termios> {
    let mut state = rustix::termios::tcgetattr(fd)?;
    state
        .local_modes
        .remove(rustix::termios::LocalModes::PENDIN);
    Ok(state)
}

fn assert_termios_equal(actual: &rustix::termios::Termios, expected: &rustix::termios::Termios) {
    assert_eq!(actual.input_modes, expected.input_modes);
    assert_eq!(actual.output_modes, expected.output_modes);
    assert_eq!(actual.control_modes, expected.control_modes);
    assert_eq!(actual.local_modes, expected.local_modes);
}

fn wait_for_echo(fd: impl std::os::fd::AsFd, enabled: bool) -> std::io::Result<()> {
    for _ in 0..500 {
        let state = rustix::termios::tcgetattr(fd.as_fd())?;
        if state
            .local_modes
            .contains(rustix::termios::LocalModes::ECHO)
            == enabled
        {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        "terminal echo state did not change",
    ))
}

fn read_available(controller: &mut fs::File) -> std::io::Result<Vec<u8>> {
    let original = rustix::fs::fcntl_getfl(&*controller)?;
    rustix::fs::fcntl_setfl(&*controller, original | rustix::fs::OFlags::NONBLOCK)?;
    let mut output = Vec::new();
    let mut bytes = [0_u8; 4096];
    for _ in 0..100 {
        match controller.read(&mut bytes) {
            Ok(0) => break,
            Ok(count) => output.extend_from_slice(&bytes[..count]),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            Err(error) => return Err(error),
        }
    }
    rustix::fs::fcntl_setfl(&*controller, original)?;
    Ok(output)
}

fn pty_prompter(pty: &rustix_openpty::Pty) -> std::io::Result<(TerminalPrompter, fs::File)> {
    let input = duplicate_file(&pty.user)?;
    let output = duplicate_file(&pty.user)?;
    let controller = duplicate_file(&pty.controller)?;
    Ok((TerminalPrompter::from_files(input, output), controller))
}

#[test]
fn ordinary_line_input_echoes_and_honors_defaults() -> TestResult {
    let pty = rustix_openpty::openpty(None, None)?;
    let saved = normalized_termios(&pty.user)?;
    let (mut prompt, mut controller) = pty_prompter(&pty)?;
    let reader = std::thread::spawn(move || prompt.line("Name: ", Some("Default")));
    std::thread::sleep(std::time::Duration::from_millis(20));
    controller.write_all(b"Ada\n")?;
    let value = reader.join().map_err(|_| "line reader panicked")??;
    let output = read_available(&mut controller)?;
    assert_eq!(value.as_deref(), Some("Ada"));
    assert!(output.windows(3).any(|window| window == b"Ada"));
    assert_termios_equal(&normalized_termios(&pty.user)?, &saved);
    Ok(())
}

#[test]
fn hidden_secret_does_not_echo_and_preserves_non_trailing_bytes() -> TestResult {
    let pty = rustix_openpty::openpty(None, None)?;
    let mut state = rustix::termios::tcgetattr(&pty.user)?;
    state.make_raw();
    state.local_modes.insert(rustix::termios::LocalModes::ECHO);
    rustix::termios::tcsetattr(&pty.user, rustix::termios::OptionalActions::Now, &state)?;
    let saved = normalized_termios(&pty.user)?;
    let (mut prompt, mut controller) = pty_prompter(&pty)?;
    let reader = std::thread::spawn(move || prompt.secret("Token: "));
    wait_for_echo(&pty.user, false)?;
    let during = rustix::termios::tcgetattr(&pty.user)?;
    let mut expected = state.clone();
    expected
        .local_modes
        .remove(rustix::termios::LocalModes::ECHO);
    assert_eq!(during.input_modes, expected.input_modes);
    assert_eq!(during.output_modes, expected.output_modes);
    assert_eq!(during.control_modes, expected.control_modes);
    assert_eq!(during.local_modes, expected.local_modes);

    let mut entered = format!("  {SENTINEL}").into_bytes();
    entered.extend_from_slice(&[4, b' ', b' ', b'\r', b'\n']);
    controller.write_all(&entered)?;
    let secret = reader
        .join()
        .map_err(|_| "secret reader panicked")??
        .ok_or("secret unexpectedly cancelled")?;
    let mut expected_secret = format!("  {SENTINEL}").into_bytes();
    expected_secret.extend_from_slice(&[4, b' ', b' ']);
    assert_eq!(secret.expose(), expected_secret);
    let output = read_available(&mut controller)?;
    assert!(output.windows(7).any(|window| window == b"Token: "));
    assert!(
        !output
            .windows(SENTINEL.len())
            .any(|window| window == SENTINEL.as_bytes())
    );
    assert_termios_equal(&normalized_termios(&pty.user)?, &saved);
    Ok(())
}

#[test]
fn hidden_secret_eof_ctrl_c_and_empty_input_cancel_and_restore() -> TestResult {
    for (input, raw) in [
        (b"\x04".as_slice(), false),
        (b"\x03".as_slice(), true),
        (b"\n".as_slice(), true),
    ] {
        let pty = rustix_openpty::openpty(None, None)?;
        let mut state = rustix::termios::tcgetattr(&pty.user)?;
        if raw {
            state.make_raw();
            state.local_modes.insert(rustix::termios::LocalModes::ECHO);
        }
        rustix::termios::tcsetattr(&pty.user, rustix::termios::OptionalActions::Now, &state)?;
        let saved = normalized_termios(&pty.user)?;
        let (mut prompt, mut controller) = pty_prompter(&pty)?;
        let reader = std::thread::spawn(move || prompt.secret("Token: "));
        wait_for_echo(&pty.user, false)?;
        controller.write_all(input)?;
        let result = reader.join().map_err(|_| "secret reader panicked")?;
        assert!(matches!(result, Err(ConfigureError::Cancelled)));
        assert_termios_equal(&normalized_termios(&pty.user)?, &saved);
    }
    Ok(())
}

#[test]
#[allow(clippy::panic, reason = "test-only unwind is the behavior under test")]
fn hidden_secret_read_failure_and_unwind_restore_terminal() -> TestResult {
    let pty = rustix_openpty::openpty(None, None)?;
    let saved = normalized_termios(&pty.user)?;
    let (mut prompt, controller) = pty_prompter(&pty)?;
    let reader = std::thread::spawn(move || prompt.secret("Token: "));
    wait_for_echo(&pty.user, false)?;
    drop(controller);
    drop(pty.controller);
    let error = reader
        .join()
        .map_err(|_| "secret reader panicked")?
        .err()
        .ok_or("hung-up PTY unexpectedly succeeded")?;
    assert!(matches!(error, ConfigureError::Io(_)));
    assert_termios_equal(&normalized_termios(&pty.user)?, &saved);

    let unwind_pty = rustix_openpty::openpty(None, None)?;
    let unwind_saved = normalized_termios(&unwind_pty.user)?;
    let result = std::panic::catch_unwind(|| {
        let _guard =
            super::prompt::HiddenInput::acquire(&unwind_pty.user).map_err(std::io::Error::other)?;
        std::panic::panic_any("test-only unwind");
        #[allow(unreachable_code)]
        Ok::<(), std::io::Error>(())
    });
    assert!(result.is_err());
    assert_termios_equal(&normalized_termios(&unwind_pty.user)?, &unwind_saved);
    Ok(())
}

#[test]
fn actual_sigint_cancels_hidden_input_and_restores_terminal() -> TestResult {
    let output = std::process::Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            "configure::tests::actual_sigint_child",
            "--nocapture",
        ])
        .env("GASCAN_CONFIGURE_SIGINT_CHILD", "1")
        .output()?;
    assert!(output.status.success());
    Ok(())
}

#[test]
fn actual_sigint_child() -> TestResult {
    if std::env::var_os("GASCAN_CONFIGURE_SIGINT_CHILD").is_none() {
        return Ok(());
    }
    let pty = rustix_openpty::openpty(None, None)?;
    let saved = normalized_termios(&pty.user)?;
    let (mut prompt, _controller) = pty_prompter(&pty)?;
    let reader = std::thread::spawn(move || prompt.secret("Token: "));
    wait_for_echo(&pty.user, false)?;
    std::thread::sleep(std::time::Duration::from_millis(20));
    rustix::process::kill_process(rustix::process::getpid(), rustix::process::Signal::INT)?;
    let result = reader.join().map_err(|_| "secret reader panicked")?;
    assert!(matches!(result, Err(ConfigureError::Cancelled)));
    assert_termios_equal(&normalized_termios(&pty.user)?, &saved);
    Ok(())
}
