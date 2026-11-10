use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use super::error::SandboxError;
use super::policy::ExecPolicy;

/// Which pipe produced a live output chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputStream {
    Stdout,
    Stderr,
}

/// Receives stdout/stderr bytes while a command still runs.
pub type OutputSink = Arc<dyn Fn(OutputStream, &[u8]) + Send + Sync>;

/// Emit captured result text through an optional live sink.
pub fn emit_result_output(on_output: Option<&OutputSink>, result: &ExecResult) {
    let Some(sink) = on_output else {
        return;
    };
    if !result.stdout.is_empty() {
        sink(OutputStream::Stdout, result.stdout.as_bytes());
    }
    if !result.stderr.is_empty() {
        sink(OutputStream::Stderr, result.stderr.as_bytes());
    }
}

#[derive(Debug, Clone)]
pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
}

impl ExecResult {
    pub fn from_output(output: std::process::Output) -> Self {
        Self {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            exit_code: output.status.code(),
        }
    }
}

#[async_trait]
pub trait CommandSandbox: Send + Sync {
    fn id(&self) -> &'static str;
    fn is_available(&self) -> bool;

    async fn run_shell(
        &self,
        policy: &ExecPolicy,
        command: &str,
        timeout: Duration,
    ) -> Result<ExecResult, SandboxError>;

    async fn run_shell_streaming(
        &self,
        policy: &ExecPolicy,
        command: &str,
        timeout: Duration,
        on_output: Option<OutputSink>,
    ) -> Result<ExecResult, SandboxError> {
        let result = self.run_shell(policy, command, timeout).await?;
        emit_result_output(on_output.as_ref(), &result);
        Ok(result)
    }

    /// Start one long-lived sandbox for `policy`.
    ///
    /// The default fails closed. A backend that implements this reuses the
    /// same mounts for later commands. It must not run the command on the host.
    async fn open_warm(&self, _policy: &ExecPolicy) -> Result<Box<dyn WarmCommand>, SandboxError> {
        let _ = _policy;
        Err(SandboxError::Unavailable(
            "warm session is unavailable".into(),
        ))
    }
}

/// One command inside a warm sandbox session.
#[async_trait]
pub trait WarmCommand: Send {
    /// Run `command` with `cwd` inside the session roots.
    async fn run(
        &mut self,
        cwd: &Path,
        command: &str,
        max_output_bytes: usize,
        timeout: Duration,
    ) -> Result<ExecResult, SandboxError>;

    /// Host pid of the sandbox process. It stays the same across `run` calls.
    fn host_pid(&self) -> u32;
}
