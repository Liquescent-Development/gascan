use std::process::Stdio;

use gascan_core::runtime::RuntimeError;
use tokio::{io::AsyncWriteExt, process::Command};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub stdin: Vec<u8>,
}

impl CommandSpec {
    pub fn new<P, I, A>(program: P, args: I) -> Self
    where
        P: Into<String>,
        I: IntoIterator<Item = A>,
        A: Into<String>,
    {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
            stdin: Vec::new(),
        }
    }

    pub fn with_stdin(mut self, stdin: impl Into<Vec<u8>>) -> Self {
        self.stdin = stdin.into();
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandOutput {
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[async_trait::async_trait]
pub trait CommandRunner: Send + Sync {
    async fn run(&self, spec: CommandSpec) -> Result<CommandOutput, RuntimeError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ProcessRunner;

#[async_trait::async_trait]
impl CommandRunner for ProcessRunner {
    async fn run(&self, spec: CommandSpec) -> Result<CommandOutput, RuntimeError> {
        let operation = spec.program.clone();
        let mut child = Command::new(&spec.program)
            .args(&spec.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| RuntimeError::CommandIo {
                operation: operation.clone(),
                message: error.to_string(),
            })?;

        if !spec.stdin.is_empty() {
            child
                .stdin
                .as_mut()
                .expect("piped stdin is available")
                .write_all(&spec.stdin)
                .await
                .map_err(|error| RuntimeError::CommandIo {
                    operation: operation.clone(),
                    message: error.to_string(),
                })?;
        }
        drop(child.stdin.take());

        let output = child
            .wait_with_output()
            .await
            .map_err(|error| RuntimeError::CommandIo {
                operation: operation.clone(),
                message: error.to_string(),
            })?;
        let status = match output.status.code() {
            Some(status) => status,
            None => {
                let stderr = decode_diagnostic(&operation, output.stderr)?;
                return Err(RuntimeError::CommandFailed {
                    operation,
                    exit_code: None,
                    stderr,
                });
            }
        };

        if !output.status.success() {
            let stderr = decode_diagnostic(&operation, output.stderr)?;
            return Err(RuntimeError::CommandFailed {
                operation,
                exit_code: Some(status),
                stderr,
            });
        }

        Ok(CommandOutput {
            status,
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }
}

fn decode_diagnostic(operation: &str, bytes: Vec<u8>) -> Result<String, RuntimeError> {
    String::from_utf8(bytes).map_err(|error| RuntimeError::InvalidOutput {
        operation: operation.to_owned(),
        message: format!("stderr is not UTF-8: {error}"),
    })
}
