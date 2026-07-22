// Task 1 establishes the presentation API; command integration follows in later tasks.
#![allow(dead_code)]

use console::Term;
use gascan_proto::v1;
use indicatif::{ProgressBar, ProgressStyle};
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

pub(crate) struct OperationProgress {
    kind: OperationKind,
    sandbox_id: Option<String>,
    progress_bar: Option<ProgressBar>,
    last_message: Option<&'static str>,
}

impl OperationProgress {
    pub(crate) fn new(
        kind: OperationKind,
        sandbox_id: Option<String>,
        capabilities: OutputCapabilities,
    ) -> (Self, Option<String>) {
        let initial = match kind {
            OperationKind::Up => "Preparing sandbox",
            OperationKind::Apply => "Applying configuration",
            OperationKind::Down => "Stopping sandbox",
            OperationKind::Destroy => "Destroying sandbox",
        };
        let progress_bar = capabilities.interactive.then(|| {
            let progress_bar = ProgressBar::new_spinner();
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
            progress_bar.finish_with_message(completion);
            None
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

#[cfg(test)]
mod tests {
    use super::*;
    use gascan_proto::v1;
    use std::process::Command;

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
