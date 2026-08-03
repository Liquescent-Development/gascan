use super::{ConfigureError, ConfigureIo, Prompter};
use crate::guest::Secret;
use crate::presentation::OutputCapabilities;
use console::Style;
use std::io::{IsTerminal as _, Write as _};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

const MAX_PROMPT_BYTES: usize = 1024 * 1024;

pub(crate) struct TerminalPrompter {
    input: std::fs::File,
    output: std::fs::File,
    error: std::fs::File,
    output_palette: ConfigurePalette,
    error_palette: ConfigurePalette,
}

#[derive(Clone, Copy)]
pub(crate) struct ConfigurePalette {
    capabilities: OutputCapabilities,
}

impl ConfigurePalette {
    pub(crate) const fn new(capabilities: OutputCapabilities) -> Self {
        Self { capabilities }
    }

    pub(crate) fn heading(&self, text: &str) -> String {
        self.render(text, Style::new().cyan().bold())
    }

    pub(crate) fn prompt(&self, text: &str) -> String {
        self.render(text, Style::new().cyan())
    }

    pub(crate) fn hint(&self, text: &str) -> String {
        self.render(text, Style::new().dim())
    }

    pub(crate) fn success(&self, text: &str) -> String {
        self.symbol("✓", text, Style::new().green())
    }

    pub(crate) fn warning(&self, text: &str) -> String {
        self.symbol("⚠", text, Style::new().yellow())
    }

    pub(crate) fn failure(&self, text: &str) -> String {
        self.symbol("✗", text, Style::new().red())
    }

    fn symbol(&self, symbol: &str, text: &str, style: Style) -> String {
        let text = if self.capabilities.unicode_enabled() {
            format!("{symbol} {text}")
        } else {
            text.to_owned()
        };
        self.render(&text, style)
    }

    fn render(&self, text: &str, style: Style) -> String {
        if self.capabilities.color_enabled() {
            style.force_styling(true).apply_to(text).to_string()
        } else {
            text.to_owned()
        }
    }
}

impl TerminalPrompter {
    pub(crate) fn new() -> Result<Self, ConfigureError> {
        Ok(Self::with_capabilities(
            std::fs::File::from(rustix::io::dup(std::io::stdin()).map_err(std::io::Error::from)?),
            std::fs::File::from(rustix::io::dup(std::io::stdout()).map_err(std::io::Error::from)?),
            std::fs::File::from(rustix::io::dup(std::io::stderr()).map_err(std::io::Error::from)?),
            OutputCapabilities::for_stdout(),
            OutputCapabilities::for_stderr(),
        ))
    }

    #[cfg(test)]
    pub(super) fn from_files(
        input: std::fs::File,
        output: std::fs::File,
        error: std::fs::File,
    ) -> Self {
        Self::from_files_with_capabilities(
            input,
            output,
            error,
            OutputCapabilities::test(false, false, false),
            OutputCapabilities::test(false, false, false),
        )
    }

    #[cfg(test)]
    pub(super) fn from_files_with_capabilities(
        input: std::fs::File,
        output: std::fs::File,
        error: std::fs::File,
        output_capabilities: OutputCapabilities,
        error_capabilities: OutputCapabilities,
    ) -> Self {
        Self::with_capabilities(
            input,
            output,
            error,
            output_capabilities,
            error_capabilities,
        )
    }

    fn with_capabilities(
        input: std::fs::File,
        output: std::fs::File,
        error: std::fs::File,
        output_capabilities: OutputCapabilities,
        error_capabilities: OutputCapabilities,
    ) -> Self {
        Self {
            input,
            output,
            error,
            output_palette: ConfigurePalette::new(output_capabilities),
            error_palette: ConfigurePalette::new(error_capabilities),
        }
    }

    fn write_prompt(&mut self, prompt: &str) -> Result<(), ConfigureError> {
        let prompt = self.error_palette.prompt(prompt);
        self.error.write_all(prompt.as_bytes())?;
        self.error.flush()?;
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
        let interrupt = PromptInterrupt::acquire()?;
        let input = NonblockingInput::acquire(&self.input)?;
        self.write_prompt(prompt)?;
        let Some(bytes) = read_line(&input.fd, &interrupt)? else {
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
        let interrupt = PromptInterrupt::acquire()?;
        let mut hidden = HiddenInput::acquire(&self.input)?;
        self.write_prompt(prompt)?;
        let result = hidden.read_secret(&interrupt);
        drop(hidden);
        self.error.write_all(b"\n")?;
        self.error.flush()?;
        let bytes = result?;
        if bytes.is_empty() {
            return Err(ConfigureError::Cancelled);
        }
        Ok(Some(Secret::new(bytes)))
    }
}

impl ConfigureIo for TerminalPrompter {
    fn write_out(&mut self, text: &str) -> Result<(), ConfigureError> {
        self.output.write_all(text.as_bytes())?;
        self.output.flush()?;
        Ok(())
    }

    fn write_err(&mut self, text: &str) -> Result<(), ConfigureError> {
        self.error.write_all(text.as_bytes())?;
        self.error.flush()?;
        Ok(())
    }

    fn write_heading(&mut self, text: &str) -> Result<(), ConfigureError> {
        let text = self.error_palette.heading(text);
        self.write_err(&text)
    }

    fn write_hint(&mut self, text: &str) -> Result<(), ConfigureError> {
        let text = self.error_palette.hint(text);
        self.write_err(&text)
    }

    fn write_success(&mut self, text: &str) -> Result<(), ConfigureError> {
        let text = self.output_palette.success(text);
        self.write_out(&text)
    }

    fn write_warning(&mut self, text: &str) -> Result<(), ConfigureError> {
        let text = self.error_palette.warning(text);
        self.write_err(&text)
    }

    fn write_failure(&mut self, text: &str) -> Result<(), ConfigureError> {
        let text = self.error_palette.failure(text);
        self.write_err(&text)
    }

    fn stdin_is_terminal(&self) -> bool {
        std::io::stdin().is_terminal()
    }

    fn stderr_is_terminal(&self) -> bool {
        std::io::stderr().is_terminal()
    }
}

fn read_line(
    input: impl std::os::fd::AsFd,
    interrupt: &PromptInterrupt<'_>,
) -> Result<Option<Vec<u8>>, ConfigureError> {
    let mut bytes = Vec::new();
    let mut byte = [0_u8; 1];
    loop {
        if interrupt.was_triggered() {
            return Err(ConfigureError::Cancelled);
        }
        match rustix::io::read(&input, &mut byte) {
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
            Err(rustix::io::Errno::AGAIN) | Err(rustix::io::Errno::INTR) => {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Err(error) => return Err(ConfigureError::Io(std::io::Error::from(error))),
        }
    }
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    Ok(Some(bytes))
}

struct NonblockingInput {
    fd: std::os::fd::OwnedFd,
    saved_flags: rustix::fs::OFlags,
}

impl NonblockingInput {
    fn acquire(fd: impl std::os::fd::AsFd) -> std::io::Result<Self> {
        let fd = rustix::io::dup(fd)?;
        let saved_flags = rustix::fs::fcntl_getfl(&fd)?;
        let input = Self { fd, saved_flags };
        rustix::fs::fcntl_setfl(&input.fd, input.saved_flags | rustix::fs::OFlags::NONBLOCK)?;
        Ok(input)
    }
}

impl Drop for NonblockingInput {
    fn drop(&mut self) {
        let _ = rustix::fs::fcntl_setfl(&self.fd, self.saved_flags);
    }
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

    fn read_secret(&mut self, interrupt: &PromptInterrupt<'_>) -> Result<Vec<u8>, ConfigureError> {
        let mut bytes = Vec::new();
        let mut byte = [0_u8; 1];
        loop {
            if interrupt.was_triggered() {
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

struct PromptSignal {
    _default_registration: signal_hook::SigId,
    _interrupt_registration: signal_hook::SigId,
    default_when_inactive: Arc<AtomicBool>,
    triggered: Arc<AtomicBool>,
    prompt_gate: Mutex<()>,
}

impl PromptSignal {
    fn register() -> std::io::Result<Self> {
        let default_when_inactive = Arc::new(AtomicBool::new(true));
        let triggered = Arc::new(AtomicBool::new(false));
        let default_registration = signal_hook::flag::register_conditional_default(
            signal_hook::consts::SIGINT,
            Arc::clone(&default_when_inactive),
        )?;
        let interrupt_registration =
            signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&triggered))?;
        Ok(Self {
            _default_registration: default_registration,
            _interrupt_registration: interrupt_registration,
            default_when_inactive,
            triggered,
            prompt_gate: Mutex::new(()),
        })
    }

    fn begin(&'static self) -> PromptInterrupt<'static> {
        let prompt_gate = match self.prompt_gate.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        self.triggered.store(false, Ordering::SeqCst);
        self.default_when_inactive.store(false, Ordering::SeqCst);
        PromptInterrupt {
            signal: self,
            _prompt_gate: prompt_gate,
        }
    }
}

enum PromptSignalState {
    Ready(PromptSignal),
    Failed(std::io::ErrorKind),
}

static PROMPT_SIGNAL: OnceLock<PromptSignalState> = OnceLock::new();

struct PromptInterrupt<'a> {
    signal: &'a PromptSignal,
    _prompt_gate: MutexGuard<'a, ()>,
}

impl PromptInterrupt<'static> {
    fn acquire() -> std::io::Result<Self> {
        match PROMPT_SIGNAL.get_or_init(|| match PromptSignal::register() {
            Ok(signal) => PromptSignalState::Ready(signal),
            Err(error) => PromptSignalState::Failed(error.kind()),
        }) {
            PromptSignalState::Ready(signal) => Ok(signal.begin()),
            PromptSignalState::Failed(kind) => Err(std::io::Error::new(
                *kind,
                "SIGINT prompt handler could not be installed",
            )),
        }
    }
}

impl PromptInterrupt<'_> {
    fn was_triggered(&self) -> bool {
        self.signal.triggered.load(Ordering::SeqCst)
    }
}

impl Drop for PromptInterrupt<'_> {
    fn drop(&mut self) {
        self.signal
            .default_when_inactive
            .store(true, Ordering::SeqCst);
        self.signal.triggered.store(false, Ordering::SeqCst);
    }
}
