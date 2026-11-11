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

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::process::{Command, Stdio};

    use super::{archive_workspace, decode_hex, restore_workspace};

    #[test]
    fn restore_does_not_write_git_metadata() {
        let planted = tempfile::TempDir::new().expect("planted");
        let source = planted.path();
        std::fs::create_dir_all(source.join(".git/hooks")).expect("hooks");
        std::fs::write(source.join(".git/hooks/post-merge"), "#!/bin/sh\n").expect("hook");
        std::fs::write(source.join("keep.txt"), "new\n").expect("keep");
        let tar = Command::new("tar")
            .args(["-cf", "-", "-C"])
            .arg(source)
            .arg(".")
            .output()
            .expect("tar");
        assert!(tar.status.success());

        let dir = tempfile::TempDir::new().expect("dir");
        let root = dir.path().canonicalize().expect("canonical");
        std::fs::create_dir_all(root.join(".git/hooks")).expect("hooks");
        std::fs::write(root.join("keep.txt"), "old\n").expect("keep");
        restore_workspace(&root, &super::encode_hex(&tar.stdout)).expect("restore");
        assert!(!root.join(".git/hooks/post-merge").exists());
        assert_eq!(
            std::fs::read_to_string(root.join("keep.txt")).expect("keep"),
            "new\n"
        );

        let archive = archive_workspace(&root).expect("archive");
        let bytes = decode_hex(&archive).expect("hex");
        let mut list = Command::new("tar")
            .args(["-tf", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("list");
        list.stdin
            .take()
            .expect("stdin")
            .write_all(&bytes)
            .expect("write");
        let listed = list.wait_with_output().expect("wait");
        let names = String::from_utf8(listed.stdout).expect("utf8");
        assert!(!names.contains(".git"), "{names}");
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push_str(&format!("{byte:02x}"));
    }
    result
}
