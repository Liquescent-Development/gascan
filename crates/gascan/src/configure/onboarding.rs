use super::{
    ConfigureError, ConfigureIo, Forge, ForgeRequest, ForgeSetup, GitDefaults, GitProtocol,
    GitRequest, GitSetup, HostDiscovery, ReceiptState, RegistrationState, SystemHostDiscovery,
    complete_receipt, configure_forge, configure_git, current_git_setup, decline_receipt,
    receipt_state,
};
use crate::client::Client;
use crate::guest::{ClientGuestRunner, GuestCommand, GuestRunner, Secret};
use gascan_proto::v1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ConfigureOutcome {
    Completed,
    Cancelled,
    Partial,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OfferResult {
    Suppressed,
    Pending,
    Declined,
    Completed,
    Cancelled,
}

pub(crate) async fn offer_after_up(
    client: &mut Client,
    selector: v1::SandboxSelector,
    io: &mut dyn ConfigureIo,
) -> Result<OfferResult, ConfigureError> {
    let discovery = SystemHostDiscovery::new();
    let mut runner = ClientGuestRunner::new(client);
    offer_after_up_with(&mut runner, selector, &discovery, io).await
}

pub(super) async fn offer_after_up_with<R: GuestRunner, H: HostDiscovery + ?Sized>(
    runner: &mut R,
    selector: v1::SandboxSelector,
    discovery: &H,
    io: &mut dyn ConfigureIo,
) -> Result<OfferResult, ConfigureError> {
    if !io.stdin_is_terminal() || !io.stderr_is_terminal() {
        return Ok(OfferResult::Suppressed);
    }
    match receipt_state(runner, selector.clone()).await? {
        ReceiptState::Complete => return Ok(OfferResult::Completed),
        ReceiptState::Declined => return Ok(OfferResult::Declined),
        ReceiptState::Pending => {}
    }
    let accepted = match io.confirm(
        "Set up Git, GitHub, and GitLab for this sandbox now? [Y/n] ",
        true,
    ) {
        Ok(accepted) => accepted,
        Err(ConfigureError::Cancelled) => return Ok(OfferResult::Cancelled),
        Err(error) => return Err(error),
    };
    if !accepted {
        decline_receipt(runner, selector).await?;
        io.write_err("Run 'gascan configure' whenever you are ready.\n")?;
        return Ok(OfferResult::Declined);
    }
    Ok(
        match configure_all(runner, selector, discovery, io).await? {
            ConfigureOutcome::Completed => OfferResult::Completed,
            ConfigureOutcome::Cancelled => OfferResult::Cancelled,
            ConfigureOutcome::Partial => OfferResult::Pending,
        },
    )
}

enum RemoteSummary {
    Setup {
        setup: ForgeSetup,
        protocol: GitProtocol,
    },
    Skipped(&'static str),
}

pub(crate) async fn configure_all<R: GuestRunner, H: HostDiscovery + ?Sized>(
    runner: &mut R,
    selector: v1::SandboxSelector,
    discovery: &H,
    io: &mut dyn ConfigureIo,
) -> Result<ConfigureOutcome, ConfigureError> {
    match configure_all_inner(runner, selector, discovery, io).await {
        Err(ConfigureError::Cancelled) => {
            io.write_out("Configuration cancelled; no completion receipt was written.\n")?;
            Ok(ConfigureOutcome::Cancelled)
        }
        result => result,
    }
}

async fn configure_all_inner<R: GuestRunner, H: HostDiscovery + ?Sized>(
    runner: &mut R,
    selector: v1::SandboxSelector,
    discovery: &H,
    io: &mut dyn ConfigureIo,
) -> Result<ConfigureOutcome, ConfigureError> {
    io.write_err("Git\n")?;
    let current = current_git_setup(runner, selector.clone()).await?;
    let defaults = match discovery.git_defaults() {
        Ok(defaults) => defaults,
        Err(_) => {
            io.write_err("Host Git defaults were unavailable; enter values manually.\n")?;
            GitDefaults {
                name: None,
                email: None,
            }
        }
    };
    show_git_state(io, current.as_ref(), &defaults)?;
    let configure = io.confirm("Configure Git identity and signing? [Y/n] ", true)?;
    let git = if configure {
        Some(configure_git_values(runner, selector.clone(), current, defaults, io).await?)
    } else {
        current
    };

    let Some(git) = git else {
        let github = RemoteSummary::Skipped(" (Git/key unavailable)");
        let gitlab = RemoteSummary::Skipped(" (Git/key unavailable)");
        write_summary(io, None, &github, &gitlab)?;
        return Ok(ConfigureOutcome::Partial);
    };

    match has_default_route(runner, selector.clone()).await {
        Ok(true) => {}
        Ok(false) => {
            io.write_err(
                "Remote setup skipped: this sandbox has no usable default route; set network = \"networked\" and retry.\n",
            )?;
            let github = RemoteSummary::Skipped(" (offline)");
            let gitlab = RemoteSummary::Skipped(" (offline)");
            write_summary(io, Some(&git), &github, &gitlab)?;
            return Ok(ConfigureOutcome::Partial);
        }
        Err(_) => {
            io.write_err(
                "Remote setup paused because the default route probe failed; Git setup was retained. Retry with `gascan configure gh` and `gascan configure glab`.\n",
            )?;
            let github = RemoteSummary::Skipped(" (route probe failed; retry available)");
            let gitlab = RemoteSummary::Skipped(" (route probe failed; retry available)");
            write_summary(io, Some(&git), &github, &gitlab)?;
            return Ok(ConfigureOutcome::Partial);
        }
    }

    let (github, github_failed) =
        configure_remote_section(runner, &selector, discovery, io, Forge::GitHub, &git).await?;
    let (gitlab, gitlab_failed) =
        configure_remote_section(runner, &selector, discovery, io, Forge::GitLab, &git).await?;
    write_summary(io, Some(&git), &github, &gitlab)?;
    if github_failed || gitlab_failed {
        Ok(ConfigureOutcome::Partial)
    } else {
        complete_receipt(runner, selector).await?;
        Ok(ConfigureOutcome::Completed)
    }
}

pub(crate) async fn configure_git_interactive<R: GuestRunner, H: HostDiscovery + ?Sized>(
    runner: &mut R,
    selector: v1::SandboxSelector,
    discovery: &H,
    io: &mut dyn ConfigureIo,
) -> Result<ConfigureOutcome, ConfigureError> {
    let result = async {
        io.write_err("Git\n")?;
        let current = current_git_setup(runner, selector.clone()).await?;
        let defaults = discovery.git_defaults().unwrap_or(GitDefaults {
            name: None,
            email: None,
        });
        show_git_state(io, current.as_ref(), &defaults)?;
        let setup = configure_git_values(runner, selector, current, defaults, io).await?;
        write_git_summary(io, &setup)?;
        Ok(ConfigureOutcome::Completed)
    }
    .await;
    clean_cancellation(result, io)
}

pub(crate) async fn configure_forge_interactive<R: GuestRunner>(
    runner: &mut R,
    selector: v1::SandboxSelector,
    forge: Forge,
    hostname: String,
    protocol: GitProtocol,
    token: Option<Secret>,
    io: &mut dyn ConfigureIo,
) -> Result<ConfigureOutcome, ConfigureError> {
    let result = async {
        let Some(mut git) = current_git_setup(runner, selector.clone()).await? else {
            return Err(ConfigureError::GuestCommand {
                category: "developer configuration",
                message: "Git identity and key are not configured; run `gascan configure git`"
                    .to_owned(),
            });
        };
        if git.protocol != protocol {
            git = configure_git(
                runner,
                selector.clone(),
                GitRequest {
                    sandbox_id: selector.sandbox_id.clone(),
                    name: git.name,
                    email: git.email,
                    protocol,
                },
            )
            .await?;
        }
        let token = match token {
            Some(token) => token,
            None => io
                .secret(match forge {
                    Forge::GitHub => "GitHub token: ",
                    Forge::GitLab => "GitLab token: ",
                })?
                .ok_or(ConfigureError::Cancelled)?,
        };
        let result = configure_forge(
            runner,
            selector,
            ForgeRequest {
                forge,
                hostname,
                protocol,
                token,
                key: git,
            },
        )
        .await;
        let (summary, failed) = remote_result(result, protocol, io)?;
        write_remote_summary(io, forge, &summary)?;
        Ok(if failed {
            ConfigureOutcome::Partial
        } else {
            ConfigureOutcome::Completed
        })
    }
    .await;
    clean_cancellation(result, io)
}

fn clean_cancellation(
    result: Result<ConfigureOutcome, ConfigureError>,
    io: &mut dyn ConfigureIo,
) -> Result<ConfigureOutcome, ConfigureError> {
    match result {
        Err(ConfigureError::Cancelled) => {
            io.write_out("Configuration cancelled.\n")?;
            Ok(ConfigureOutcome::Cancelled)
        }
        result => result,
    }
}

async fn configure_git_values<R: GuestRunner>(
    runner: &mut R,
    selector: v1::SandboxSelector,
    current: Option<GitSetup>,
    defaults: GitDefaults,
    io: &mut dyn ConfigureIo,
) -> Result<GitSetup, ConfigureError> {
    let name_default = current
        .as_ref()
        .map(|setup| setup.name.as_str())
        .or(defaults.name.as_deref());
    let email_default = current
        .as_ref()
        .map(|setup| setup.email.as_str())
        .or(defaults.email.as_deref());
    let name = required_line(io, "Git name: ", name_default)?;
    let email = required_line(io, "Git email: ", email_default)?;
    let protocol_default = current
        .as_ref()
        .map_or(GitProtocol::Ssh, |setup| setup.protocol);
    let protocol = prompt_protocol(io, protocol_default)?;
    if let Some(current) = current {
        if current.name == name && current.email == email && current.protocol == protocol {
            return Ok(current);
        }
    }
    configure_git(
        runner,
        selector.clone(),
        GitRequest {
            sandbox_id: selector.sandbox_id,
            name,
            email,
            protocol,
        },
    )
    .await
}

fn required_line(
    io: &mut dyn ConfigureIo,
    prompt: &str,
    default: Option<&str>,
) -> Result<String, ConfigureError> {
    let value = io.line(prompt, default)?.ok_or(ConfigureError::Cancelled)?;
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err(ConfigureError::InvalidOutput { category: "prompt" });
    }
    Ok(value)
}

fn prompt_protocol(
    io: &mut dyn ConfigureIo,
    default: GitProtocol,
) -> Result<GitProtocol, ConfigureError> {
    let default = protocol_name(default);
    match io
        .line("Git protocol (ssh or https): ", Some(default))?
        .ok_or(ConfigureError::Cancelled)?
        .as_str()
    {
        "ssh" => Ok(GitProtocol::Ssh),
        "https" => Ok(GitProtocol::Https),
        _ => Err(ConfigureError::InvalidOutput { category: "prompt" }),
    }
}

fn show_git_state(
    io: &mut dyn ConfigureIo,
    current: Option<&GitSetup>,
    defaults: &GitDefaults,
) -> Result<(), ConfigureError> {
    if let Some(current) = current {
        io.write_err(&format!(
            "Current: {} <{}>; protocol {}; fingerprint {}\n",
            current.name,
            current.email,
            protocol_name(current.protocol),
            current.fingerprint
        ))?;
    } else {
        io.write_err("Current: not configured\n")?;
    }
    if defaults.name.is_some() || defaults.email.is_some() {
        io.write_err(&format!(
            "Host defaults: {} <{}>\n",
            defaults.name.as_deref().unwrap_or("not set"),
            defaults.email.as_deref().unwrap_or("not set")
        ))?;
    }
    Ok(())
}

async fn has_default_route<R: GuestRunner>(
    runner: &mut R,
    selector: v1::SandboxSelector,
) -> Result<bool, ConfigureError> {
    let output = runner
        .execute(
            selector,
            GuestCommand {
                argv: ["ip", "route", "show", "default"]
                    .into_iter()
                    .map(|argument| argument.as_bytes().to_vec())
                    .collect(),
                environment: Vec::new(),
                stdin: None,
            },
        )
        .await
        .map_err(|_| ConfigureError::GuestCommand {
            category: "default route probe",
            message: "guest route inspection was unavailable".to_owned(),
        })?;
    if output.code != 0 || !output.stderr.is_empty() {
        return Ok(false);
    }
    let text = std::str::from_utf8(&output.stdout).map_err(|_| ConfigureError::InvalidOutput {
        category: "default route probe",
    })?;
    Ok(text
        .lines()
        .any(|line| line.split_ascii_whitespace().next() == Some("default")))
}

async fn configure_remote_section<R: GuestRunner, H: HostDiscovery + ?Sized>(
    runner: &mut R,
    selector: &v1::SandboxSelector,
    discovery: &H,
    io: &mut dyn ConfigureIo,
    forge: Forge,
    git: &GitSetup,
) -> Result<(RemoteSummary, bool), ConfigureError> {
    let name = forge_name(forge);
    io.write_err(&format!("{name}\n"))?;
    if !io.confirm(&format!("Configure {name}? [Y/n] "), true)? {
        return Ok((RemoteSummary::Skipped(""), false));
    }
    let (hostname, token) = choose_forge_token(discovery, io, forge)?;
    let result = configure_forge(
        runner,
        selector.clone(),
        ForgeRequest {
            forge,
            hostname,
            protocol: git.protocol,
            token,
            key: git.clone(),
        },
    )
    .await;
    remote_result(result, git.protocol, io)
}

fn choose_forge_token<H: HostDiscovery + ?Sized>(
    discovery: &H,
    io: &mut dyn ConfigureIo,
    forge: Forge,
) -> Result<(String, Secret), ConfigureError> {
    let accounts = match discovery.accounts(forge) {
        Ok(accounts) => accounts,
        Err(_) => {
            io.write_err("Host account import was unavailable; enter a token manually.\n")?;
            Vec::new()
        }
    };
    if !accounts.is_empty() {
        io.write_err(&format!("Available {} accounts:\n", forge_name(forge)))?;
        for (index, account) in accounts.iter().enumerate() {
            io.write_err(&format!(
                "  {}. {} at {}\n",
                index + 1,
                account.login.as_deref().unwrap_or("unknown account"),
                account.hostname
            ))?;
        }
        let selection = io
            .line(
                "Import account number, or press Enter for hidden token entry: ",
                None,
            )?
            .ok_or(ConfigureError::Cancelled)?;
        if !selection.is_empty() {
            let index = selection
                .parse::<usize>()
                .ok()
                .and_then(|index| index.checked_sub(1))
                .filter(|index| *index < accounts.len())
                .ok_or(ConfigureError::InvalidOutput { category: "prompt" })?;
            let account = &accounts[index];
            let label = account.login.as_deref().unwrap_or("unknown account");
            if io.confirm(
                &format!("Import token for {label} at {}? [y/N] ", account.hostname),
                false,
            )? {
                match discovery.token(forge, account) {
                    Ok(token) => return Ok((account.hostname.clone(), token)),
                    Err(_) => {
                        io.write_err(
                            "Host token import was unavailable; enter the token instead.\n",
                        )?;
                        let token = hidden_token(io, forge)?;
                        return Ok((account.hostname.clone(), token));
                    }
                }
            }
            return Ok((account.hostname.clone(), hidden_token(io, forge)?));
        }
    }
    let hostname = required_line(
        io,
        match forge {
            Forge::GitHub => "GitHub hostname: ",
            Forge::GitLab => "GitLab hostname: ",
        },
        Some(default_hostname(forge)),
    )?;
    Ok((hostname, hidden_token(io, forge)?))
}

fn hidden_token(io: &mut dyn ConfigureIo, forge: Forge) -> Result<Secret, ConfigureError> {
    io.secret(match forge {
        Forge::GitHub => "GitHub token: ",
        Forge::GitLab => "GitLab token: ",
    })?
    .ok_or(ConfigureError::Cancelled)
}

fn remote_result(
    result: Result<ForgeSetup, ConfigureError>,
    protocol: GitProtocol,
    io: &mut dyn ConfigureIo,
) -> Result<(RemoteSummary, bool), ConfigureError> {
    match result {
        Ok(setup) => Ok((RemoteSummary::Setup { setup, protocol }, false)),
        Err(ConfigureError::Forge {
            setup,
            category,
            hostname,
            message,
            retry,
        }) => {
            io.write_err(&format!(
                "{category} for {hostname} failed: {message}; retry with `{retry}`\n"
            ))?;
            Ok((
                RemoteSummary::Setup {
                    setup: *setup,
                    protocol,
                },
                true,
            ))
        }
        Err(error) => Err(error),
    }
}

fn write_summary(
    io: &mut dyn ConfigureIo,
    git: Option<&GitSetup>,
    github: &RemoteSummary,
    gitlab: &RemoteSummary,
) -> Result<(), ConfigureError> {
    io.write_out("Summary\n")?;
    if let Some(git) = git {
        write_git_summary(io, git)?;
    } else {
        io.write_out("Git: skipped\n")?;
    }
    write_remote_summary(io, Forge::GitHub, github)?;
    write_remote_summary(io, Forge::GitLab, gitlab)
}

fn write_git_summary(io: &mut dyn ConfigureIo, git: &GitSetup) -> Result<(), ConfigureError> {
    io.write_out(&format!(
        "Git: {} <{}>; protocol {}; fingerprint {}\n",
        git.name,
        git.email,
        protocol_name(git.protocol),
        git.fingerprint
    ))
}

fn write_remote_summary(
    io: &mut dyn ConfigureIo,
    forge: Forge,
    summary: &RemoteSummary,
) -> Result<(), ConfigureError> {
    let name = forge_name(forge);
    match summary {
        RemoteSummary::Skipped(reason) => io.write_out(&format!("{name}: skipped{reason}\n")),
        RemoteSummary::Setup { setup, protocol } => io.write_out(&format!(
            "{name}: {} at {}; protocol {}; authentication {}; authentication key {}; signing key {}\n",
            setup.login,
            setup.hostname,
            protocol_name(*protocol),
            if setup.authenticated { "configured" } else { "failed" },
            registration_name(setup.authentication_key),
            registration_name(setup.signing_key),
        )),
    }
}

const fn registration_name(state: RegistrationState) -> &'static str {
    match state {
        RegistrationState::Existing => "existing",
        RegistrationState::Added => "added",
        RegistrationState::Skipped => "skipped",
        RegistrationState::Failed => "failed",
    }
}

const fn protocol_name(protocol: GitProtocol) -> &'static str {
    match protocol {
        GitProtocol::Ssh => "ssh",
        GitProtocol::Https => "https",
    }
}

const fn forge_name(forge: Forge) -> &'static str {
    match forge {
        Forge::GitHub => "GitHub",
        Forge::GitLab => "GitLab",
    }
}

const fn default_hostname(forge: Forge) -> &'static str {
    match forge {
        Forge::GitHub => "github.com",
        Forge::GitLab => "gitlab.com",
    }
}
