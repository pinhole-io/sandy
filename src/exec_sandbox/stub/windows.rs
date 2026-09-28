use std::time::Duration;

use async_trait::async_trait;

use crate::exec_sandbox::policy::ExecPolicy;
use crate::exec_sandbox::{CommandSandbox, ExecResult, SandboxError};

/// Placeholder for a future Windows sandbox backend.
#[cfg_attr(not(windows), allow(dead_code))]
pub struct WindowsSandbox;

#[async_trait]
impl CommandSandbox for WindowsSandbox {
    fn id(&self) -> &'static str {
        "windows"
    }

    fn is_available(&self) -> bool {
        false
    }

    async fn run_shell(
        &self,
        _policy: &ExecPolicy,
        _command: &str,
        _timeout: Duration,
    ) -> Result<ExecResult, SandboxError> {
        Err(SandboxError::Unavailable(
            "windows sandbox backend is not implemented".into(),
        ))
    }
}
