use std::io::IsTerminal;
use std::os::fd::AsFd;

pub struct RawTerminal {
    state: std::sync::Arc<std::sync::Mutex<Option<TerminalState>>>,
}

struct TerminalState {
    fd: std::os::fd::OwnedFd,
    saved: rustix::termios::Termios,
}

#[derive(Clone)]
pub struct TerminalRestore {
    state: std::sync::Arc<std::sync::Mutex<Option<TerminalState>>>,
}

impl RawTerminal {
    pub fn acquire() -> std::io::Result<Self> {
        let stdin = std::io::stdin();
        if !stdin.is_terminal() {
            return Ok(Self {
                state: std::sync::Arc::new(std::sync::Mutex::new(None)),
            });
        }
        Self::acquire_fd(stdin.as_fd())
    }
    fn acquire_fd(fd: impl AsFd) -> std::io::Result<Self> {
        let saved = rustix::termios::tcgetattr(fd.as_fd())?;
        let mut raw = saved.clone();
        raw.make_raw();
        rustix::termios::tcsetattr(fd.as_fd(), rustix::termios::OptionalActions::Now, &raw)?;
        Ok(Self {
            state: std::sync::Arc::new(std::sync::Mutex::new(Some(TerminalState {
                fd: rustix::io::dup(fd.as_fd())?,
                saved,
            }))),
        })
    }
    pub fn restore_handle(&self) -> TerminalRestore {
        TerminalRestore {
            state: self.state.clone(),
        }
    }
}

impl TerminalRestore {
    pub fn restore(&self) {
        if let Ok(mut state) = self.state.lock() {
            if let Some(state) = state.take() {
                let _ = rustix::termios::tcsetattr(
                    &state.fd,
                    rustix::termios::OptionalActions::Now,
                    &state.saved,
                );
            }
        }
    }
}

impl Drop for RawTerminal {
    fn drop(&mut self) {
        TerminalRestore {
            state: self.state.clone(),
        }
        .restore();
    }
}

#[cfg(test)]
mod tests {
    use super::RawTerminal;

    #[test]
    #[allow(clippy::panic, reason = "test-only unwind is the behavior under test")]
    fn panic_unwind_restores_a_real_pty() -> Result<(), Box<dyn std::error::Error>> {
        let pty = rustix_openpty::openpty(None, None)?;
        let mut initial = rustix::termios::tcgetattr(&pty.user)?;
        initial
            .local_modes
            .remove(rustix::termios::LocalModes::PENDIN);
        let mut raw = initial.clone();
        raw.make_raw();
        rustix::termios::tcsetattr(&pty.user, rustix::termios::OptionalActions::Now, &raw)?;
        rustix::termios::tcsetattr(&pty.user, rustix::termios::OptionalActions::Now, &initial)?;
        let mut saved = rustix::termios::tcgetattr(&pty.user)?;
        saved
            .local_modes
            .remove(rustix::termios::LocalModes::PENDIN);
        rustix::termios::tcsetattr(&pty.user, rustix::termios::OptionalActions::Now, &saved)?;
        let saved = rustix::termios::tcgetattr(&pty.user)?;
        let result = std::panic::catch_unwind(|| {
            let _guard = RawTerminal::acquire_fd(&pty.user).map_err(std::io::Error::other)?;
            std::panic::panic_any("test-only unwind");
            #[allow(unreachable_code)]
            Ok::<(), std::io::Error>(())
        });
        assert!(result.is_err());
        let restored = rustix::termios::tcgetattr(&pty.user)?;
        assert_eq!(restored.input_modes, saved.input_modes);
        assert_eq!(restored.output_modes, saved.output_modes);
        assert_eq!(restored.control_modes, saved.control_modes);
        let mut restored_local = restored.local_modes;
        let mut saved_local = saved.local_modes;
        restored_local.remove(rustix::termios::LocalModes::PENDIN);
        saved_local.remove(rustix::termios::LocalModes::PENDIN);
        assert_eq!(restored_local, saved_local);
        Ok(())
    }
}
