// Task 1 establishes the presentation API; command integration follows in later tasks.
#![allow(dead_code)]

use console::{Style, Term};
use gascan_proto::v1;
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use std::fmt::Write as _;
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OperationKind {
    Up,
    Apply,
    Down,
    Destroy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct OutputCapabilities {
    interactive: bool,
    color: bool,
    unicode: bool,
}

impl OutputCapabilities {
    pub(crate) fn for_stdout() -> Self {
        Self::for_term(
            Term::stdout(),
            std::env::var_os("NO_COLOR").is_none() && console::colors_enabled(),
        )
    }

    pub(crate) fn for_stderr() -> Self {
        Self::for_term(
            Term::stderr(),
            std::env::var_os("NO_COLOR").is_none() && console::colors_enabled_stderr(),
        )
    }

    fn for_term(term: Term, color: bool) -> Self {
        let interactive = term.is_term();
        Self {
            interactive,
            color,
            unicode: term.features().wants_emoji(),
        }
    }

    #[cfg(test)]
    fn plain() -> Self {
        Self {
            interactive: false,
            color: false,
            unicode: false,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct DoctorCheck {
    pub(crate) id: String,
    pub(crate) status: String,
    pub(crate) detail: String,
    pub(crate) remedy: String,
}

pub(crate) fn render_doctor(checks: &[DoctorCheck], capabilities: OutputCapabilities) -> String {
    struct Group<'a> {
        id: &'a str,
        checks: Vec<&'a DoctorCheck>,
    }

    let mut groups: Vec<Group<'_>> = Vec::new();
    for check in checks {
        let group_id = check
            .id
            .split_once('.')
            .map_or(check.id.as_str(), |(id, _)| id);
        if let Some(group) = groups.iter_mut().find(|group| group.id == group_id) {
            group.checks.push(check);
        } else {
            groups.push(Group {
                id: group_id,
                checks: vec![check],
            });
        }
    }

    let ready = checks.iter().all(|check| check.status == "pass");
    let heading = if ready {
        styled_heading("Gascan is ready", "✓", true, capabilities)
    } else {
        styled_heading("Gascan needs attention", "✗", false, capabilities)
    };
    let group_width = groups
        .iter()
        .map(|group| humanize(group.id).len())
        .max()
        .unwrap_or(0)
        .max("Workspace".len());
    let mut output = format!("{heading}\n");
    for group in groups {
        let title = humanize(group.id);
        let passing = group
            .checks
            .iter()
            .filter(|check| check.status == "pass")
            .count();
        let total = group.checks.len();
        let noun = if total == 1 { "check" } else { "checks" };
        let _ = writeln!(
            output,
            "  {title:<group_width$}  {passing}/{total} {noun} passed"
        );
        for check in group
            .checks
            .into_iter()
            .filter(|check| check.status != "pass")
        {
            let check_id = check
                .id
                .split_once('.')
                .map_or(check.id.as_str(), |(_, id)| id);
            let check_heading = styled_heading(&humanize(check_id), "✗", false, capabilities);
            let _ = writeln!(output, "    {check_heading}");
            if !check.detail.is_empty() {
                let _ = writeln!(output, "      {}", check.detail);
            }
            if !check.remedy.is_empty() {
                let _ = writeln!(output, "      Fix: {}", check.remedy);
            }
        }
    }
    output
}

pub(crate) fn render_status(
    status: &v1::SandboxStatus,
    _capabilities: OutputCapabilities,
) -> String {
    format!(
        "Sandbox: {}\nState:   {}\n",
        status.sandbox_id,
        human_state(status.actual_state)
    )
}

pub(crate) fn render_list(
    sandboxes: &[v1::SandboxStatus],
    _capabilities: OutputCapabilities,
) -> String {
    if sandboxes.is_empty() {
        return "No sandboxes found.\n".to_owned();
    }
    let sandbox_width = sandboxes
        .iter()
        .map(|sandbox| sandbox.sandbox_id.len())
        .max()
        .unwrap_or(0)
        .max("SANDBOX".len());
    let mut output = format!("{:<sandbox_width$}  STATE\n", "SANDBOX");
    for sandbox in sandboxes {
        let _ = writeln!(
            output,
            "{:<sandbox_width$}  {}",
            sandbox.sandbox_id,
            human_state(sandbox.actual_state)
        );
    }
    output
}

fn styled_heading(
    heading: &str,
    symbol: &str,
    success: bool,
    capabilities: OutputCapabilities,
) -> String {
    let heading = if capabilities.unicode {
        format!("{symbol} {heading}")
    } else {
        heading.to_owned()
    };
    if !capabilities.color {
        return heading;
    }
    let style = if success {
        Style::new().green()
    } else {
        Style::new().red()
    };
    style.apply_to(heading).to_string()
}

fn humanize(identifier: &str) -> String {
    let mut value = identifier.replace('_', " ");
    if let Some(first) = value.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    value
}

fn human_state(value: i32) -> &'static str {
    match v1::ActualState::try_from(value).unwrap_or(v1::ActualState::Unknown) {
        v1::ActualState::Pending => "Pending",
        v1::ActualState::Running => "Running",
        v1::ActualState::Stopped => "Stopped",
        v1::ActualState::Absent => "Absent",
        v1::ActualState::Failed => "Failed",
        _ => "Unknown",
    }
}

pub(crate) struct OperationProgress {
    kind: OperationKind,
    sandbox_id: Option<String>,
    color: bool,
    return_interactive_completion: bool,
    progress_bar: Option<ProgressBar>,
    last_message: Option<&'static str>,
}

impl OperationProgress {
    pub(crate) fn new(
        kind: OperationKind,
        sandbox_id: Option<String>,
        capabilities: OutputCapabilities,
    ) -> (Self, Option<String>) {
        let draw_target = capabilities
            .interactive
            .then(|| ProgressDrawTarget::term_like_with_hz(Box::new(console::Term::stderr()), 12));
        Self::create(kind, sandbox_id, capabilities, draw_target, true)
    }

    pub(crate) fn with_draw_target(
        kind: OperationKind,
        sandbox_id: Option<String>,
        capabilities: OutputCapabilities,
        draw_target: ProgressDrawTarget,
    ) -> (Self, Option<String>) {
        let draw_target = capabilities.interactive.then_some(draw_target);
        Self::create(kind, sandbox_id, capabilities, draw_target, false)
    }

    fn create(
        kind: OperationKind,
        sandbox_id: Option<String>,
        capabilities: OutputCapabilities,
        draw_target: Option<ProgressDrawTarget>,
        return_interactive_completion: bool,
    ) -> (Self, Option<String>) {
        let initial = match kind {
            OperationKind::Up => "Preparing sandbox",
            OperationKind::Apply => "Applying configuration",
            OperationKind::Down => "Stopping sandbox",
            OperationKind::Destroy => "Destroying sandbox",
        };
        let progress_bar = draw_target.map(|draw_target| {
            let progress_bar = ProgressBar::with_draw_target(None, draw_target);
            let template = if capabilities.color {
                "{spinner:.cyan} {msg}"
            } else {
                "{spinner} {msg}"
            };
            let mut style = ProgressStyle::with_template(template)
                .unwrap_or_else(|_| ProgressStyle::default_spinner());
            style = if capabilities.unicode {
                style.tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"])
            } else {
                style.tick_strings(&["-", "\\", "|", "/"])
            };
            progress_bar.set_style(style);
            progress_bar.set_message(initial);
            progress_bar.enable_steady_tick(Duration::from_millis(80));
            progress_bar
        });
        let output = (!capabilities.interactive).then(|| initial.to_owned());
        (
            Self {
                kind,
                sandbox_id,
                color: capabilities.color,
                return_interactive_completion,
                progress_bar,
                last_message: Some(initial),
            },
            output,
        )
    }

    pub(crate) fn update(&mut self, event: &v1::OperationEvent) -> Option<String> {
        let message = match (
            event.phase.as_str(),
            v1::ProvisionStep::try_from(event.provision_step).ok(),
        ) {
            ("validated", _) => Some("Validating configuration"),
            ("created", _) => Some("Creating sandbox"),
            ("started", _) => Some("Starting sandbox"),
            ("apply_required", _) => Some("Preparing configuration changes"),
            ("before_health", _) => Some("Checking sandbox health"),
            ("provision_step", Some(v1::ProvisionStep::WriteSafeMiseConfig)) => {
                Some("Writing safe mise configuration")
            }
            ("provision_step", Some(v1::ProvisionStep::InstallTools)) => {
                Some("Installing project tools")
            }
            ("provision_step", Some(v1::ProvisionStep::RunSetup)) => Some("Running project setup"),
            ("provision_step", Some(v1::ProvisionStep::VerifyGascamp)) => Some("Verifying Gascamp"),
            ("provision_step", Some(v1::ProvisionStep::HealthCheck)) => {
                Some("Checking sandbox health")
            }
            _ => None,
        }?;

        if self.last_message == Some(message) {
            return None;
        }
        self.last_message = Some(message);
        if let Some(progress_bar) = &self.progress_bar {
            progress_bar.set_message(message);
            None
        } else {
            Some(message.to_owned())
        }
    }

    pub(crate) fn finish_success(mut self) -> Option<String> {
        let completion = match (self.kind, self.sandbox_id.as_deref()) {
            (OperationKind::Up, None) => "Sandbox is running".to_owned(),
            (OperationKind::Up, Some(id)) => format!("Sandbox {id} is running"),
            (OperationKind::Apply, None) => "Sandbox configuration is up to date".to_owned(),
            (OperationKind::Apply, Some(id)) => {
                format!("Sandbox {id} configuration is up to date")
            }
            (OperationKind::Down, None) => "Sandbox is stopped".to_owned(),
            (OperationKind::Down, Some(id)) => format!("Sandbox {id} is stopped"),
            (OperationKind::Destroy, None) => "Sandbox is destroyed".to_owned(),
            (OperationKind::Destroy, Some(id)) => format!("Sandbox {id} is destroyed"),
        };

        if let Some(progress_bar) = self.progress_bar.take() {
            let check = if self.color {
                "\u{1b}[32m✓\u{1b}[0m"
            } else {
                "✓"
            };
            progress_bar.finish_and_clear();
            let completion = format!("{check} {completion}");
            if self.return_interactive_completion {
                Some(completion)
            } else {
                progress_bar.println(completion);
                None
            }
        } else {
            Some(completion)
        }
    }

    pub(crate) fn clear(&mut self) {
        if let Some(progress_bar) = self.progress_bar.take() {
            progress_bar.finish_and_clear();
        }
    }
}

impl Drop for OperationProgress {
    fn drop(&mut self) {
        self.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gascan_proto::v1;
    use indicatif::{InMemoryTerm, ProgressDrawTarget};
    use std::process::Command;

    fn check(id: &str, status: &str, detail: &str, remedy: &str) -> DoctorCheck {
        DoctorCheck {
            id: id.to_owned(),
            status: status.to_owned(),
            detail: detail.to_owned(),
            remedy: remedy.to_owned(),
        }
    }

    fn passing_checks() -> Vec<DoctorCheck> {
        vec![
            check("host.release", "pass", "report sha256: abc", ""),
            check("host.fixture", "pass", "fixture sha256: def", ""),
            check(
                "runtime.offline",
                "pass",
                "network isolation is available",
                "",
            ),
        ]
    }

    fn status(id: &str, state: v1::ActualState) -> v1::SandboxStatus {
        v1::SandboxStatus {
            sandbox_id: id.to_owned(),
            actual_state: state as i32,
            ..Default::default()
        }
    }

    #[test]
    fn passing_doctor_report_is_compact_and_uses_one_many_grammar() {
        assert_eq!(
            render_doctor(&passing_checks(), OutputCapabilities::plain()),
            "Gascan is ready\n  Host       2/2 checks passed\n  Runtime    1/1 check passed\n"
        );
    }

    #[test]
    fn mixed_doctor_report_expands_only_failed_checks() {
        let checks = vec![
            check("host.release", "pass", "passing release detail", ""),
            check("runtime.version", "pass", "passing version detail", ""),
            check(
                "runtime.offline",
                "fail",
                "network isolation is unavailable",
                "install a supported runtime",
            ),
        ];

        let output = render_doctor(&checks, OutputCapabilities::plain());

        assert!(output.contains("Gascan needs attention"));
        assert!(output.contains("Offline"));
        assert!(output.contains("network isolation is unavailable"));
        assert!(output.contains("Fix: install a supported runtime"));
        assert!(!output.contains("passing release detail"));
        assert!(!output.contains("passing version detail"));
    }

    #[test]
    fn doctor_humanizes_unknown_groups_and_checks() {
        let checks = vec![check(
            "future_runtime.secret_probe",
            "fail",
            "probe failed",
            "retry the probe",
        )];

        let output = render_doctor(&checks, OutputCapabilities::plain());

        assert!(output.contains("  Future runtime  0/1 check passed"));
        assert!(output.contains("    Secret probe"));
    }

    #[test]
    fn status_is_a_labeled_human_summary() {
        assert_eq!(
            render_status(
                &status("code-123", v1::ActualState::Running),
                OutputCapabilities::plain()
            ),
            "Sandbox: code-123\nState:   Running\n"
        );
    }

    #[test]
    fn list_is_an_aligned_two_column_table() {
        let sandboxes = vec![
            status("code-1", v1::ActualState::Running),
            status("code-longer", v1::ActualState::Stopped),
        ];

        assert_eq!(
            render_list(&sandboxes, OutputCapabilities::plain()),
            "SANDBOX      STATE\ncode-1       Running\ncode-longer  Stopped\n"
        );
    }

    #[test]
    fn empty_list_is_explicit() {
        assert_eq!(
            render_list(&[], OutputCapabilities::plain()),
            "No sandboxes found.\n"
        );
    }

    fn event(phase: &str) -> v1::OperationEvent {
        v1::OperationEvent {
            phase: phase.to_owned(),
            ..Default::default()
        }
    }

    fn provision_event(step: v1::ProvisionStep) -> v1::OperationEvent {
        v1::OperationEvent {
            phase: "provision_step".to_owned(),
            provision_step: step as i32,
            ..Default::default()
        }
    }

    #[test]
    fn static_up_uses_semantic_messages_and_suppresses_plumbing() {
        let capabilities = OutputCapabilities::plain();
        let (mut progress, initial) = OperationProgress::new(OperationKind::Up, None, capabilities);
        assert_eq!(initial.as_deref(), Some("Preparing sandbox"));
        assert_eq!(progress.update(&event("operation")), None);
        assert_eq!(
            progress.update(&event("validated")).as_deref(),
            Some("Validating configuration")
        );
        assert_eq!(
            progress.update(&event("created")).as_deref(),
            Some("Creating sandbox")
        );
        assert_eq!(
            progress.update(&event("started")).as_deref(),
            Some("Starting sandbox")
        );
        assert_eq!(progress.update(&event("before_provision")), None);
        assert_eq!(progress.update(&event("after_provision")), None);
        assert_eq!(
            progress.update(&event("before_health")).as_deref(),
            Some("Checking sandbox health")
        );
        assert_eq!(progress.update(&event("after_health")), None);
        assert_eq!(
            progress.finish_success().as_deref(),
            Some("Sandbox is running")
        );
    }

    #[test]
    fn provision_steps_are_typed_human_copy_and_deduplicated() {
        let (mut progress, _) =
            OperationProgress::new(OperationKind::Apply, None, OutputCapabilities::plain());
        let install = provision_event(v1::ProvisionStep::InstallTools);
        assert_eq!(
            progress.update(&install).as_deref(),
            Some("Installing project tools")
        );
        assert_eq!(progress.update(&install), None);
        assert_eq!(
            progress
                .update(&provision_event(v1::ProvisionStep::RunSetup))
                .as_deref(),
            Some("Running project setup")
        );
        assert_eq!(
            progress
                .update(&provision_event(v1::ProvisionStep::VerifyGascamp))
                .as_deref(),
            Some("Verifying Gascamp")
        );
        assert_eq!(
            progress
                .update(&provision_event(v1::ProvisionStep::HealthCheck))
                .as_deref(),
            Some("Checking sandbox health")
        );
    }

    #[test]
    fn opaque_payload_and_unknown_phases_never_reach_human_output() {
        let (mut progress, _) =
            OperationProgress::new(OperationKind::Up, None, OutputCapabilities::plain());
        let mut unknown = event("private_internal_phase");
        unknown.payload = b"secret-material".to_vec();
        assert_eq!(progress.update(&unknown), None);
    }

    #[test]
    fn known_selector_is_used_only_in_completion_copy() {
        let (progress, _) = OperationProgress::new(
            OperationKind::Down,
            Some("code-123".to_owned()),
            OutputCapabilities::plain(),
        );
        assert_eq!(
            progress.finish_success().as_deref(),
            Some("Sandbox code-123 is stopped")
        );
    }

    #[test]
    fn interactive_progress_replaces_message_and_finishes_with_checkmark() {
        let terminal = InMemoryTerm::new(4, 80);
        let capabilities = OutputCapabilities {
            interactive: true,
            color: false,
            unicode: true,
        };
        let (mut progress, initial) = OperationProgress::with_draw_target(
            OperationKind::Up,
            None,
            capabilities,
            ProgressDrawTarget::term_like_with_hz(Box::new(terminal.clone()), 12),
        );
        assert_eq!(initial, None);

        std::thread::sleep(Duration::from_millis(100));
        assert!(terminal.contents().contains("Preparing sandbox"));
        assert!(
            terminal
                .contents()
                .chars()
                .any(|character| { "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏".contains(character) })
        );

        assert_eq!(progress.update(&event("validated")), None);
        std::thread::sleep(Duration::from_millis(100));
        let updated = terminal.contents();
        assert!(updated.contains("Validating configuration"));
        assert!(!updated.contains("Preparing sandbox"));
        assert_eq!(updated.lines().count(), 1);

        assert_eq!(progress.finish_success(), None);
        assert_eq!(terminal.contents(), "✓ Sandbox is running");
    }

    #[test]
    fn interactive_progress_clears_on_drop_without_completion() {
        let terminal = InMemoryTerm::new(4, 80);
        let capabilities = OutputCapabilities {
            interactive: true,
            color: false,
            unicode: true,
        };
        let (progress, initial) = OperationProgress::with_draw_target(
            OperationKind::Destroy,
            Some("code-123".to_owned()),
            capabilities,
            ProgressDrawTarget::term_like_with_hz(Box::new(terminal.clone()), 12),
        );
        assert_eq!(initial, None);
        std::thread::sleep(Duration::from_millis(100));
        assert!(!terminal.contents().is_empty());

        drop(progress);

        assert_eq!(terminal.contents(), "");
    }

    #[test]
    fn no_color_takes_precedence_over_clicolor_force() -> Result<(), Box<dyn std::error::Error>> {
        const CHILD_MARKER: &str = "GASCAN_TEST_NO_COLOR_PRECEDENCE";

        if std::env::var_os(CHILD_MARKER).is_some() {
            assert!(!OutputCapabilities::for_stdout().color);
            assert!(!OutputCapabilities::for_stderr().color);
            return Ok(());
        }

        let status = Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "presentation::tests::no_color_takes_precedence_over_clicolor_force",
                "--nocapture",
            ])
            .env(CHILD_MARKER, "1")
            .env("NO_COLOR", "1")
            .env("CLICOLOR_FORCE", "1")
            .status()?;

        assert!(status.success());
        Ok(())
    }
}
