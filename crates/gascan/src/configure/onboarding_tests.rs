use super::onboarding::offer_after_up_with;
use super::{
    ConfigureError, ConfigureIo, ConfigureOutcome, Forge, GitDefaults, GitProtocol, HostAccount,
    HostDiscovery, OfferResult, Prompter, configure_all, configure_forge_interactive,
    configure_git_interactive,
};
use crate::cli::CliError;
use crate::guest::{GuestCommand, GuestOutput, GuestRunner, Secret};
use gascan_proto::v1;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const SANDBOX_ID: &str = "demo-0123456789ab";
const SENTINEL: &str = "task7-coordinator-token-never-print-f02d31";
const PUBLIC_KEY: &str = concat!(
    "ssh-ed25519 ",
    "AAAAC3NzaC1lZDI1NTE5AAAAIAEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEB",
    " gascan-demo-0123456789ab"
);
const FINGERPRINT: &str = "SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

#[derive(Debug)]
struct RecordedCommand {
    argv: Vec<Vec<u8>>,
    environment: Vec<v1::EnvironmentVariable>,
    stdin: Option<Vec<u8>>,
}

#[derive(Default)]
struct FakeRunner {
    outputs: VecDeque<Result<GuestOutput, CliError>>,
    interactive: VecDeque<Result<i32, CliError>>,
    commands: Vec<RecordedCommand>,
    interactive_commands: Vec<Vec<Vec<u8>>>,
    events: Option<Arc<Mutex<Vec<String>>>>,
}

impl FakeRunner {
    fn with_outputs(outputs: impl IntoIterator<Item = GuestOutput>) -> Self {
        Self {
            outputs: outputs.into_iter().map(Ok).collect(),
            ..Self::default()
        }
    }

    fn record_events(&mut self, events: Arc<Mutex<Vec<String>>>) {
        self.events = Some(events);
    }
}

#[tonic::async_trait]
impl GuestRunner for FakeRunner {
    async fn execute(
        &mut self,
        selector: v1::SandboxSelector,
        command: GuestCommand,
    ) -> Result<GuestOutput, CliError> {
        assert_eq!(selector.sandbox_id, SANDBOX_ID);
        if let Some(events) = &self.events {
            let command_name = command
                .argv
                .iter()
                .take(3)
                .map(|argument| String::from_utf8_lossy(argument))
                .collect::<Vec<_>>()
                .join(" ");
            if let Ok(mut events) = events.lock() {
                events.push(format!("guest:{command_name}"));
            }
        }
        self.commands.push(RecordedCommand {
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
        assert_eq!(selector.sandbox_id, SANDBOX_ID);
        self.interactive_commands.push(argv);
        self.interactive.pop_front().unwrap_or_else(|| {
            Err(CliError::Runtime(
                "unexpected interactive command".to_owned(),
            ))
        })
    }
}

struct FakeDiscovery {
    defaults: GitDefaults,
    accounts: Vec<(Forge, Vec<HostAccount>)>,
    failing_tokens: Vec<String>,
    events: Arc<Mutex<Vec<String>>>,
    account_calls: RefCell<Vec<Forge>>,
}

impl FakeDiscovery {
    fn new(defaults: GitDefaults, events: Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            defaults,
            accounts: Vec::new(),
            failing_tokens: Vec::new(),
            events,
            account_calls: RefCell::new(Vec::new()),
        }
    }
}

impl HostDiscovery for FakeDiscovery {
    fn git_defaults(&self) -> Result<GitDefaults, ConfigureError> {
        Ok(GitDefaults {
            name: self.defaults.name.clone(),
            email: self.defaults.email.clone(),
        })
    }

    fn accounts(&self, forge: Forge) -> Result<Vec<HostAccount>, ConfigureError> {
        self.events
            .lock()
            .map_err(|_| ConfigureError::InvalidOutput {
                category: "test event log",
            })?
            .push(format!("accounts:{forge:?}"));
        self.account_calls.borrow_mut().push(forge);
        Ok(self
            .accounts
            .iter()
            .find_map(|(candidate, accounts)| (*candidate == forge).then(|| accounts.clone()))
            .unwrap_or_default())
    }

    fn token(&self, _forge: Forge, account: &HostAccount) -> Result<Secret, ConfigureError> {
        self.events
            .lock()
            .map_err(|_| ConfigureError::InvalidOutput {
                category: "test event log",
            })?
            .push(format!("token:{}", account.hostname));
        if self.failing_tokens.contains(&account.hostname) {
            return Err(ConfigureError::HostCommand {
                category: "test token retrieval",
                message: "command did not complete successfully".to_owned(),
            });
        }
        Ok(Secret::new(SENTINEL.as_bytes().to_vec()))
    }
}

struct FakeIo {
    confirms: VecDeque<Result<bool, ConfigureError>>,
    confirm_defaults: Vec<bool>,
    lines: VecDeque<Result<Option<String>, ConfigureError>>,
    secrets: VecDeque<Result<Option<Vec<u8>>, ConfigureError>>,
    stdout: String,
    stderr: String,
    events: Arc<Mutex<Vec<String>>>,
    stdin_terminal: bool,
    stderr_terminal: bool,
}

impl FakeIo {
    fn interactive(events: Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            confirms: VecDeque::new(),
            confirm_defaults: Vec::new(),
            lines: VecDeque::new(),
            secrets: VecDeque::new(),
            stdout: String::new(),
            stderr: String::new(),
            events,
            stdin_terminal: true,
            stderr_terminal: true,
        }
    }

    fn push_confirm(&mut self, value: bool) {
        self.confirms.push_back(Ok(value));
    }

    fn push_line(&mut self, value: &str) {
        self.lines.push_back(Ok(Some(value.to_owned())));
    }

    fn push_secret(&mut self) {
        self.secrets
            .push_back(Ok(Some(SENTINEL.as_bytes().to_vec())));
    }

    fn event(&self, value: String) -> Result<(), ConfigureError> {
        self.events
            .lock()
            .map_err(|_| ConfigureError::InvalidOutput {
                category: "test event log",
            })?
            .push(value);
        Ok(())
    }
}

impl Prompter for FakeIo {
    fn confirm(&mut self, prompt: &str, default: bool) -> Result<bool, ConfigureError> {
        self.confirm_defaults.push(default);
        self.event(format!("confirm:{prompt}"))?;
        self.confirms
            .pop_front()
            .unwrap_or(Err(ConfigureError::InvalidOutput {
                category: "test confirm",
            }))
    }

    fn line(
        &mut self,
        prompt: &str,
        _default: Option<&str>,
    ) -> Result<Option<String>, ConfigureError> {
        self.event(format!("line:{prompt}"))?;
        self.lines
            .pop_front()
            .unwrap_or(Err(ConfigureError::InvalidOutput {
                category: "test line",
            }))
    }

    fn secret(&mut self, prompt: &str) -> Result<Option<Secret>, ConfigureError> {
        self.event(format!("secret:{prompt}"))?;
        self.secrets
            .pop_front()
            .unwrap_or(Err(ConfigureError::InvalidOutput {
                category: "test secret",
            }))
            .map(|secret| secret.map(Secret::new))
    }
}

impl ConfigureIo for FakeIo {
    fn write_out(&mut self, text: &str) -> Result<(), ConfigureError> {
        self.stdout.push_str(text);
        self.event(format!("out:{text}"))
    }

    fn write_err(&mut self, text: &str) -> Result<(), ConfigureError> {
        self.stderr.push_str(text);
        self.event(format!("err:{text}"))
    }

    fn stdin_is_terminal(&self) -> bool {
        self.stdin_terminal
    }

    fn stderr_is_terminal(&self) -> bool {
        self.stderr_terminal
    }
}

fn selector() -> v1::SandboxSelector {
    v1::SandboxSelector {
        sandbox_id: SANDBOX_ID.to_owned(),
    }
}

fn output(code: i32, stdout: impl Into<Vec<u8>>, stderr: impl Into<Vec<u8>>) -> GuestOutput {
    GuestOutput {
        code,
        stdout: stdout.into(),
        stderr: stderr.into(),
    }
}

fn empty_status() -> GuestOutput {
    output(
        0,
        "{\"name\":null,\"email\":null,\"protocol\":null,\"public_key\":null,\"fingerprint\":null,\"receipt\":\"pending\"}\n",
        [],
    )
}

fn receipt_status(state: &str) -> GuestOutput {
    output(0, format!("{state}\n"), [])
}

fn configured_status(protocol: GitProtocol) -> GuestOutput {
    let protocol = match protocol {
        GitProtocol::Ssh => "ssh",
        GitProtocol::Https => "https",
    };
    output(
        0,
        format!(
            "{{\"name\":\"Ada Lovelace\",\"email\":\"ada@example.test\",\"protocol\":\"{protocol}\",\"public_key\":\"{PUBLIC_KEY}\",\"fingerprint\":\"{FINGERPRINT}\",\"receipt\":\"pending\"}}\n"
        ),
        [],
    )
}

fn online_route() -> GuestOutput {
    output(0, "default via 192.0.2.1 dev eth0\n", [])
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

fn github_key() -> String {
    format!(
        "{{\"id\":17,\"key\":\"{PUBLIC_KEY}\",\"title\":\"existing\",\"verified\":true,\"created_at\":\"2026-07-30T00:00:00Z\",\"read_only\":false}}"
    )
}

fn github_keys(existing: bool) -> GuestOutput {
    let key = existing.then(github_key).unwrap_or_default();
    output(0, format!("[{key}]"), [])
}

fn gitlab_keys(existing: bool) -> GuestOutput {
    let key = if existing {
        format!(
            "{{\"id\":23,\"title\":\"existing\",\"key\":\"{PUBLIC_KEY}\",\"created_at\":\"2026-07-30T00:00:00Z\",\"expires_at\":null,\"last_used_at\":null,\"usage_type\":\"auth_and_signing\"}}"
        )
    } else {
        String::new()
    };
    output(0, format!("[{key}]"), [])
}

fn successful_github_setup_outputs(
    hostname: &str,
    login: &str,
    protocol: GitProtocol,
) -> Vec<GuestOutput> {
    vec![
        output(0, [], []),
        github_status(hostname, login, protocol),
        github_keys(true),
        github_keys(true),
    ]
}

fn argv(command: &RecordedCommand) -> Vec<String> {
    command
        .argv
        .iter()
        .map(|argument| String::from_utf8_lossy(argument).into_owned())
        .collect()
}

#[tokio::test]
async fn first_up_pending_receipt_decline_is_recorded_without_setup_values() -> TestResult {
    let events = Arc::new(Mutex::new(Vec::new()));
    let discovery = FakeDiscovery::new(
        GitDefaults {
            name: None,
            email: None,
        },
        Arc::clone(&events),
    );
    let mut io = FakeIo::interactive(events);
    io.push_confirm(false);
    let mut runner = FakeRunner::with_outputs([receipt_status("pending"), output(0, [], [])]);

    let result = offer_after_up_with(&mut runner, selector(), &discovery, &mut io).await?;

    assert_eq!(result, OfferResult::Declined);
    assert_eq!(runner.commands.len(), 2);
    assert_eq!(
        argv(&runner.commands[0]),
        [
            "/usr/local/bin/configure-developer-home",
            "receipt",
            "status",
        ]
    );
    assert_eq!(
        argv(&runner.commands[1]),
        [
            "/usr/local/bin/configure-developer-home",
            "receipt",
            "decline",
        ]
    );
    assert!(runner.commands.iter().all(|command| {
        command.environment.is_empty() && command.stdin.is_none() && argv(command).len() == 3
    }));
    assert_eq!(
        io.stderr,
        "Run 'gascan configure' whenever you are ready.\n"
    );
    let recorded = io
        .events
        .lock()
        .map_err(|_| "test event log was poisoned")?;
    assert_eq!(
        recorded
            .iter()
            .filter(|event| {
                event.as_str()
                    == "confirm:Set up Git, GitHub, and GitLab for this sandbox now? [Y/n] "
            })
            .count(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn first_up_complete_and_declined_receipts_suppress_the_prompt() -> TestResult {
    for (state, expected) in [
        ("complete", OfferResult::Completed),
        ("declined", OfferResult::Declined),
    ] {
        let events = Arc::new(Mutex::new(Vec::new()));
        let discovery = FakeDiscovery::new(
            GitDefaults {
                name: None,
                email: None,
            },
            Arc::clone(&events),
        );
        let mut io = FakeIo::interactive(events);
        let mut runner = FakeRunner::with_outputs([receipt_status(state)]);

        let result = offer_after_up_with(&mut runner, selector(), &discovery, &mut io).await?;

        assert_eq!(result, expected);
        assert_eq!(runner.commands.len(), 1);
        assert!(io.stdout.is_empty());
        assert!(io.stderr.is_empty());
        assert!(
            io.events
                .lock()
                .map_err(|_| "test event log was poisoned")?
                .iter()
                .all(|event| !event.starts_with("confirm:"))
        );
    }
    Ok(())
}

#[tokio::test]
async fn first_up_cancelled_prompt_keeps_the_receipt_pending() -> TestResult {
    let events = Arc::new(Mutex::new(Vec::new()));
    let discovery = FakeDiscovery::new(
        GitDefaults {
            name: None,
            email: None,
        },
        Arc::clone(&events),
    );
    let mut io = FakeIo::interactive(events);
    io.confirms.push_back(Err(ConfigureError::Cancelled));
    let mut runner = FakeRunner::with_outputs([receipt_status("pending")]);

    let result = offer_after_up_with(&mut runner, selector(), &discovery, &mut io).await?;

    assert_eq!(result, OfferResult::Cancelled);
    assert_eq!(runner.commands.len(), 1);
    assert!(io.stdout.is_empty());
    assert!(io.stderr.is_empty());
    Ok(())
}

#[tokio::test]
async fn first_up_accepted_guide_completes_the_receipt_once() -> TestResult {
    let events = Arc::new(Mutex::new(Vec::new()));
    let discovery = FakeDiscovery::new(
        GitDefaults {
            name: None,
            email: None,
        },
        Arc::clone(&events),
    );
    let mut io = FakeIo::interactive(events);
    for answer in [true, true, false, false] {
        io.push_confirm(answer);
    }
    let mut runner = FakeRunner::with_outputs([
        receipt_status("pending"),
        configured_status(GitProtocol::Ssh),
        online_route(),
        output(0, [], []),
    ]);

    let result = offer_after_up_with(&mut runner, selector(), &discovery, &mut io).await?;

    assert_eq!(result, OfferResult::Completed);
    assert_eq!(
        runner
            .commands
            .iter()
            .filter(|command| {
                argv(command).ends_with(&["receipt".to_owned(), "complete".to_owned()])
            })
            .count(),
        1
    );
    assert_eq!(
        io.events
            .lock()
            .map_err(|_| "test event log was poisoned")?
            .iter()
            .filter(|event| {
                event.as_str()
                    == "confirm:Set up Git, GitHub, and GitLab for this sandbox now? [Y/n] "
            })
            .count(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn first_up_partial_guide_leaves_the_receipt_pending() -> TestResult {
    let events = Arc::new(Mutex::new(Vec::new()));
    let discovery = FakeDiscovery::new(
        GitDefaults {
            name: None,
            email: None,
        },
        Arc::clone(&events),
    );
    let mut io = FakeIo::interactive(events);
    io.push_confirm(true);
    for value in ["Ada Lovelace", "ada@example.test", "https"] {
        io.push_line(value);
    }
    let mut runner = FakeRunner::with_outputs([
        receipt_status("pending"),
        empty_status(),
        output(0, [], []),
        configured_status(GitProtocol::Https),
        output(0, [], []),
    ]);

    let result = offer_after_up_with(&mut runner, selector(), &discovery, &mut io).await?;

    assert_eq!(result, OfferResult::Pending);
    assert!(runner
        .commands
        .iter()
        .all(|command| !argv(command).ends_with(&["receipt".to_owned(), "complete".to_owned()])));
    Ok(())
}

#[tokio::test]
async fn first_up_non_tty_is_suppressed_before_receipt_access() -> TestResult {
    for redirected in ["stdin", "stderr"] {
        let events = Arc::new(Mutex::new(Vec::new()));
        let discovery = FakeDiscovery::new(
            GitDefaults {
                name: None,
                email: None,
            },
            Arc::clone(&events),
        );
        let mut io = FakeIo::interactive(events);
        if redirected == "stdin" {
            io.stdin_terminal = false;
        } else {
            io.stderr_terminal = false;
        }
        let mut runner = FakeRunner::default();

        let result = offer_after_up_with(&mut runner, selector(), &discovery, &mut io).await?;

        assert_eq!(result, OfferResult::Suppressed);
        assert!(runner.commands.is_empty());
        assert!(io.stdout.is_empty());
        assert!(io.stderr.is_empty());
    }
    Ok(())
}

#[tokio::test]
async fn first_up_receipt_status_and_decline_write_errors_are_returned() -> TestResult {
    for outputs in [
        vec![output(1, [], [])],
        vec![receipt_status("pending"), output(1, [], [])],
    ] {
        let events = Arc::new(Mutex::new(Vec::new()));
        let discovery = FakeDiscovery::new(
            GitDefaults {
                name: None,
                email: None,
            },
            Arc::clone(&events),
        );
        let mut io = FakeIo::interactive(events);
        io.push_confirm(false);
        let mut runner = FakeRunner::with_outputs(outputs);

        let error = match offer_after_up_with(&mut runner, selector(), &discovery, &mut io).await {
            Err(error) => error,
            Ok(_) => return Err("receipt failure unexpectedly completed the offer".into()),
        };

        assert!(error.to_string().contains("developer-home receipt"));
        assert!(!io.stderr.contains(SENTINEL));
    }
    Ok(())
}

#[tokio::test]
async fn first_up_setup_error_is_returned_without_writing_a_receipt() -> TestResult {
    let events = Arc::new(Mutex::new(Vec::new()));
    let discovery = FakeDiscovery::new(
        GitDefaults {
            name: None,
            email: None,
        },
        Arc::clone(&events),
    );
    let mut io = FakeIo::interactive(events);
    io.push_confirm(true);
    let mut runner = FakeRunner::with_outputs([
        receipt_status("pending"),
        output(1, SENTINEL.as_bytes(), SENTINEL.as_bytes()),
    ]);

    let error = match offer_after_up_with(&mut runner, selector(), &discovery, &mut io).await {
        Err(error) => error,
        Ok(_) => return Err("setup failure unexpectedly completed the offer".into()),
    };

    assert!(error.to_string().contains("developer-home status"));
    assert!(runner
        .commands
        .iter()
        .all(|command| !argv(command).ends_with(&["receipt".to_owned(), "complete".to_owned()])));
    assert!(!error.to_string().contains(SENTINEL));
    assert!(!io.stdout.contains(SENTINEL));
    assert!(!io.stderr.contains(SENTINEL));
    Ok(())
}

#[tokio::test]
async fn aggregate_accepts_and_edits_host_defaults_with_ssh_default_and_explicit_skips()
-> TestResult {
    let events = Arc::new(Mutex::new(Vec::new()));
    let discovery = FakeDiscovery::new(
        GitDefaults {
            name: Some("Ada Lovelace".to_owned()),
            email: Some("ada@old.test".to_owned()),
        },
        Arc::clone(&events),
    );
    let mut io = FakeIo::interactive(events);
    io.push_confirm(false);
    io.push_line("Ada Lovelace");
    io.push_line("ada@example.test");
    io.push_line("ssh");
    io.push_confirm(false);
    io.push_confirm(false);
    let mut runner = FakeRunner::with_outputs([
        empty_status(),
        output(0, [], []),
        configured_status(GitProtocol::Ssh),
        online_route(),
        output(0, [], []),
    ]);

    let outcome = configure_all(&mut runner, selector(), &discovery, &mut io).await?;

    assert_eq!(outcome, ConfigureOutcome::Completed);
    assert_eq!(runner.commands.len(), 5);
    assert_eq!(
        argv(&runner.commands[1]),
        [
            "/usr/local/bin/configure-developer-home",
            "git",
            "--sandbox-id",
            SANDBOX_ID,
            "--name",
            "Ada Lovelace",
            "--email",
            "ada@example.test",
            "--protocol",
            "ssh",
        ]
    );
    assert_eq!(
        argv(&runner.commands[3]),
        ["ip", "route", "show", "default"]
    );
    assert_eq!(
        argv(&runner.commands[4]),
        [
            "/usr/local/bin/configure-developer-home",
            "receipt",
            "complete",
        ]
    );
    assert!(
        runner
            .commands
            .iter()
            .all(|command| command.environment.is_empty())
    );
    assert!(io.stdout.contains("Ada Lovelace"));
    assert!(io.stderr.contains("ada@old.test"));
    assert!(io.stdout.contains(FINGERPRINT));
    assert!(io.stderr.contains("GitHub: skipped"));
    assert!(io.stderr.contains("GitLab: skipped"));
    assert_eq!(
        io.stdout,
        format!("Git: Ada Lovelace <ada@example.test>; protocol ssh; fingerprint {FINGERPRINT}\n")
    );
    assert!(
        io.stderr
            .contains("Summary\nGitHub: skipped\nGitLab: skipped\n")
    );
    assert!(io.stderr.lines().any(|line| line == "Git"));
    assert!(io.stderr.lines().any(|line| line == "GitHub"));
    assert!(io.stderr.lines().any(|line| line == "GitLab"));
    assert!(!io.stdout.contains(PUBLIC_KEY));
    assert!(!io.stdout.contains(SENTINEL));
    assert!(!io.stderr.contains(SENTINEL));
    assert!(!io.stdout.contains("\x1b["));
    assert!(!io.stderr.contains("\x1b["));
    Ok(())
}

#[tokio::test]
async fn cancelling_unconfigured_git_leaves_dependent_sections_without_receipt() -> TestResult {
    let events = Arc::new(Mutex::new(Vec::new()));
    let discovery = FakeDiscovery::new(
        GitDefaults {
            name: None,
            email: None,
        },
        Arc::clone(&events),
    );
    let mut io = FakeIo::interactive(events);
    io.lines.push_back(Err(ConfigureError::Cancelled));
    let mut runner = FakeRunner::with_outputs([empty_status()]);

    let outcome = configure_all(&mut runner, selector(), &discovery, &mut io).await?;

    assert_eq!(outcome, ConfigureOutcome::Cancelled);
    assert_eq!(runner.commands.len(), 1);
    assert!(io.stdout.contains("Configuration cancelled"));
    assert!(
        runner
            .commands
            .iter()
            .all(|command| !argv(command).contains(&"receipt".to_owned()))
    );
    Ok(())
}

#[tokio::test]
async fn aggregate_hidden_entry_prompts_for_an_enterprise_hostname() -> TestResult {
    let events = Arc::new(Mutex::new(Vec::new()));
    let discovery = FakeDiscovery::new(
        GitDefaults {
            name: None,
            email: None,
        },
        Arc::clone(&events),
    );
    let mut io = FakeIo::interactive(events);
    for answer in [true, true, false] {
        io.push_confirm(answer);
    }
    io.push_line("github.enterprise.test");
    io.push_secret();
    let mut runner = FakeRunner::with_outputs([
        configured_status(GitProtocol::Https),
        online_route(),
        output(0, [], []),
        github_status(
            "github.enterprise.test",
            "enterprise-user",
            GitProtocol::Https,
        ),
        github_keys(true),
        github_keys(true),
        output(0, [], []),
    ]);

    let outcome = configure_all(&mut runner, selector(), &discovery, &mut io).await?;

    assert_eq!(outcome, ConfigureOutcome::Completed);
    assert!(
        argv(&runner.commands[2]).contains(&"github.enterprise.test".to_owned()),
        "manual hostname was not forwarded: {:?}",
        argv(&runner.commands[2])
    );
    assert_eq!(
        runner
            .commands
            .iter()
            .filter(|command| command.stdin.as_deref() == Some(SENTINEL.as_bytes()))
            .count(),
        1
    );
    assert!(io.stdout.contains("github.enterprise.test"));
    assert!(!io.stdout.contains(SENTINEL));
    assert!(!io.stderr.contains(SENTINEL));
    Ok(())
}

#[tokio::test]
async fn route_probe_failure_reports_retained_git_and_focused_retries_without_receipt() -> TestResult
{
    let events = Arc::new(Mutex::new(Vec::new()));
    let discovery = FakeDiscovery::new(
        GitDefaults {
            name: None,
            email: None,
        },
        Arc::clone(&events),
    );
    let mut io = FakeIo::interactive(events);
    io.push_confirm(true);
    let mut runner = FakeRunner::with_outputs([configured_status(GitProtocol::Https)]);
    runner.outputs.push_back(Err(CliError::Runtime(
        "injected route probe transport failure".to_owned(),
    )));

    let outcome = configure_all(&mut runner, selector(), &discovery, &mut io).await?;

    assert_eq!(outcome, ConfigureOutcome::Partial);
    assert!(io.stdout.contains("Git: Ada Lovelace <ada@example.test>"));
    assert!(io.stderr.contains("Git setup was retained"));
    assert!(io.stderr.contains("gascan configure gh"));
    assert!(io.stderr.contains("gascan configure glab"));
    assert!(
        runner
            .commands
            .iter()
            .all(|command| !argv(command).contains(&"receipt".to_owned()))
    );
    Ok(())
}

#[tokio::test]
async fn aggregate_reuses_existing_https_setup_and_imports_selected_enterprise_accounts()
-> TestResult {
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut discovery = FakeDiscovery::new(
        GitDefaults {
            name: None,
            email: None,
        },
        Arc::clone(&events),
    );
    discovery.accounts = vec![
        (
            Forge::GitHub,
            vec![
                HostAccount {
                    hostname: "github.com".to_owned(),
                    login: Some("octocat".to_owned()),
                },
                HostAccount {
                    hostname: "github.enterprise.test".to_owned(),
                    login: Some("enterprise-user".to_owned()),
                },
            ],
        ),
        (
            Forge::GitLab,
            vec![HostAccount {
                hostname: "gitlab.self-managed.test".to_owned(),
                login: Some("tanuki".to_owned()),
            }],
        ),
    ];
    let mut io = FakeIo::interactive(Arc::clone(&events));
    for answer in [true, true] {
        io.push_confirm(answer);
    }
    io.push_line("2");
    let key = github_key();
    let mut runner = FakeRunner::with_outputs([
        configured_status(GitProtocol::Https),
        online_route(),
        output(0, [], []),
        github_status(
            "github.enterprise.test",
            "enterprise-user",
            GitProtocol::Https,
        ),
        output(0, format!("[{key}]"), []),
        output(0, format!("[{key}]"), []),
        output(0, [], []),
        gitlab_status("gitlab.self-managed.test", "tanuki", GitProtocol::Https),
        gitlab_keys(true),
        output(0, [], []),
    ]);

    let outcome = configure_all(&mut runner, selector(), &discovery, &mut io).await?;

    assert_eq!(outcome, ConfigureOutcome::Completed);
    assert_eq!(runner.commands.len(), 10);
    assert_eq!(
        argv(&runner.commands[1]),
        ["ip", "route", "show", "default"]
    );
    assert_eq!(
        runner
            .commands
            .iter()
            .filter(|command| command.stdin.as_deref() == Some(SENTINEL.as_bytes()))
            .count(),
        2
    );
    assert!(argv(&runner.commands[2]).contains(&"github.enterprise.test".to_owned()));
    assert!(argv(&runner.commands[6]).contains(&"gitlab.self-managed.test".to_owned()));
    let recorded = events.lock().map_err(|_| "test event log was poisoned")?;
    for hostname in ["github.enterprise.test", "gitlab.self-managed.test"] {
        let token = recorded
            .iter()
            .position(|event| event == &format!("token:{hostname}"))
            .ok_or("token retrieval was not recorded")?;
        assert!(
            recorded[..token]
                .iter()
                .any(|event| event.starts_with("accounts:")),
            "token was retrieved before account discovery: {recorded:?}"
        );
    }
    assert!(io.stdout.contains("enterprise-user"));
    assert!(io.stdout.contains("github.enterprise.test"));
    assert!(io.stdout.contains("tanuki"));
    assert!(io.stdout.contains("gitlab.self-managed.test"));
    assert!(io.stderr.contains("Available GitHub accounts"));
    assert!(
        recorded
            .iter()
            .any(|event| event == "confirm:Import tanuki at gitlab.self-managed.test? [Y/n] ")
    );
    assert!(io.stdout.contains("https"));
    assert!(!io.stdout.contains(PUBLIC_KEY));
    assert!(!io.stdout.contains(SENTINEL));
    Ok(())
}

#[tokio::test]
async fn failed_host_token_import_falls_back_to_hidden_entry_without_native_output() -> TestResult {
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut discovery = FakeDiscovery::new(
        GitDefaults {
            name: None,
            email: None,
        },
        Arc::clone(&events),
    );
    discovery.accounts = vec![(
        Forge::GitHub,
        vec![HostAccount {
            hostname: "github.enterprise.test".to_owned(),
            login: Some("enterprise-user".to_owned()),
        }],
    )];
    discovery
        .failing_tokens
        .push("github.enterprise.test".to_owned());
    let mut io = FakeIo::interactive(events);
    for answer in [true, true, true, false] {
        io.push_confirm(answer);
    }
    for answer in ["Ada Lovelace", "ada@example.test", "https", "1"] {
        io.push_line(answer);
    }
    io.push_secret();
    let key = github_key();
    let mut runner = FakeRunner::with_outputs([
        configured_status(GitProtocol::Https),
        online_route(),
        output(0, [], []),
        github_status(
            "github.enterprise.test",
            "enterprise-user",
            GitProtocol::Https,
        ),
        output(0, format!("[{key}]"), []),
        output(0, format!("[{key}]"), []),
        output(0, [], []),
    ]);

    let outcome = configure_all(&mut runner, selector(), &discovery, &mut io).await?;

    assert_eq!(outcome, ConfigureOutcome::Completed);
    assert!(io.stderr.contains("Host token import was unavailable"));
    assert!(!io.stderr.contains("command did not complete"));
    assert!(
        runner
            .commands
            .iter()
            .any(|command| command.stdin.as_deref() == Some(SENTINEL.as_bytes()))
    );
    assert!(!io.stdout.contains(SENTINEL));
    assert!(!io.stderr.contains(SENTINEL));
    Ok(())
}

#[tokio::test]
async fn declined_enterprise_import_keeps_the_selected_hostname_for_hidden_entry() -> TestResult {
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut discovery = FakeDiscovery::new(
        GitDefaults {
            name: None,
            email: None,
        },
        Arc::clone(&events),
    );
    discovery.accounts = vec![(
        Forge::GitHub,
        vec![HostAccount {
            hostname: "github.enterprise.test".to_owned(),
            login: Some("enterprise-user".to_owned()),
        }],
    )];
    let mut io = FakeIo::interactive(Arc::clone(&events));
    for answer in [true, false, true, false] {
        io.push_confirm(answer);
    }
    io.push_secret();
    let mut runner = FakeRunner::with_outputs([
        configured_status(GitProtocol::Https),
        online_route(),
        output(0, [], []),
        github_status(
            "github.enterprise.test",
            "enterprise-user",
            GitProtocol::Https,
        ),
        github_keys(true),
        github_keys(true),
        output(0, [], []),
    ]);

    let outcome = configure_all(&mut runner, selector(), &discovery, &mut io).await?;

    assert_eq!(outcome, ConfigureOutcome::Completed);
    assert!(argv(&runner.commands[2]).contains(&"github.enterprise.test".to_owned()));
    assert_eq!(
        runner
            .commands
            .iter()
            .filter(|command| command.stdin.as_deref() == Some(SENTINEL.as_bytes()))
            .count(),
        1
    );
    let recorded = events.lock().map_err(|_| "test event log was poisoned")?;
    assert!(
        recorded.iter().all(|event| !event.starts_with("token:")),
        "declined import unexpectedly retrieved a host token: {recorded:?}"
    );
    assert!(!io.stdout.contains(SENTINEL));
    assert!(!io.stderr.contains(SENTINEL));
    Ok(())
}

#[tokio::test]
async fn cancellation_is_clean_and_never_writes_the_completion_receipt() -> TestResult {
    let events = Arc::new(Mutex::new(Vec::new()));
    let discovery = FakeDiscovery::new(
        GitDefaults {
            name: None,
            email: None,
        },
        Arc::clone(&events),
    );
    let mut io = FakeIo::interactive(events);
    io.confirms.push_back(Err(ConfigureError::Cancelled));
    let mut runner = FakeRunner::with_outputs([configured_status(GitProtocol::Ssh)]);

    let outcome = configure_all(&mut runner, selector(), &discovery, &mut io).await?;

    assert_eq!(outcome, ConfigureOutcome::Cancelled);
    assert_eq!(runner.commands.len(), 1);
    assert!(io.stdout.contains("Configuration cancelled"));
    assert!(!io.stdout.contains(SENTINEL));
    Ok(())
}

#[tokio::test]
async fn offline_route_skips_remote_sections_without_changing_protocol_or_receipt() -> TestResult {
    let events = Arc::new(Mutex::new(Vec::new()));
    let discovery = FakeDiscovery::new(
        GitDefaults {
            name: None,
            email: None,
        },
        Arc::clone(&events),
    );
    let mut io = FakeIo::interactive(events);
    io.push_confirm(true);
    let mut runner =
        FakeRunner::with_outputs([configured_status(GitProtocol::Https), output(0, [], [])]);

    let outcome = configure_all(&mut runner, selector(), &discovery, &mut io).await?;

    assert_eq!(outcome, ConfigureOutcome::Partial);
    assert_eq!(runner.commands.len(), 2);
    assert_eq!(
        argv(&runner.commands[1]),
        ["ip", "route", "show", "default"]
    );
    assert!(discovery.account_calls.borrow().is_empty());
    assert!(io.stderr.contains("network = \"networked\""));
    assert!(io.stderr.contains("GitHub: skipped (offline)"));
    assert!(io.stderr.contains("GitLab: skipped (offline)"));
    Ok(())
}

#[tokio::test]
async fn registration_partial_success_is_retained_and_summarized_with_focused_retry() -> TestResult
{
    let events = Arc::new(Mutex::new(Vec::new()));
    let discovery = FakeDiscovery::new(
        GitDefaults {
            name: None,
            email: None,
        },
        Arc::clone(&events),
    );
    let mut io = FakeIo::interactive(events);
    for answer in [true, true, false] {
        io.push_confirm(answer);
    }
    io.push_line("github.com");
    io.push_secret();
    let created = github_key();
    let mut runner = FakeRunner::with_outputs([
        configured_status(GitProtocol::Https),
        online_route(),
        output(0, [], []),
        github_status("github.com", "octocat", GitProtocol::Https),
        github_keys(false),
        github_keys(false),
        output(0, created, []),
        output(403, SENTINEL.as_bytes(), SENTINEL.as_bytes()),
    ]);

    let outcome = configure_all(&mut runner, selector(), &discovery, &mut io).await?;

    assert_eq!(outcome, ConfigureOutcome::Partial);
    assert!(io.stdout.contains("GitHub: octocat at github.com"));
    assert!(io.stdout.contains("authentication key added"));
    assert!(io.stdout.contains("signing key failed"));
    assert!(io.stderr.contains("gascan configure gh"));
    assert!(!io.stdout.contains(PUBLIC_KEY));
    assert!(!io.stdout.contains(SENTINEL));
    assert!(!io.stderr.contains(SENTINEL));
    Ok(())
}

#[tokio::test]
async fn focused_git_has_no_host_defaults_and_accepts_explicit_https_values() -> TestResult {
    let events = Arc::new(Mutex::new(Vec::new()));
    let discovery = FakeDiscovery::new(
        GitDefaults {
            name: None,
            email: None,
        },
        Arc::clone(&events),
    );
    let mut io = FakeIo::interactive(events);
    for answer in ["Grace Hopper", "grace@example.test", "https"] {
        io.push_line(answer);
    }
    let configured = output(
        0,
        format!(
            "{{\"name\":\"Grace Hopper\",\"email\":\"grace@example.test\",\"protocol\":\"https\",\"public_key\":\"{PUBLIC_KEY}\",\"fingerprint\":\"{FINGERPRINT}\",\"receipt\":\"pending\"}}\n"
        ),
        [],
    );
    let mut runner = FakeRunner::with_outputs([empty_status(), output(0, [], []), configured]);

    let outcome = configure_git_interactive(&mut runner, selector(), &discovery, &mut io).await?;

    assert_eq!(outcome, ConfigureOutcome::Completed);
    assert!(argv(&runner.commands[1]).ends_with(&["--protocol".to_owned(), "https".to_owned()]));
    assert!(io.stdout.contains("Grace Hopper"));
    assert!(io.stdout.contains(FINGERPRINT));
    assert_eq!(
        io.stdout,
        format!(
            "Git: Grace Hopper <grace@example.test>; protocol https; fingerprint {FINGERPRINT}\n"
        )
    );
    assert!(io.stderr.lines().any(|line| line == "Git"));
    Ok(())
}

#[tokio::test]
async fn complete_host_git_defaults_configure_with_one_confirmation() -> TestResult {
    let events = Arc::new(Mutex::new(Vec::new()));
    let discovery = FakeDiscovery::new(
        GitDefaults {
            name: Some("Ada Lovelace".to_owned()),
            email: Some("ada@example.test".to_owned()),
        },
        Arc::clone(&events),
    );
    let mut io = FakeIo::interactive(Arc::clone(&events));
    io.push_confirm(true);
    let mut runner = FakeRunner::with_outputs([
        empty_status(),
        output(0, [], []),
        configured_status(GitProtocol::Ssh),
    ]);

    let outcome = configure_git_interactive(&mut runner, selector(), &discovery, &mut io).await?;

    assert_eq!(outcome, ConfigureOutcome::Completed);
    let events = events.lock().map_err(|_| "event log poisoned")?;
    assert!(events.iter().any(|event| event.contains(
        "Use this identity with SSH transport and signed commits?"
    )));
    assert!(!events.iter().any(|event| event.contains("Git name:")));
    assert!(!events.iter().any(|event| event.contains("Git email:")));
    assert!(!events.iter().any(|event| event.contains("Git protocol")));
    Ok(())
}

#[tokio::test]
async fn declined_host_git_defaults_edit_prefilled_values() -> TestResult {
    let events = Arc::new(Mutex::new(Vec::new()));
    let discovery = FakeDiscovery::new(
        GitDefaults {
            name: Some("Ada Lovelace".to_owned()),
            email: Some("ada@old.test".to_owned()),
        },
        Arc::clone(&events),
    );
    let mut io = FakeIo::interactive(Arc::clone(&events));
    io.push_confirm(false);
    for value in ["Ada Lovelace", "ada@example.test", "https"] {
        io.push_line(value);
    }
    let mut runner = FakeRunner::with_outputs([
        empty_status(),
        output(0, [], []),
        configured_status(GitProtocol::Https),
    ]);

    let outcome = configure_git_interactive(&mut runner, selector(), &discovery, &mut io).await?;

    assert_eq!(outcome, ConfigureOutcome::Completed);
    let events = events.lock().map_err(|_| "event log poisoned")?;
    assert!(events.iter().any(|event| event == "line:Git name: "));
    assert!(events.iter().any(|event| event == "line:Git email: "));
    assert!(
        events
            .iter()
            .any(|event| event == "line:Git protocol (ssh or https): ")
    );
    assert!(argv(&runner.commands[1]).ends_with(&["--protocol".to_owned(), "https".to_owned()]));
    Ok(())
}

#[tokio::test]
async fn existing_git_configuration_is_kept_without_mutation() -> TestResult {
    let events = Arc::new(Mutex::new(Vec::new()));
    let discovery = FakeDiscovery::new(
        GitDefaults {
            name: Some("Grace Hopper".to_owned()),
            email: Some("grace@example.test".to_owned()),
        },
        Arc::clone(&events),
    );
    let mut io = FakeIo::interactive(Arc::clone(&events));
    io.push_confirm(true);
    let mut runner = FakeRunner::with_outputs([configured_status(GitProtocol::Ssh)]);

    let outcome = configure_git_interactive(&mut runner, selector(), &discovery, &mut io).await?;

    assert_eq!(outcome, ConfigureOutcome::Completed);
    assert_eq!(runner.commands.len(), 1);
    assert_eq!(io.confirm_defaults, [true]);
    let events = events.lock().map_err(|_| "event log poisoned")?;
    assert!(
        events
            .iter()
            .any(|event| event == "confirm:Keep this Git configuration? [Y/n] ")
    );
    Ok(())
}

#[tokio::test]
async fn one_detected_forge_account_imports_without_a_secret_prompt() -> TestResult {
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut discovery = FakeDiscovery::new(
        GitDefaults {
            name: None,
            email: None,
        },
        Arc::clone(&events),
    );
    discovery.accounts = vec![(
        Forge::GitHub,
        vec![HostAccount {
            hostname: "github.com".to_owned(),
            login: Some("richardkiene".to_owned()),
        }],
    )];
    let mut io = FakeIo::interactive(Arc::clone(&events));
    for answer in [true, true, false] {
        io.push_confirm(answer);
    }
    let mut outputs = vec![configured_status(GitProtocol::Https), online_route()];
    outputs.extend(successful_github_setup_outputs(
        "github.com",
        "richardkiene",
        GitProtocol::Https,
    ));
    outputs.push(output(0, [], []));
    let mut runner = FakeRunner::with_outputs(outputs);
    runner.record_events(Arc::clone(&events));

    let outcome = configure_all(&mut runner, selector(), &discovery, &mut io).await?;

    assert_eq!(outcome, ConfigureOutcome::Completed);
    let events = events.lock().map_err(|_| "event log poisoned")?;
    let accounts = events
        .iter()
        .position(|event| event == "accounts:GitHub")
        .ok_or("GitHub accounts were not requested")?;
    let selection = events
        .iter()
        .position(|event| event == "confirm:Import richardkiene at github.com? [Y/n] ")
        .ok_or("direct import prompt was not shown")?;
    let token = events
        .iter()
        .position(|event| event == "token:github.com")
        .ok_or("host token was not requested")?;
    let guest = events
        .iter()
        .position(|event| event == "guest:gh auth login")
        .ok_or("GitHub authentication command was not run")?;
    assert!(
        accounts < selection && selection < token && token < guest,
        "{events:?}"
    );
    assert!(events.iter().all(|event| event != "secret:GitHub token: "));
    for legacy in ["Configure GitHub?", "Configure GitLab?"] {
        assert!(
            events.iter().all(|event| !event.contains(legacy)),
            "{events:?}"
        );
    }
    assert!(!io.stdout.contains(SENTINEL));
    assert!(!io.stderr.contains(SENTINEL));
    Ok(())
}

#[tokio::test]
async fn declining_one_detected_forge_account_offers_hidden_manual_entry() -> TestResult {
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut discovery = FakeDiscovery::new(
        GitDefaults {
            name: None,
            email: None,
        },
        Arc::clone(&events),
    );
    discovery.accounts = vec![(
        Forge::GitHub,
        vec![HostAccount {
            hostname: "github.enterprise.test".to_owned(),
            login: Some("richardkiene".to_owned()),
        }],
    )];
    let mut io = FakeIo::interactive(Arc::clone(&events));
    for answer in [true, false, true, false] {
        io.push_confirm(answer);
    }
    io.push_secret();
    let mut outputs = vec![configured_status(GitProtocol::Https), online_route()];
    outputs.extend(successful_github_setup_outputs(
        "github.enterprise.test",
        "richardkiene",
        GitProtocol::Https,
    ));
    outputs.push(output(0, [], []));
    let mut runner = FakeRunner::with_outputs(outputs);

    let outcome = configure_all(&mut runner, selector(), &discovery, &mut io).await?;

    assert_eq!(outcome, ConfigureOutcome::Completed);
    let events = events.lock().map_err(|_| "event log poisoned")?;
    assert!(
        events
            .iter()
            .any(|event| event == "confirm:Enter a token manually? [y/N] ")
    );
    assert!(events.iter().any(|event| event == "secret:GitHub token: "));
    assert!(events.iter().all(|event| !event.starts_with("token:")));
    assert!(argv(&runner.commands[2]).contains(&"github.enterprise.test".to_owned()));
    assert!(!io.stdout.contains(SENTINEL));
    assert!(!io.stderr.contains(SENTINEL));
    Ok(())
}

#[tokio::test]
async fn multiple_forge_accounts_select_and_import_without_followup_confirmation() -> TestResult {
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut discovery = FakeDiscovery::new(
        GitDefaults {
            name: None,
            email: None,
        },
        Arc::clone(&events),
    );
    discovery.accounts = vec![(
        Forge::GitHub,
        vec![
            HostAccount {
                hostname: "github.com".to_owned(),
                login: Some("octocat".to_owned()),
            },
            HostAccount {
                hostname: "github.enterprise.test".to_owned(),
                login: Some("richardkiene".to_owned()),
            },
        ],
    )];
    let mut io = FakeIo::interactive(Arc::clone(&events));
    for answer in [true, false] {
        io.push_confirm(answer);
    }
    io.push_line("2");
    let mut outputs = vec![configured_status(GitProtocol::Https), online_route()];
    outputs.extend(successful_github_setup_outputs(
        "github.enterprise.test",
        "richardkiene",
        GitProtocol::Https,
    ));
    outputs.push(output(0, [], []));
    let mut runner = FakeRunner::with_outputs(outputs);

    let outcome = configure_all(&mut runner, selector(), &discovery, &mut io).await?;

    assert_eq!(outcome, ConfigureOutcome::Completed);
    let events = events.lock().map_err(|_| "event log poisoned")?;
    assert!(
        events.iter().any(
            |event| event == "line:Select an account (1-2), m for manual token, or s to skip: "
        )
    );
    assert!(
        events
            .iter()
            .all(|event| !event.starts_with("confirm:Import token for"))
    );
    assert!(
        events
            .iter()
            .any(|event| event == "token:github.enterprise.test")
    );
    assert!(!io.stdout.contains(SENTINEL));
    assert!(!io.stderr.contains(SENTINEL));
    Ok(())
}

#[tokio::test]
async fn multiple_forge_accounts_manual_selection_prompts_for_hostname_and_hidden_token()
-> TestResult {
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut discovery = FakeDiscovery::new(
        GitDefaults {
            name: None,
            email: None,
        },
        Arc::clone(&events),
    );
    discovery.accounts = vec![(
        Forge::GitHub,
        vec![
            HostAccount {
                hostname: "github.com".to_owned(),
                login: Some("octocat".to_owned()),
            },
            HostAccount {
                hostname: "github.enterprise.test".to_owned(),
                login: Some("richardkiene".to_owned()),
            },
        ],
    )];
    let mut io = FakeIo::interactive(Arc::clone(&events));
    for answer in [true, false] {
        io.push_confirm(answer);
    }
    io.push_line("m");
    io.push_line("github.manual.test");
    io.push_secret();
    let mut outputs = vec![configured_status(GitProtocol::Https), online_route()];
    outputs.extend(successful_github_setup_outputs(
        "github.manual.test",
        "manual-user",
        GitProtocol::Https,
    ));
    outputs.push(output(0, [], []));
    let mut runner = FakeRunner::with_outputs(outputs);

    let outcome = configure_all(&mut runner, selector(), &discovery, &mut io).await?;

    assert_eq!(outcome, ConfigureOutcome::Completed);
    let events = events.lock().map_err(|_| "event log poisoned")?;
    assert!(
        events.iter().any(
            |event| event == "line:Select an account (1-2), m for manual token, or s to skip: "
        )
    );
    assert!(events.iter().any(|event| event == "line:GitHub hostname: "));
    assert!(events.iter().any(|event| event == "secret:GitHub token: "));
    assert!(events.iter().all(|event| !event.starts_with("token:")));
    assert!(argv(&runner.commands[2]).contains(&"github.manual.test".to_owned()));
    assert!(!io.stdout.contains(SENTINEL));
    assert!(!io.stderr.contains(SENTINEL));
    Ok(())
}

#[tokio::test]
async fn multiple_forge_accounts_skip_selection_avoids_forge_commands_and_completes_receipt()
-> TestResult {
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut discovery = FakeDiscovery::new(
        GitDefaults {
            name: None,
            email: None,
        },
        Arc::clone(&events),
    );
    discovery.accounts = vec![(
        Forge::GitHub,
        vec![
            HostAccount {
                hostname: "github.com".to_owned(),
                login: Some("octocat".to_owned()),
            },
            HostAccount {
                hostname: "github.enterprise.test".to_owned(),
                login: Some("richardkiene".to_owned()),
            },
        ],
    )];
    let mut io = FakeIo::interactive(Arc::clone(&events));
    for answer in [true, false] {
        io.push_confirm(answer);
    }
    io.push_line("s");
    let mut runner = FakeRunner::with_outputs([
        configured_status(GitProtocol::Https),
        online_route(),
        output(0, [], []),
    ]);
    runner.record_events(Arc::clone(&events));

    let outcome = configure_all(&mut runner, selector(), &discovery, &mut io).await?;

    assert_eq!(outcome, ConfigureOutcome::Completed);
    assert_eq!(runner.commands.len(), 3);
    assert!(runner.commands.iter().all(|command| {
        !argv(command)
            .iter()
            .any(|argument| argument == "gh" || argument == "glab")
    }));
    assert!(runner.commands.iter().any(|command| {
        argv(command).ends_with(&["receipt".to_owned(), "complete".to_owned()])
    }));
    let events = events.lock().map_err(|_| "event log poisoned")?;
    assert!(events.iter().all(|event| !event.starts_with("token:")));
    assert!(events.iter().all(|event| event != "secret:GitHub token: "));
    assert!(io.stderr.contains("GitHub: skipped"));
    assert!(io.stderr.contains("GitLab: skipped"));
    Ok(())
}

#[tokio::test]
async fn no_forge_account_offers_default_no_manual_configuration() -> TestResult {
    let events = Arc::new(Mutex::new(Vec::new()));
    let discovery = FakeDiscovery::new(
        GitDefaults {
            name: None,
            email: None,
        },
        Arc::clone(&events),
    );
    let mut io = FakeIo::interactive(Arc::clone(&events));
    for answer in [true, false, false] {
        io.push_confirm(answer);
    }
    let mut runner = FakeRunner::with_outputs([
        configured_status(GitProtocol::Https),
        online_route(),
        output(0, [], []),
    ]);

    let outcome = configure_all(&mut runner, selector(), &discovery, &mut io).await?;

    assert_eq!(outcome, ConfigureOutcome::Completed);
    let events = events.lock().map_err(|_| "event log poisoned")?;
    assert!(
        events
            .iter()
            .any(|event| event == "confirm:Configure GitHub with a token? [y/N] ")
    );
    assert!(events.iter().all(|event| event != "secret:GitHub token: "));
    assert_eq!(runner.commands.len(), 3);
    assert!(io.stderr.contains("GitHub: skipped"));
    assert!(io.stderr.contains("GitLab: skipped"));
    Ok(())
}

#[tokio::test]
async fn failed_selected_forge_token_import_offers_manual_fallback() -> TestResult {
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut discovery = FakeDiscovery::new(
        GitDefaults {
            name: None,
            email: None,
        },
        Arc::clone(&events),
    );
    discovery.accounts = vec![(
        Forge::GitHub,
        vec![HostAccount {
            hostname: "github.enterprise.test".to_owned(),
            login: Some("richardkiene".to_owned()),
        }],
    )];
    discovery
        .failing_tokens
        .push("github.enterprise.test".to_owned());
    let mut io = FakeIo::interactive(Arc::clone(&events));
    for answer in [true, true, true, false] {
        io.push_confirm(answer);
    }
    io.push_secret();
    let mut outputs = vec![configured_status(GitProtocol::Https), online_route()];
    outputs.extend(successful_github_setup_outputs(
        "github.enterprise.test",
        "richardkiene",
        GitProtocol::Https,
    ));
    outputs.push(output(0, [], []));
    let mut runner = FakeRunner::with_outputs(outputs);

    let outcome = configure_all(&mut runner, selector(), &discovery, &mut io).await?;

    assert_eq!(outcome, ConfigureOutcome::Completed);
    assert!(io.stderr.contains("Host token import was unavailable"));
    let events = events.lock().map_err(|_| "event log poisoned")?;
    assert!(
        events
            .iter()
            .any(|event| event == "confirm:Enter a token manually? [y/N] ")
    );
    assert!(events.iter().any(|event| event == "secret:GitHub token: "));
    assert!(!io.stdout.contains(SENTINEL));
    assert!(!io.stderr.contains(SENTINEL));
    Ok(())
}

#[tokio::test]
async fn focused_forge_uses_the_provided_secret_once_and_updates_the_requested_protocol()
-> TestResult {
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut io = FakeIo::interactive(events);
    let mut runner = FakeRunner::with_outputs([
        configured_status(GitProtocol::Https),
        output(0, [], []),
        configured_status(GitProtocol::Ssh),
        output(0, [], []),
        gitlab_status("gitlab.enterprise.test", "tanuki", GitProtocol::Ssh),
        gitlab_keys(true),
        output(0, [], []),
        output(0, "Welcome to GitLab, @tanuki!\n", []),
    ]);
    runner.interactive.push_back(Ok(0));

    let outcome = configure_forge_interactive(
        &mut runner,
        selector(),
        Forge::GitLab,
        "gitlab.enterprise.test".to_owned(),
        GitProtocol::Ssh,
        Some(Secret::new(SENTINEL.as_bytes().to_vec())),
        &mut io,
    )
    .await?;

    assert_eq!(outcome, ConfigureOutcome::Completed);
    assert_eq!(
        runner
            .commands
            .iter()
            .filter(|command| command.stdin.as_deref() == Some(SENTINEL.as_bytes()))
            .count(),
        1
    );
    assert!(io.stdout.contains("tanuki"));
    assert!(io.stdout.contains("gitlab.enterprise.test"));
    assert!(!io.stdout.contains(SENTINEL));
    assert!(!io.stderr.contains(SENTINEL));
    Ok(())
}
