use std::io::IsTerminal;
use std::os::fd::AsFd;

pub struct RawTerminal {
    saved: std::sync::Arc<std::sync::Mutex<Option<rustix::termios::Termios>>>,
}

#[derive(Clone)]
pub struct TerminalRestore {
    saved: std::sync::Arc<std::sync::Mutex<Option<rustix::termios::Termios>>>,
}

impl RawTerminal {
    pub fn acquire() -> std::io::Result<Self> {
        let stdin = std::io::stdin();
        if !stdin.is_terminal() {
            return Ok(Self {
                saved: std::sync::Arc::new(std::sync::Mutex::new(None)),
            });
        }
        let saved = rustix::termios::tcgetattr(stdin.as_fd())?;
        let mut raw = saved.clone();
        raw.make_raw();
        rustix::termios::tcsetattr(stdin.as_fd(), rustix::termios::OptionalActions::Now, &raw)?;
        Ok(Self {
            saved: std::sync::Arc::new(std::sync::Mutex::new(Some(saved))),
        })
    }
    pub fn restore_handle(&self) -> TerminalRestore {
        TerminalRestore {
            saved: self.saved.clone(),
        }
    }
}

impl TerminalRestore {
    pub fn restore(&self) {
        if let Ok(mut saved) = self.saved.lock() {
            if let Some(saved) = saved.take() {
                let stdin = std::io::stdin();
                let _ = rustix::termios::tcsetattr(
                    stdin.as_fd(),
                    rustix::termios::OptionalActions::Now,
                    &saved,
                );
            }
        }
    }
}

impl Drop for RawTerminal {
    fn drop(&mut self) {
        TerminalRestore {
            saved: self.saved.clone(),
        }
        .restore();
    }
}
