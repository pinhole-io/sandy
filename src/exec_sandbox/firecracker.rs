use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use crate::service::firecracker::{FirecrackerConfig, FirecrackerService};

use super::error::SandboxError;
use super::policy::ExecPolicy;
use super::{CommandSandbox, ExecResult};

pub struct FirecrackerShellSandbox {
    config: FirecrackerConfig,
}

impl FirecrackerShellSandbox {
    pub fn new() -> Self {
        Self {
            config: FirecrackerConfig::default(),
        }
    }

    pub fn with_config(config: FirecrackerConfig) -> Self {
        Self { config }
    }
}

impl Default for FirecrackerShellSandbox {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CommandSandbox for FirecrackerShellSandbox {
    fn id(&self) -> &'static str {
        "firecracker"
    }

    fn is_available(&self) -> bool {
        FirecrackerService::new(self.config.clone())
            .map(|service| service.is_available())
            .unwrap_or(false)
    }

    async fn run_shell(
        &self,
        policy: &ExecPolicy,
        command: &str,
        timeout: Duration,
    ) -> Result<ExecResult, SandboxError> {
        let cwd = policy
            .cwd
            .canonicalize()
            .map_err(|error| SandboxError::InvalidPolicy(format!("cwd: {error}")))?;
        for root in &policy.writable_roots {
            let root = root
                .canonicalize()
                .map_err(|error| SandboxError::InvalidPolicy(format!("writable root: {error}")))?;
            if !root.starts_with(&cwd) {
                return Err(SandboxError::InvalidPolicy(format!(
                    "Firecracker shell currently supports writable roots below cwd only: {}",
                    root.display()
                )));
            }
        }

        let workspace_hex = archive_workspace(&cwd)?;
        let mut config = self.config.clone();
        config.timeout_secs = timeout.as_secs().max(1);
        if policy.allow_network {
            config.tap_device = std::env::var("SANDY_FC_TAP").ok();
            if config.tap_device.is_none() {
                return Err(SandboxError::Unavailable(
                    "network access requires SANDY_FC_TAP".into(),
                ));
            }
        }
        let service = FirecrackerService::new(config)
            .map_err(|error| SandboxError::Unavailable(error.to_string()))?;
        let (result, updated_workspace) = service
            .execute_shell(command, workspace_hex, policy.env.resolve())
            .await
            .map_err(|error| SandboxError::SpawnFailed(error.to_string()))?;

        if let Some(updated_workspace) = updated_workspace {
            restore_workspace(&cwd, &updated_workspace)?;
        }
        Ok(ExecResult {
            stdout: result.stdout,
            stderr: result.stderr,
            exit_code: Some(result.exit_code),
        })
    }
}

pub fn firecracker_backend_if_available() -> Option<Arc<dyn CommandSandbox>> {
    let backend = FirecrackerShellSandbox::new();
    if backend.is_available() {
        Some(Arc::new(backend))
    } else {
        None
    }
}

fn archive_workspace(cwd: &Path) -> Result<String, SandboxError> {
    let output = Command::new("tar")
        .args(["-cf", "-", "--exclude=.git", "-C"])
        .arg(cwd)
        .arg(".")
        .output()
        .map_err(SandboxError::Io)?;
    if !output.status.success() {
        return Err(SandboxError::SpawnFailed(
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
    }
    Ok(encode_hex(&output.stdout))
}

fn restore_workspace(cwd: &Path, archive_hex: &str) -> Result<(), SandboxError> {
    let archive = decode_hex(archive_hex)?;
    let mut child = Command::new("tar")
        .args(["-xf", "-", "--exclude=.git", "-C"])
        .arg(cwd)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(SandboxError::Io)?;
    use std::io::Write;
    child
        .stdin
        .take()
        .ok_or_else(|| SandboxError::SpawnFailed("tar stdin unavailable".into()))?
        .write_all(&archive)
        .map_err(SandboxError::Io)?;
    let output = child.wait_with_output().map_err(SandboxError::Io)?;
    if !output.status.success() {
        return Err(SandboxError::SpawnFailed(
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
    }
    Ok(())
}

fn decode_hex(value: &str) -> Result<Vec<u8>, SandboxError> {
    if !value.len().is_multiple_of(2) {
        return Err(SandboxError::SpawnFailed(
            "guest workspace archive has invalid hex length".into(),
        ));
    }
    (0..value.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&value[index..index + 2], 16).map_err(|_| {
                SandboxError::SpawnFailed("guest workspace archive has invalid hex".into())
            })
        })
        .collect()
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push_str(&format!("{byte:02x}"));
    }
    result
}
