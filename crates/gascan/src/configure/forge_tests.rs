use super::{
    ConfigureError, Forge, ForgeRequest, ForgeSetup, GitProtocol, GitSetup, RegistrationState,
    configure_forge,
};
use crate::cli::CliError;
use crate::guest::{GuestCommand, GuestOutput, GuestRunner, Secret};
use gascan_proto::v1;
use std::collections::VecDeque;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const SANDBOX_ID: &str = "demo-0123456789ab";
const SENTINEL: &str = "task6-sentinel-token-never-disclose-7f309b";
const PUBLIC_KEY: &str = concat!(
    "ssh-ed25519 ",
    "AAAAC3NzaC1lZDI1NTE5AAAAIAEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEB",
    " gascan-demo-0123456789ab"
);
const SAME_KEY_OTHER_COMMENT: &str = concat!(
    "ssh-ed25519 ",
    "AAAAC3NzaC1lZDI1NTE5AAAAIAEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEB",
    " unrelated-title-and-comment"
);

struct RecordedCommand {
    selector: v1::SandboxSelector,
    argv: Vec<Vec<u8>>,
    environment: Vec<v1::EnvironmentVariable>,
    stdin: Option<Vec<u8>>,
}

#[derive(Default)]
struct FakeGuestRunner {
    outputs: VecDeque<Result<GuestOutput, CliError>>,
    interactive_results: VecDeque<Result<i32, CliError>>,
    commands: Vec<RecordedCommand>,
    interactive_commands: Vec<(v1::SandboxSelector, Vec<Vec<u8>>)>,
}

impl FakeGuestRunner {
    fn with_outputs(outputs: impl IntoIterator<Item = GuestOutput>) -> Self {
        Self {
            outputs: outputs.into_iter().map(Ok).collect(),
            ..Self::default()
        }
    }

    fn with_steps(
        outputs: impl IntoIterator<Item = Result<GuestOutput, CliError>>,
        interactive_results: impl IntoIterator<Item = Result<i32, CliError>>,
    ) -> Self {
        Self {
            outputs: outputs.into_iter().collect(),
            interactive_results: interactive_results.into_iter().collect(),
            ..Self::default()
        }
    }
}

#[tonic::async_trait]
impl GuestRunner for FakeGuestRunner {
    async fn execute(
        &mut self,
        selector: v1::SandboxSelector,
        command: GuestCommand,
    ) -> Result<GuestOutput, CliError> {
        self.commands.push(RecordedCommand {
            selector,
            argv: command.argv,
            environment: command.environment,
            stdin: command.stdin.map(|secret| secret.expose().to_vec()),
        });
        self.outputs
            .pop_front()
            .unwrap_or_else(|| Err(CliError::Runtime("unexpected guest command".to_owned())))
    }

    async fn execute_interactive(
        &mut self,
        selector: v1::SandboxSelector,
        argv: Vec<Vec<u8>>,
    ) -> Result<i32, CliError> {
        self.interactive_commands.push((selector, argv));
        self.interactive_results.pop_front().unwrap_or_else(|| {
            Err(CliError::Runtime(
                "unexpected interactive command".to_owned(),
            ))
        })
    }
}

fn selector() -> v1::SandboxSelector {
    v1::SandboxSelector {
        sandbox_id: SANDBOX_ID.to_owned(),
    }
}

fn request(forge: Forge, hostname: &str, protocol: GitProtocol) -> ForgeRequest {
    ForgeRequest {
        forge,
        hostname: hostname.to_owned(),
        protocol,
        token: Secret::new(SENTINEL.as_bytes().to_vec()),
        key: GitSetup {
            name: "Ada Lovelace".to_owned(),
            email: "ada@example.test".to_owned(),
            protocol,
            public_key: PUBLIC_KEY.to_owned(),
            fingerprint: "SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_owned(),
        },
    }
}

fn output(code: i32, stdout: impl Into<Vec<u8>>, stderr: impl Into<Vec<u8>>) -> GuestOutput {
    GuestOutput {
        code,
        stdout: stdout.into(),
        stderr: stderr.into(),
    }
}

fn github_status(hostname: &str, login: &str, protocol: GitProtocol) -> GuestOutput {
    let protocol = match protocol {
        GitProtocol::Ssh => "ssh",
        GitProtocol::Https => "https",
    };
    output(
        0,
        [],
        format!(
            "{hostname}\n  ✓ Logged in to {hostname} account {login} (/home/workspace/.config/gh/hosts.yml)\n  - Active account: true\n  - Git operations protocol: {protocol}\n"
        ),
    )
}

fn gitlab_status(hostname: &str, login: &str, protocol: GitProtocol) -> GuestOutput {
    let protocol = match protocol {
        GitProtocol::Ssh => "ssh",
        GitProtocol::Https => "https",
    };
    output(
        0,
        format!(
            "{hostname}\n  ✓ Logged in to {hostname} as {login} (/home/workspace/.config/glab-cli/config.yml)\n  ✓ Git operations for {hostname} configured to use {protocol} protocol.\n"
        ),
        [],
    )
}

fn github_key(key: &str) -> String {
    format!(
        "{{\"id\":17,\"key\":\"{key}\",\"title\":\"unrelated title\",\"verified\":true,\"created_at\":\"2026-07-30T00:00:00Z\",\"read_only\":false}}"
    )
}

fn github_keys(keys: &[&str]) -> GuestOutput {
    output(0, format!("[{}]", keys.join(",")), [])
}

fn gitlab_key(key: &str, usage_type: &str) -> String {
    format!(
        "{{\"id\":23,\"title\":\"unrelated title\",\"key\":\"{key}\",\"created_at\":\"2026-07-30T00:00:00Z\",\"expires_at\":null,\"last_used_at\":null,\"usage_type\":\"{usage_type}\"}}"
    )
}

fn gitlab_keys(keys: &[&str]) -> GuestOutput {
    output(0, format!("[{}]", keys.join(",")), [])
}

fn github_ssh_success(login: &str) -> GuestOutput {
    output(
        1,
        [],
        format!(
            "Hi {login}! You've successfully authenticated, but GitHub does not provide shell access.\n"
        ),
    )
}

fn gitlab_ssh_success(login: &str) -> GuestOutput {
    output(0, format!("Welcome to GitLab, @{login}!\n"), [])
}

fn github_environment() -> Vec<(&'static str, &'static str)> {
    vec![("GH_NO_UPDATE_NOTIFIER", "1"), ("NO_COLOR", "1")]
}

fn gitlab_environment() -> Vec<(&'static str, &'static str)> {
    vec![("GLAB_CHECK_UPDATE", "0"), ("NO_COLOR", "1")]
}

fn environment(command: &RecordedCommand) -> Vec<(&str, &str)> {
    command
        .environment
        .iter()
        .map(|variable| (variable.name.as_str(), variable.value.as_str()))
        .collect()
}

fn argv(parts: &[&str]) -> Vec<Vec<u8>> {
    parts.iter().map(|part| part.as_bytes().to_vec()).collect()
}

fn assert_command(
    command: &RecordedCommand,
    expected_argv: &[&str],
    expected_environment: &[(&str, &str)],
    expected_stdin: Option<&[u8]>,
) {
    assert_eq!(command.selector, selector());
    assert_eq!(command.argv, argv(expected_argv));
    assert_eq!(environment(command), expected_environment);
    assert_eq!(command.stdin.as_deref(), expected_stdin);
}

fn assert_setup(
    setup: &ForgeSetup,
    forge: Forge,
    hostname: &str,
    login: &str,
    authenticated: bool,
    authentication_key: RegistrationState,
    signing_key: RegistrationState,
) {
    assert_eq!(setup.forge, forge);
    assert_eq!(setup.hostname, hostname);
    assert_eq!(setup.login, login);
    assert_eq!(setup.authenticated, authenticated);
    assert_eq!(setup.authentication_key, authentication_key);
    assert_eq!(setup.signing_key, signing_key);
    assert!(!format!("{setup:?}").contains(SENTINEL));
}

fn forge_error(
    error: ConfigureError,
    hostname: &str,
    retry: &str,
) -> Result<ForgeSetup, Box<dyn std::error::Error>> {
    let display = format!("{error}");
    let debug = format!("{error:?}");
    assert!(display.contains(hostname));
    assert!(display.contains(retry));
    assert!(!display.contains(SENTINEL));
    assert!(!debug.contains(SENTINEL));
    match error {
        ConfigureError::Forge { setup, .. } => Ok(*setup),
        other => Err(format!("expected structured forge error, got {other:?}").into()),
    }
}

#[tokio::test]
async fn github_existing_matching_roles_compare_key_body_not_title_or_comment() -> TestResult {
    let authentication = github_key(SAME_KEY_OTHER_COMMENT);
    let signing = github_key(SAME_KEY_OTHER_COMMENT);
    let mut runner = FakeGuestRunner::with_outputs([
        output(0, "authentication complete\n", []),
        github_status("github.com", "octocat", GitProtocol::Https),
        github_keys(&[&authentication]),
        github_keys(&[&signing]),
    ]);

    let setup = configure_forge(
        &mut runner,
        selector(),
        request(Forge::GitHub, "github.com", GitProtocol::Https),
    )
    .await?;

    assert_setup(
        &setup,
        Forge::GitHub,
        "github.com",
        "octocat",
        true,
        RegistrationState::Existing,
        RegistrationState::Existing,
    );
    assert_eq!(runner.commands.len(), 4);
    assert_command(
        &runner.commands[0],
        &[
            "gh",
            "auth",
            "login",
            "--hostname",
            "github.com",
            "--git-protocol",
            "https",
            "--with-token",
        ],
        &github_environment(),
        Some(SENTINEL.as_bytes()),
    );
    assert_command(
        &runner.commands[1],
        &["gh", "auth", "status", "--hostname", "github.com"],
        &github_environment(),
        None,
    );
    assert_command(
        &runner.commands[2],
        &["gh", "api", "--hostname", "github.com", "user/keys"],
        &github_environment(),
        None,
    );
    assert_command(
        &runner.commands[3],
        &[
            "gh",
            "api",
            "--hostname",
            "github.com",
            "user/ssh_signing_keys",
        ],
        &github_environment(),
        None,
    );
    assert!(runner.interactive_commands.is_empty());
    Ok(())
}

#[tokio::test]
async fn github_login_uses_gh_2_45_compatible_arguments() -> TestResult {
    let mut runner = FakeGuestRunner::with_outputs([output(1, [], "authentication failed\n")]);
    let error = match configure_forge(
        &mut runner,
        selector(),
        request(Forge::GitHub, "github.com", GitProtocol::Ssh),
    )
    .await
    {
        Err(error) => error,
        Ok(setup) => return Err(format!("expected failure, got {setup:?}").into()),
    };

    assert_command(
        &runner.commands[0],
        &[
            "gh",
            "auth",
            "login",
            "--hostname",
            "github.com",
            "--git-protocol",
            "ssh",
            "--with-token",
        ],
        &github_environment(),
        Some(SENTINEL.as_bytes()),
    );
    assert!(
        !runner.commands[0]
            .argv
            .iter()
            .any(|argument| argument == b"--skip-ssh-key")
    );
    drop(error);
    Ok(())
}

#[tokio::test]
async fn native_login_failure_is_useful_bounded_and_secret_free() -> TestResult {
    let stderr =
        format!("HTTP 401: bad credentials\n{SENTINEL}\n\x1b]8;;https://evil.test\x07click\n");
    let mut runner = FakeGuestRunner::with_outputs([output(1, [], stderr)]);
    let error = match configure_forge(
        &mut runner,
        selector(),
        request(Forge::GitHub, "github.com", GitProtocol::Ssh),
    )
    .await
    {
        Err(error) => error,
        Ok(setup) => return Err(format!("expected failure, got {setup:?}").into()),
    };
    let rendered = format!("{error}");
    assert!(rendered.contains("HTTP 401: bad credentials"));
    assert!(!rendered.contains(SENTINEL));
    assert!(!rendered.contains('\x1b'));
    assert!(rendered.contains("gascan configure gh"));
    Ok(())
}

#[tokio::test]
async fn github_status_selects_the_active_account_in_the_requested_protocol() -> TestResult {
    let authentication = github_key(SAME_KEY_OTHER_COMMENT);
    let signing = github_key(SAME_KEY_OTHER_COMMENT);
    let status = output(
        0,
        [],
        concat!(
            "github.com\n",
            "  ✓ Logged in to github.com account stale-user (/home/workspace/.config/gh/hosts.yml)\n",
            "  - Active account: false\n",
            "  - Git operations protocol: ssh\n",
            "  ✓ Logged in to github.com account octocat (/home/workspace/.config/gh/hosts.yml)\n",
            "  - Active account: true\n",
            "  - Git operations protocol: https\n",
        ),
    );
    let mut runner = FakeGuestRunner::with_outputs([
        output(0, [], []),
        status,
        github_keys(&[&authentication]),
        github_keys(&[&signing]),
    ]);

    let setup = configure_forge(
        &mut runner,
        selector(),
        request(Forge::GitHub, "github.com", GitProtocol::Https),
    )
    .await?;

    assert_eq!(setup.login, "octocat");
    assert!(setup.authenticated);
    Ok(())
}

#[tokio::test]
async fn forge_status_requires_the_requested_git_protocol() -> TestResult {
    let mut github = FakeGuestRunner::with_outputs([
        output(0, [], []),
        github_status("github.com", "octocat", GitProtocol::Ssh),
    ]);
    let github_error = configure_forge(
        &mut github,
        selector(),
        request(Forge::GitHub, "github.com", GitProtocol::Https),
    )
    .await
    .err()
    .ok_or("GitHub status with the wrong protocol succeeded")?;
    assert_eq!(
        format!("{github_error}"),
        "GitHub authentication for github.com failed: native authentication could not be verified; retry with `gascan configure gh`"
    );
    assert_eq!(github.commands.len(), 2);

    let mut gitlab = FakeGuestRunner::with_outputs([
        output(0, [], []),
        gitlab_status("gitlab.com", "tanuki", GitProtocol::Ssh),
    ]);
    let gitlab_error = configure_forge(
        &mut gitlab,
        selector(),
        request(Forge::GitLab, "gitlab.com", GitProtocol::Https),
    )
    .await
    .err()
    .ok_or("GitLab status with the wrong protocol succeeded")?;
    assert_eq!(
        format!("{gitlab_error}"),
        "GitLab authentication for gitlab.com failed: native authentication could not be verified; retry with `gascan configure glab`"
    );
    assert_eq!(gitlab.commands.len(), 2);
    Ok(())
}

#[tokio::test]
async fn github_registers_both_roles_on_enterprise_and_verifies_ssh_visibly_then_bounded()
-> TestResult {
    let host = "github.enterprise.test";
    let created = github_key(PUBLIC_KEY);
    let mut runner = FakeGuestRunner::with_steps(
        [
            Ok(output(0, [], [])),
            Ok(github_status(host, "enterprise-user", GitProtocol::Ssh)),
            Ok(github_keys(&[])),
            Ok(github_keys(&[])),
            Ok(output(0, [], [])),
            Ok(output(0, created.as_bytes(), [])),
            Ok(output(0, created.as_bytes(), [])),
            Ok(github_ssh_success("enterprise-user")),
        ],
        [Ok(1)],
    );

    let setup = configure_forge(
        &mut runner,
        selector(),
        request(Forge::GitHub, host, GitProtocol::Ssh),
    )
    .await?;

    assert_setup(
        &setup,
        Forge::GitHub,
        host,
        "enterprise-user",
        true,
        RegistrationState::Added,
        RegistrationState::Added,
    );
    assert_eq!(runner.commands.len(), 8);
    assert_command(
        &runner.commands[4],
        &[
            "/usr/local/bin/configure-developer-home",
            "ssh-host",
            "--hostname",
            host,
        ],
        &[],
        None,
    );
    assert_command(
        &runner.commands[5],
        &[
            "gh",
            "api",
            "--hostname",
            host,
            "--method",
            "POST",
            "user/keys",
            "--raw-field",
            "title=Gas Can demo-0123456789ab",
            "--raw-field",
            &format!("key={PUBLIC_KEY}"),
        ],
        &github_environment(),
        None,
    );
    assert_command(
        &runner.commands[6],
        &[
            "gh",
            "api",
            "--hostname",
            host,
            "--method",
            "POST",
            "user/ssh_signing_keys",
            "--raw-field",
            "title=Gas Can demo-0123456789ab",
            "--raw-field",
            &format!("key={PUBLIC_KEY}"),
        ],
        &github_environment(),
        None,
    );
    assert_eq!(runner.interactive_commands.len(), 1);
    assert_eq!(runner.interactive_commands[0].0, selector());
    assert_eq!(
        runner.interactive_commands[0].1,
        argv(&["ssh", "-T", "git@github.enterprise.test"])
    );
    assert_command(
        &runner.commands[7],
        &["ssh", "-T", "git@github.enterprise.test"],
        &[],
        None,
    );
    assert_eq!(
        runner
            .commands
            .iter()
            .filter(|command| command.stdin.is_some())
            .count(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn github_registers_only_the_missing_signing_role_without_ssh_side_effects() -> TestResult {
    let authentication = github_key(SAME_KEY_OTHER_COMMENT);
    let created = github_key(PUBLIC_KEY);
    let mut runner = FakeGuestRunner::with_outputs([
        output(0, [], []),
        github_status("github.com", "octocat", GitProtocol::Https),
        github_keys(&[&authentication]),
        github_keys(&[]),
        output(0, created, []),
    ]);

    let setup = configure_forge(
        &mut runner,
        selector(),
        request(Forge::GitHub, "github.com", GitProtocol::Https),
    )
    .await?;
    assert_setup(
        &setup,
        Forge::GitHub,
        "github.com",
        "octocat",
        true,
        RegistrationState::Existing,
        RegistrationState::Added,
    );
    assert_eq!(runner.commands.len(), 5);
    assert_command(
        &runner.commands[4],
        &[
            "gh",
            "api",
            "--hostname",
            "github.com",
            "--method",
            "POST",
            "user/ssh_signing_keys",
            "--raw-field",
            "title=Gas Can demo-0123456789ab",
            "--raw-field",
            &format!("key={PUBLIC_KEY}"),
        ],
        &github_environment(),
        None,
    );
    assert!(runner.interactive_commands.is_empty());
    Ok(())
}

#[tokio::test]
async fn github_rejected_token_returns_redacted_structured_authentication_failure() -> TestResult {
    let mut runner =
        FakeGuestRunner::with_outputs([output(1, SENTINEL.as_bytes(), SENTINEL.as_bytes())]);
    let error = configure_forge(
        &mut runner,
        selector(),
        request(Forge::GitHub, "github.com", GitProtocol::Ssh),
    )
    .await
    .err()
    .ok_or("rejected GitHub token succeeded")?;
    assert_eq!(
        format!("{error}"),
        "GitHub authentication for github.com failed: [REDACTED]; retry with `gascan configure gh`"
    );
    let setup = forge_error(error, "github.com", "gascan configure gh")?;
    assert_setup(
        &setup,
        Forge::GitHub,
        "github.com",
        "",
        false,
        RegistrationState::Skipped,
        RegistrationState::Skipped,
    );
    assert_eq!(runner.commands.len(), 1);
    assert!(runner.interactive_commands.is_empty());
    Ok(())
}

#[tokio::test]
async fn github_missing_scope_and_malformed_json_preserve_independent_role_states() -> TestResult {
    let authentication = github_key(SAME_KEY_OTHER_COMMENT);
    let signing = github_key(SAME_KEY_OTHER_COMMENT);

    let mut missing_scope = FakeGuestRunner::with_outputs([
        output(0, [], []),
        github_status("github.com", "octocat", GitProtocol::Https),
        github_keys(&[&authentication]),
        output(403, SENTINEL.as_bytes(), SENTINEL.as_bytes()),
    ]);
    let error = configure_forge(
        &mut missing_scope,
        selector(),
        request(Forge::GitHub, "github.com", GitProtocol::Https),
    )
    .await
    .err()
    .ok_or("missing GitHub signing scope succeeded")?;
    let setup = forge_error(error, "github.com", "gascan configure gh")?;
    assert_setup(
        &setup,
        Forge::GitHub,
        "github.com",
        "octocat",
        true,
        RegistrationState::Existing,
        RegistrationState::Failed,
    );

    let malformed = format!(
        "[{{\"id\":17,\"key\":\"{PUBLIC_KEY}\",\"title\":\"x\",\"created_at\":\"2026-07-30T00:00:00Z\",\"private_key\":\"{SENTINEL}\"}}]"
    );
    let mut malformed_json = FakeGuestRunner::with_outputs([
        output(0, [], []),
        github_status("github.com", "octocat", GitProtocol::Https),
        output(0, malformed, []),
        github_keys(&[&signing]),
    ]);
    let error = configure_forge(
        &mut malformed_json,
        selector(),
        request(Forge::GitHub, "github.com", GitProtocol::Https),
    )
    .await
    .err()
    .ok_or("unknown GitHub key field succeeded")?;
    let setup = forge_error(error, "github.com", "gascan configure gh")?;
    assert_setup(
        &setup,
        Forge::GitHub,
        "github.com",
        "octocat",
        true,
        RegistrationState::Failed,
        RegistrationState::Existing,
    );
    Ok(())
}

#[tokio::test]
async fn github_partial_registration_retains_added_authentication_role() -> TestResult {
    let created = github_key(PUBLIC_KEY);
    let mut runner = FakeGuestRunner::with_steps(
        [
            Ok(output(0, [], [])),
            Ok(github_status("github.com", "octocat", GitProtocol::Ssh)),
            Ok(github_keys(&[])),
            Ok(github_keys(&[])),
            Ok(output(0, [], [])),
            Ok(output(0, created, [])),
            Ok(output(403, SENTINEL.as_bytes(), SENTINEL.as_bytes())),
            Ok(github_ssh_success("octocat")),
        ],
        [Ok(1)],
    );
    let error = configure_forge(
        &mut runner,
        selector(),
        request(Forge::GitHub, "github.com", GitProtocol::Ssh),
    )
    .await
    .err()
    .ok_or("partial GitHub registration reported complete success")?;
    assert_eq!(
        format!("{error}"),
        "GitHub key registration for github.com failed: one or more key registrations did not complete; retry with `gascan configure gh`"
    );
    let setup = forge_error(error, "github.com", "gascan configure gh")?;
    assert_setup(
        &setup,
        Forge::GitHub,
        "github.com",
        "octocat",
        true,
        RegistrationState::Added,
        RegistrationState::Failed,
    );
    assert_eq!(runner.interactive_commands.len(), 1);
    Ok(())
}

#[tokio::test]
async fn github_https_registers_both_roles_without_configuring_or_verifying_ssh() -> TestResult {
    let created = github_key(PUBLIC_KEY);
    let mut runner = FakeGuestRunner::with_outputs([
        output(0, [], []),
        github_status("github.com", "octocat", GitProtocol::Https),
        github_keys(&[]),
        github_keys(&[]),
        output(0, created.as_bytes(), []),
        output(0, created.as_bytes(), []),
    ]);
    let setup = configure_forge(
        &mut runner,
        selector(),
        request(Forge::GitHub, "github.com", GitProtocol::Https),
    )
    .await?;
    assert_setup(
        &setup,
        Forge::GitHub,
        "github.com",
        "octocat",
        true,
        RegistrationState::Added,
        RegistrationState::Added,
    );
    assert_eq!(runner.commands.len(), 6);
    assert!(!runner.commands.iter().any(|command| {
        command.argv.first().map(Vec::as_slice) == Some(b"/usr/local/bin/configure-developer-home")
            || command.argv.first().map(Vec::as_slice) == Some(b"ssh")
    }));
    assert!(runner.interactive_commands.is_empty());
    Ok(())
}

#[tokio::test]
async fn github_ssh_verification_requires_documented_response_not_exit_code_alone() -> TestResult {
    struct Case {
        name: &'static str,
        interactive: i32,
        probe: Option<GuestOutput>,
        succeeds: bool,
    }
    let cases = [
        Case {
            name: "transport failure",
            interactive: 255,
            probe: None,
            succeeds: false,
        },
        Case {
            name: "misleading conventional exit",
            interactive: 1,
            probe: Some(output(1, [], [])),
            succeeds: false,
        },
        Case {
            name: "wrong forge response",
            interactive: 0,
            probe: Some(gitlab_ssh_success("octocat")),
            succeeds: false,
        },
        Case {
            name: "different authenticated account",
            interactive: 1,
            probe: Some(github_ssh_success("not-octocat")),
            succeeds: false,
        },
        Case {
            name: "documented GitHub response",
            interactive: 1,
            probe: Some(github_ssh_success("octocat")),
            succeeds: true,
        },
    ];

    for case in cases {
        let has_probe = case.probe.is_some();
        let signing = github_key(SAME_KEY_OTHER_COMMENT);
        let created = github_key(PUBLIC_KEY);
        let mut outputs = vec![
            Ok(output(0, [], [])),
            Ok(github_status("github.com", "octocat", GitProtocol::Ssh)),
            Ok(github_keys(&[])),
            Ok(github_keys(&[&signing])),
            Ok(output(0, [], [])),
            Ok(output(0, created, [])),
        ];
        if let Some(probe) = case.probe {
            outputs.push(Ok(probe));
        }
        let mut runner = FakeGuestRunner::with_steps(outputs, [Ok(case.interactive)]);
        let result = configure_forge(
            &mut runner,
            selector(),
            request(Forge::GitHub, "github.com", GitProtocol::Ssh),
        )
        .await;
        if case.succeeds {
            let setup = result.map_err(|error| format!("{} failed: {error}", case.name))?;
            assert_eq!(setup.authentication_key, RegistrationState::Added);
        } else {
            let error = result
                .err()
                .ok_or_else(|| format!("{} unexpectedly succeeded", case.name))?;
            let setup = forge_error(error, "github.com", "gascan configure gh")?;
            assert_eq!(setup.authentication_key, RegistrationState::Added);
        }
        assert_eq!(
            runner
                .commands
                .iter()
                .filter(|command| command.argv.first().map(Vec::as_slice) == Some(b"ssh"))
                .count(),
            usize::from(has_probe)
        );
    }
    Ok(())
}

#[tokio::test]
async fn gitlab_existing_auth_and_signing_key_compares_body_not_title() -> TestResult {
    let existing = gitlab_key(SAME_KEY_OTHER_COMMENT, "auth_and_signing");
    let mut runner = FakeGuestRunner::with_outputs([
        output(0, [], []),
        gitlab_status("gitlab.com", "tanuki", GitProtocol::Https),
        gitlab_keys(&[&existing]),
    ]);
    let setup = configure_forge(
        &mut runner,
        selector(),
        request(Forge::GitLab, "gitlab.com", GitProtocol::Https),
    )
    .await?;
    assert_setup(
        &setup,
        Forge::GitLab,
        "gitlab.com",
        "tanuki",
        true,
        RegistrationState::Existing,
        RegistrationState::Existing,
    );
    assert_eq!(runner.commands.len(), 3);
    assert_command(
        &runner.commands[0],
        &[
            "glab",
            "auth",
            "login",
            "--hostname",
            "gitlab.com",
            "--git-protocol",
            "https",
            "--stdin",
        ],
        &gitlab_environment(),
        Some(SENTINEL.as_bytes()),
    );
    assert_command(
        &runner.commands[1],
        &["glab", "auth", "status", "--hostname", "gitlab.com"],
        &gitlab_environment(),
        None,
    );
    assert_command(
        &runner.commands[2],
        &["glab", "api", "--hostname", "gitlab.com", "/user/keys"],
        &gitlab_environment(),
        None,
    );
    Ok(())
}

#[tokio::test]
async fn gitlab_signing_only_collision_is_partial_without_duplicate_registration() -> TestResult {
    let collision = gitlab_key(SAME_KEY_OTHER_COMMENT, "signing");
    let mut runner = FakeGuestRunner::with_outputs([
        output(0, [], []),
        gitlab_status("gitlab.com", "tanuki", GitProtocol::Ssh),
        gitlab_keys(&[&collision]),
    ]);
    let error = configure_forge(
        &mut runner,
        selector(),
        request(Forge::GitLab, "gitlab.com", GitProtocol::Ssh),
    )
    .await
    .err()
    .ok_or("GitLab signing-only collision succeeded")?;
    let setup = forge_error(error, "gitlab.com", "gascan configure glab")?;
    assert_setup(
        &setup,
        Forge::GitLab,
        "gitlab.com",
        "tanuki",
        true,
        RegistrationState::Failed,
        RegistrationState::Existing,
    );
    assert_eq!(runner.commands.len(), 3);
    assert!(runner.interactive_commands.is_empty());
    Ok(())
}

#[tokio::test]
async fn gitlab_self_managed_registration_uses_auth_and_signing_and_two_step_ssh() -> TestResult {
    let host = "gitlab.self-managed.test";
    let created = gitlab_key(PUBLIC_KEY, "auth_and_signing");
    let mut runner = FakeGuestRunner::with_steps(
        [
            Ok(output(0, [], [])),
            Ok(gitlab_status(host, "tanuki", GitProtocol::Ssh)),
            Ok(gitlab_keys(&[])),
            Ok(output(0, [], [])),
            Ok(output(0, created, [])),
            Ok(gitlab_ssh_success("tanuki")),
        ],
        [Ok(0)],
    );
    let setup = configure_forge(
        &mut runner,
        selector(),
        request(Forge::GitLab, host, GitProtocol::Ssh),
    )
    .await?;
    assert_setup(
        &setup,
        Forge::GitLab,
        host,
        "tanuki",
        true,
        RegistrationState::Added,
        RegistrationState::Added,
    );
    assert_eq!(runner.commands.len(), 6);
    assert_command(
        &runner.commands[4],
        &[
            "glab",
            "api",
            "--hostname",
            host,
            "--method",
            "POST",
            "/user/keys",
            "--raw-field",
            "title=Gas Can demo-0123456789ab",
            "--raw-field",
            &format!("key={PUBLIC_KEY}"),
            "--raw-field",
            "usage_type=auth_and_signing",
        ],
        &gitlab_environment(),
        None,
    );
    assert_eq!(
        runner.interactive_commands[0].1,
        argv(&["ssh", "-T", "git@gitlab.self-managed.test"])
    );
    assert_command(
        &runner.commands[5],
        &["ssh", "-T", "git@gitlab.self-managed.test"],
        &[],
        None,
    );
    Ok(())
}

#[tokio::test]
async fn gitlab_registration_failure_retains_authentication_and_redacts_native_output() -> TestResult
{
    let mut runner = FakeGuestRunner::with_outputs([
        output(0, [], []),
        gitlab_status("gitlab.com", "tanuki", GitProtocol::Ssh),
        gitlab_keys(&[]),
        output(0, [], []),
        output(403, SENTINEL.as_bytes(), SENTINEL.as_bytes()),
    ]);
    let error = configure_forge(
        &mut runner,
        selector(),
        request(Forge::GitLab, "gitlab.com", GitProtocol::Ssh),
    )
    .await
    .err()
    .ok_or("GitLab registration failure reported success")?;
    let setup = forge_error(error, "gitlab.com", "gascan configure glab")?;
    assert_setup(
        &setup,
        Forge::GitLab,
        "gitlab.com",
        "tanuki",
        true,
        RegistrationState::Failed,
        RegistrationState::Failed,
    );
    assert!(runner.interactive_commands.is_empty());
    Ok(())
}

#[tokio::test]
async fn gitlab_ssh_verification_rejects_wrong_forge_response_with_registered_state_retained()
-> TestResult {
    let created = gitlab_key(PUBLIC_KEY, "auth_and_signing");
    let mut runner = FakeGuestRunner::with_steps(
        [
            Ok(output(0, [], [])),
            Ok(gitlab_status("gitlab.com", "tanuki", GitProtocol::Ssh)),
            Ok(gitlab_keys(&[])),
            Ok(output(0, [], [])),
            Ok(output(0, created, [])),
            Ok(output(
                0,
                "Hi tanuki! You've successfully authenticated, but GitHub does not provide shell access.\n",
                [],
            )),
        ],
        [Ok(1)],
    );
    let error = configure_forge(
        &mut runner,
        selector(),
        request(Forge::GitLab, "gitlab.com", GitProtocol::Ssh),
    )
    .await
    .err()
    .ok_or("wrong-forge GitLab SSH response succeeded")?;
    let setup = forge_error(error, "gitlab.com", "gascan configure glab")?;
    assert_setup(
        &setup,
        Forge::GitLab,
        "gitlab.com",
        "tanuki",
        true,
        RegistrationState::Added,
        RegistrationState::Added,
    );
    Ok(())
}

#[tokio::test]
async fn gitlab_ssh_verification_rejects_a_different_authenticated_account() -> TestResult {
    let created = gitlab_key(PUBLIC_KEY, "auth_and_signing");
    let mut runner = FakeGuestRunner::with_steps(
        [
            Ok(output(0, [], [])),
            Ok(gitlab_status("gitlab.com", "tanuki", GitProtocol::Ssh)),
            Ok(gitlab_keys(&[])),
            Ok(output(0, [], [])),
            Ok(output(0, created, [])),
            Ok(gitlab_ssh_success("different-user")),
        ],
        [Ok(0)],
    );
    let error = configure_forge(
        &mut runner,
        selector(),
        request(Forge::GitLab, "gitlab.com", GitProtocol::Ssh),
    )
    .await
    .err()
    .ok_or("GitLab SSH response for another account succeeded")?;
    let setup = forge_error(error, "gitlab.com", "gascan configure glab")?;
    assert_setup(
        &setup,
        Forge::GitLab,
        "gitlab.com",
        "tanuki",
        true,
        RegistrationState::Added,
        RegistrationState::Added,
    );
    Ok(())
}

#[tokio::test]
async fn forge_rejects_unbounded_status_and_key_json_with_stable_errors() -> TestResult {
    let mut status = FakeGuestRunner::with_outputs([
        output(0, [], []),
        output(0, vec![b'x'; 64 * 1024 + 1], []),
    ]);
    let error = configure_forge(
        &mut status,
        selector(),
        request(Forge::GitHub, "github.com", GitProtocol::Https),
    )
    .await
    .err()
    .ok_or("oversized auth status succeeded")?;
    let setup = forge_error(error, "github.com", "gascan configure gh")?;
    assert!(!setup.authenticated);

    let mut keys = FakeGuestRunner::with_outputs([
        output(0, [], []),
        gitlab_status("gitlab.com", "tanuki", GitProtocol::Https),
        output(0, vec![b'['; 64 * 1024 + 1], []),
    ]);
    let error = configure_forge(
        &mut keys,
        selector(),
        request(Forge::GitLab, "gitlab.com", GitProtocol::Https),
    )
    .await
    .err()
    .ok_or("oversized key list succeeded")?;
    let setup = forge_error(error, "gitlab.com", "gascan configure glab")?;
    assert_eq!(setup.authentication_key, RegistrationState::Failed);
    assert_eq!(setup.signing_key, RegistrationState::Failed);
    Ok(())
}

#[tokio::test]
async fn forge_rejects_unvalidated_hostname_without_guest_mutation_or_error_echo() -> TestResult {
    let hostname = format!("bad host\n{SENTINEL}");
    let mut runner = FakeGuestRunner::default();
    let error = configure_forge(
        &mut runner,
        selector(),
        request(Forge::GitHub, &hostname, GitProtocol::Ssh),
    )
    .await
    .err()
    .ok_or("invalid forge hostname succeeded")?;
    assert!(matches!(error, ConfigureError::InvalidOutput { .. }));
    assert!(!format!("{error}").contains(SENTINEL));
    assert!(!format!("{error:?}").contains(SENTINEL));
    assert!(runner.commands.is_empty());
    assert!(runner.interactive_commands.is_empty());
    Ok(())
}

#[tokio::test]
async fn forge_rejects_algorithm_body_mismatched_key_before_consuming_token() -> TestResult {
    let mut request = request(Forge::GitHub, "github.com", GitProtocol::Ssh);
    request.key.public_key = PUBLIC_KEY.replacen("ssh-ed25519", "ssh-rsa", 1);
    let mut runner = FakeGuestRunner::default();
    let error = configure_forge(&mut runner, selector(), request)
        .await
        .err()
        .ok_or("algorithm/body-mismatched forge key succeeded")?;
    assert!(!format!("{error}").contains(SENTINEL));
    assert!(!format!("{error:?}").contains(SENTINEL));
    assert!(runner.commands.is_empty());
    assert!(runner.interactive_commands.is_empty());
    Ok(())
}
