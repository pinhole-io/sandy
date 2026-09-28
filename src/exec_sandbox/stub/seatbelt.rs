use std::time::Duration;

use async_trait::async_trait;

use crate::exec_sandbox::policy::ExecPolicy;
use crate::exec_sandbox::{CommandSandbox, ExecResult, SandboxError};

/// Placeholder for a future macOS Seatbelt backend.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub struct SeatbeltSandbox;

#[async_trait]
impl CommandSandbox for SeatbeltSandbox {
    fn id(&self) -> &'static str {
        "seatbelt"
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
            "seatbelt backend is not implemented".into(),
        ))
    }
}
