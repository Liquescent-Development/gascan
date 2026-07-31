use super::{ConfigureError, Prompter};
use crate::guest::Secret;
use std::io::{Read as _, Write as _};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const MAX_PROMPT_BYTES: usize = 1024 * 1024;

pub(crate) struct TerminalPrompter {
    input: std::fs::File,
    output: std::fs::File,
}

impl TerminalPrompter {
    pub(crate) fn new() -> Result<Self, ConfigureError> {
        Ok(Self {
            input: std::fs::File::from(
                rustix::io::dup(std::io::stdin()).map_err(std::io::Error::from)?,
            ),
            output: std::fs::File::from(
                rustix::io::dup(std::io::stdout()).map_err(std::io::Error::from)?,
            ),
        })
    }

    #[cfg(test)]
    pub(super) fn from_files(input: std::fs::File, output: std::fs::File) -> Self {
        Self { input, output }
    }

    fn write_prompt(&mut self, prompt: &str) -> Result<(), ConfigureError> {
        self.output.write_all(prompt.as_bytes())?;
        self.output.flush()?;
        Ok(())
    }
}

impl Prompter for TerminalPrompter {
    fn confirm(&mut self, prompt: &str, default: bool) -> Result<bool, ConfigureError> {
        let Some(answer) = self.line(prompt, None)? else {
            return Err(ConfigureError::Cancelled);
        };
        if answer.is_empty() {
            return Ok(default);
        }
        match answer.to_ascii_lowercase().as_str() {
            "y" | "yes" => Ok(true),
            "n" | "no" => Ok(false),
            _ => Err(ConfigureError::InvalidOutput { category: "prompt" }),
        }
    }

    fn line(
        &mut self,
        prompt: &str,
        default: Option<&str>,
    ) -> Result<Option<String>, ConfigureError> {
        self.write_prompt(prompt)?;
        let Some(bytes) = read_line(&mut self.input)? else {
            return Ok(None);
        };
        let value = String::from_utf8(bytes)
            .map_err(|_| ConfigureError::InvalidOutput { category: "prompt" })?;
        if value.is_empty() {
            return Ok(default.map(ToOwned::to_owned).or(Some(value)));
        }
        Ok(Some(value))
    }

    fn secret(&mut self, prompt: &str) -> Result<Option<Secret>, ConfigureError> {
        let mut hidden = HiddenInput::acquire(&self.input)?;
        self.write_prompt(prompt)?;
        let result = hidden.read_secret();
        drop(hidden);
        self.output.write_all(b"\n")?;
        self.output.flush()?;
        let bytes = result?;
        if bytes.is_empty() {
            return Err(ConfigureError::Cancelled);
        }
        Ok(Some(Secret::new(bytes)))
    }
}

fn read_line(input: &mut std::fs::File) -> Result<Option<Vec<u8>>, ConfigureError> {
    let mut bytes = Vec::new();
    let mut byte = [0_u8; 1];
    loop {
        match input.read(&mut byte) {
            Ok(0) if bytes.is_empty() => return Ok(None),
            Ok(0) => break,
            Ok(_) if byte[0] == b'\n' => break,
            Ok(_) if byte[0] == 3 => return Err(ConfigureError::Cancelled),
            Ok(_) => {
                if bytes.len() == MAX_PROMPT_BYTES {
                    return Err(ConfigureError::InvalidOutput { category: "prompt" });
                }
                bytes.push(byte[0]);
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(ConfigureError::Io(error)),
        }
    }
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    Ok(Some(bytes))
}

pub(super) struct HiddenInput {
    fd: std::os::fd::OwnedFd,
    saved_termios: rustix::termios::Termios,
    saved_flags: rustix::fs::OFlags,
}

impl HiddenInput {
    pub(super) fn acquire(fd: impl std::os::fd::AsFd) -> std::io::Result<Self> {
        let fd = rustix::io::dup(fd)?;
        let saved_termios = rustix::termios::tcgetattr(&fd)?;
        let saved_flags = rustix::fs::fcntl_getfl(&fd)?;
        let hidden = Self {
            fd,
            saved_termios,
            saved_flags,
        };
        let mut no_echo = hidden.saved_termios.clone();
        no_echo
            .local_modes
            .remove(rustix::termios::LocalModes::ECHO);
        rustix::termios::tcsetattr(&hidden.fd, rustix::termios::OptionalActions::Now, &no_echo)?;
        rustix::fs::fcntl_setfl(
            &hidden.fd,
            hidden.saved_flags | rustix::fs::OFlags::NONBLOCK,
        )?;
        Ok(hidden)
    }

    fn read_secret(&mut self) -> Result<Vec<u8>, ConfigureError> {
        let interrupted = Arc::new(AtomicBool::new(false));
        let registration =
            signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&interrupted))?;
        let _registration = InterruptRegistration(registration);
        let mut bytes = Vec::new();
        let mut byte = [0_u8; 1];
        loop {
            if interrupted.load(Ordering::Relaxed) {
                return Err(ConfigureError::Cancelled);
            }
            match rustix::io::read(&self.fd, &mut byte) {
                Ok(0) => return Err(ConfigureError::Cancelled),
                Ok(_) if byte[0] == 3 => return Err(ConfigureError::Cancelled),
                Ok(_) if byte[0] == b'\n' => break,
                Ok(_) => {
                    if bytes.len() == MAX_PROMPT_BYTES {
                        return Err(ConfigureError::InvalidOutput { category: "prompt" });
                    }
                    bytes.push(byte[0]);
                }
                Err(rustix::io::Errno::AGAIN) | Err(rustix::io::Errno::INTR) => {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Err(error) => return Err(ConfigureError::Io(std::io::Error::from(error))),
            }
        }
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
        Ok(bytes)
    }
}

impl Drop for HiddenInput {
    fn drop(&mut self) {
        let _ = rustix::fs::fcntl_setfl(&self.fd, self.saved_flags);
        let _ = rustix::termios::tcsetattr(
            &self.fd,
            rustix::termios::OptionalActions::Now,
            &self.saved_termios,
        );
    }
}

struct InterruptRegistration(signal_hook::SigId);

impl Drop for InterruptRegistration {
    fn drop(&mut self) {
        signal_hook::low_level::unregister(self.0);
    }
}
