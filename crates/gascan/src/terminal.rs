use std::io::IsTerminal;
use std::os::fd::AsFd;

pub struct RawTerminal {
    saved: Option<rustix::termios::Termios>,
}

impl RawTerminal {
    pub fn acquire() -> std::io::Result<Self> {
        let stdin = std::io::stdin();
        if !stdin.is_terminal() {
            return Ok(Self { saved: None });
        }
        let saved = rustix::termios::tcgetattr(stdin.as_fd())?;
        let mut raw = saved.clone();
        raw.make_raw();
        rustix::termios::tcsetattr(stdin.as_fd(), rustix::termios::OptionalActions::Now, &raw)?;
        Ok(Self { saved: Some(saved) })
    }
}

impl Drop for RawTerminal {
    fn drop(&mut self) {
        if let Some(saved) = &self.saved {
            let stdin = std::io::stdin();
            let _ = rustix::termios::tcsetattr(
                stdin.as_fd(),
                rustix::termios::OptionalActions::Now,
                saved,
            );
        }
    }
}
