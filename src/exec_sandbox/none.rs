use std::time::Duration;

use async_trait::async_trait;
use tokio::process::Command;

use super::command::run_command_with_timeout;
use super::error::SandboxError;
use super::policy::ExecPolicy;
use super::CommandSandbox;
use super::ExecResult;

pub struct NoneSandbox;

#[async_trait]
impl CommandSandbox for NoneSandbox {
    fn id(&self) -> &'static str {
        "none"
    }

    fn is_available(&self) -> bool {
        true
    }

    async fn run_shell(
        &self,
        policy: &ExecPolicy,
        command: &str,
        timeout: Duration,
    ) -> Result<ExecResult, SandboxError> {
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(command)
            .current_dir(&policy.cwd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        super::env::apply_env(&mut cmd, &policy.env);

        let output = run_command_with_timeout(cmd, timeout, policy.max_output_bytes).await?;
        Ok(ExecResult::from_output(output))
    }

    async fn run_shell_streaming(
        &self,
        policy: &ExecPolicy,
        command: &str,
        timeout: Duration,
        on_output: Option<super::result::OutputSink>,
    ) -> Result<ExecResult, SandboxError> {
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(command)
            .current_dir(&policy.cwd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        super::env::apply_env(&mut cmd, &policy.env);

        let output = super::command::run_command_with_timeout_and_output(
            cmd,
            timeout,
            policy.max_output_bytes,
            on_output,
        )
        .await?;
        Ok(ExecResult::from_output(output))
    }
}
