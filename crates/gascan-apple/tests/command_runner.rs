use std::sync::Mutex;

use async_trait::async_trait;
use gascan_apple::{CommandOutput, CommandRunner, CommandSpec};
use gascan_core::runtime::RuntimeError;

struct RecordingRunner {
    calls: Mutex<Vec<CommandSpec>>,
    output: CommandOutput,
}

impl RecordingRunner {
    fn returning(status: i32, stdout: &[u8], stderr: &[u8]) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            output: CommandOutput {
                status,
                stdout: stdout.to_vec(),
                stderr: stderr.to_vec(),
            },
        }
    }

    fn calls(&self) -> Vec<CommandSpec> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl CommandRunner for RecordingRunner {
    async fn run(&self, spec: CommandSpec) -> Result<CommandOutput, RuntimeError> {
        self.calls.lock().unwrap().push(spec);
        Ok(self.output.clone())
    }
}

#[tokio::test]
async fn command_spec_keeps_arguments_literal() {
    let runner = RecordingRunner::returning(0, br#"{}"#, b"");
    let spec = CommandSpec::new(
        "container",
        ["inspect", "name; touch /tmp/nope", "--format", "json"],
    );
    let output = runner.run(spec).await.unwrap();
    assert_eq!(output.status, 0);
    assert_eq!(runner.calls()[0].args[1], "name; touch /tmp/nope");
}
