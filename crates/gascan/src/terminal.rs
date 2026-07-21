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
        Self::acquire_fd_with(
            fd,
            |fd| rustix::io::dup(fd).map_err(std::io::Error::from),
            || Ok(()),
        )
    }
    fn acquire_fd_with(
        fd: impl AsFd,
        duplicate: impl FnOnce(std::os::fd::BorrowedFd<'_>) -> std::io::Result<std::os::fd::OwnedFd>,
        after_raw: impl FnOnce() -> std::io::Result<()>,
    ) -> std::io::Result<Self> {
        let saved = rustix::termios::tcgetattr(fd.as_fd())?;
        let terminal = Self {
            state: std::sync::Arc::new(std::sync::Mutex::new(Some(TerminalState {
                fd: duplicate(fd.as_fd())?,
                saved: saved.clone(),
            }))),
        };
        let mut raw = saved.clone();
        raw.make_raw();
        rustix::termios::tcsetattr(fd.as_fd(), rustix::termios::OptionalActions::Now, &raw)?;
        after_raw()?;
        Ok(terminal)
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

    fn assert_modes_equal(left: &rustix::termios::Termios, right: &rustix::termios::Termios) {
        let mut left_local = left.local_modes;
        let mut right_local = right.local_modes;
        left_local.remove(rustix::termios::LocalModes::PENDIN);
        right_local.remove(rustix::termios::LocalModes::PENDIN);
        assert_eq!(left.input_modes, right.input_modes);
        assert_eq!(left.output_modes, right.output_modes);
        assert_eq!(left.control_modes, right.control_modes);
        assert_eq!(left_local, right_local);
    }

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

    #[test]
    fn setup_failures_never_leave_a_real_pty_raw() -> Result<(), Box<dyn std::error::Error>> {
        let pty = rustix_openpty::openpty(None, None)?;
        let saved = rustix::termios::tcgetattr(&pty.user)?;

        let duplicate_error = RawTerminal::acquire_fd_with(
            &pty.user,
            |_| Err(std::io::Error::other("injected duplicate failure")),
            || Ok(()),
        );
        assert!(duplicate_error.is_err());
        assert_modes_equal(&rustix::termios::tcgetattr(&pty.user)?, &saved);

        let setup_error = RawTerminal::acquire_fd_with(
            &pty.user,
            |fd| rustix::io::dup(fd).map_err(std::io::Error::from),
            || Err(std::io::Error::other("injected post-mutation failure")),
        );
        assert!(setup_error.is_err());
        assert_modes_equal(&rustix::termios::tcgetattr(&pty.user)?, &saved);
        Ok(())
    }
}
