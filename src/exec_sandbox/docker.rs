use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use crate::service::docker::{DockerConfig, DockerService, SandboxConfig};

use super::error::SandboxError;
use super::policy::ExecPolicy;
use super::CommandSandbox;
use super::ExecResult;

const SHELL_IMAGE: &str = "alpine";
const SHELL_TAG: &str = "3.20";

pub struct DockerShellSandbox {
    config: DockerConfig,
}

impl DockerShellSandbox {
    pub fn new() -> Self {
        let sandbox = SandboxConfig {
            container_user: "root".to_string(),
            network_disabled: true,
            ..Default::default()
        };

        Self {
            config: DockerConfig {
                image_name: SHELL_IMAGE.into(),
                image_tag: SHELL_TAG.into(),
                dockerfile_path: String::new(),
                build_context: String::new(),
                sandbox,
            },
        }
    }
}

impl Default for DockerShellSandbox {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CommandSandbox for DockerShellSandbox {
    fn id(&self) -> &'static str {
        "docker"
    }

    fn is_available(&self) -> bool {
        // Lightweight check: docker CLI on PATH (image pull happens at run time).
        std::process::Command::new("docker")
            .arg("info")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    async fn run_shell(
        &self,
        policy: &ExecPolicy,
        command: &str,
        timeout: Duration,
    ) -> Result<ExecResult, SandboxError> {
        let mut config = self.config.clone();
        config.sandbox.network_disabled = !policy.allow_network;
        config.sandbox.timeout_secs = timeout.as_secs().max(1);

        let service = DockerService::new(config)
            .await
            .map_err(|e| SandboxError::Unavailable(e.to_string()))?;

        let cwd = policy
            .cwd
            .canonicalize()
            .map_err(|e| SandboxError::InvalidPolicy(format!("cwd: {e}")))?;
        let mount_host = cwd.display().to_string();
        let mounts = vec![(mount_host.clone(), "/workspace".into())];

        let docker_cmd = vec![
            "sh".into(),
            "-c".into(),
            format!("cd /workspace && {command}"),
        ];

        let (stdout, stderr, exit_code) = service
            .run_command_with_mounts(docker_cmd, None, Some(mounts))
            .await
            .map_err(|e| SandboxError::SpawnFailed(e.to_string()))?;

        Ok(ExecResult {
            stdout,
            stderr,
            exit_code: Some(exit_code),
        })
    }
}

pub fn docker_backend_if_available() -> Option<Arc<dyn CommandSandbox>> {
    let backend = DockerShellSandbox::new();
    if backend.is_available() {
        Some(Arc::new(backend))
    } else {
        None
    }
}
