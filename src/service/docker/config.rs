use crate::service::docker::error::DockerError;
use regex::Regex;

/// Where the Docker image runs `uv python install` (`ENV UV_PYTHON_INSTALL_DIR` in `.docker/Dockerfile`).
/// Runtime code must pass this to `uv` (container env + `apply_sandbox_command`) because the child uses `env_clear()`.
pub const DOCKER_UV_PYTHON_INSTALL_DIR: &str = "/opt/uv-python";

/// Resource and sandbox limits applied to Docker containers.
#[derive(Debug, Clone)]
pub struct SandboxConfig {
    /// Memory limit in bytes (Docker `HostConfig.memory`).
    pub memory_bytes: i64,
    /// Total memory + swap; set equal to `memory_bytes` to effectively disable swap usage.
    pub memory_swap_bytes: i64,
    /// CPU quota in nanoseconds per second (1e9 = one full CPU).
    pub nano_cpus: i64,
    pub pids_limit: i64,
    /// When `true`, Docker uses `network_mode=none` (no outbound). When `false`, the default
    /// bridge network is used so `uv`/PyPI can reach the internet.
    pub network_disabled: bool,
    pub readonly_rootfs: bool,
    /// Max wall-clock time for a single container run or exec (seconds).
    pub timeout_secs: u64,
    pub max_output_bytes: usize,
    pub max_concurrent: usize,
    /// Container user (must exist in image, e.g. `sandbox`).
    pub container_user: String,
    /// Python version string passed to `uv` as `UV_PYTHON` (must match the image build, e.g. `3.13`).
    pub python_version: String,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        const MB: i64 = 1024 * 1024;
        Self {
            memory_bytes: 512 * MB,
            memory_swap_bytes: 512 * MB,
            nano_cpus: 1_000_000_000,
            pids_limit: 64,
            network_disabled: true,
            readonly_rootfs: true,
            timeout_secs: 30,
            max_output_bytes: 1024 * 1024,
            max_concurrent: 10,
            container_user: "sandbox".to_string(),
            python_version: "3.13".to_string(),
        }
    }
}

impl SandboxConfig {
    pub fn validate(&self) -> Result<(), DockerError> {
        if self.memory_bytes <= 0 {
            return Err(DockerError::InvalidConfig(
                "sandbox.memory_bytes must be positive".to_string(),
            ));
        }
        if self.max_output_bytes == 0 {
            return Err(DockerError::InvalidConfig(
                "sandbox.max_output_bytes must be positive".to_string(),
            ));
        }
        if self.max_concurrent == 0 {
            return Err(DockerError::InvalidConfig(
                "sandbox.max_concurrent must be positive".to_string(),
            ));
        }
        if self.timeout_secs == 0 {
            return Err(DockerError::InvalidConfig(
                "sandbox.timeout_secs must be positive".to_string(),
            ));
        }
        if self.container_user.trim().is_empty() {
            return Err(DockerError::InvalidConfig(
                "sandbox.container_user must not be empty".to_string(),
            ));
        }
        if self.python_version.trim().is_empty() {
            return Err(DockerError::InvalidConfig(
                "sandbox.python_version must not be empty".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct DockerConfig {
    pub image_name: String,
    pub image_tag: String,
    pub dockerfile_path: String,
    pub build_context: String,
    pub sandbox: SandboxConfig,
}

impl Default for DockerConfig {
    fn default() -> Self {
        Self {
            image_name: "sandy".to_string(),
            image_tag: "latest".to_string(),
            dockerfile_path: "src/sandy/.docker/Dockerfile".to_string(),
            build_context: ".".to_string(),
            sandbox: SandboxConfig::default(),
        }
    }
}

impl DockerConfig {
    /// Validate the configuration
    pub fn validate(&self) -> Result<(), DockerError> {
        self.sandbox.validate()?;
        // Validate image name (alphanumeric, dashes, underscores, slashes)
        let name_regex = Regex::new(r"^[a-zA-Z0-9][a-zA-Z0-9._/-]*$")
            .map_err(|e| DockerError::InvalidConfig(format!("Failed to compile regex: {}", e)))?;

        if !name_regex.is_match(&self.image_name) {
            return Err(DockerError::InvalidConfig(format!(
                "Invalid image name: '{}'. Must match pattern: {}",
                self.image_name,
                name_regex.as_str()
            )));
        }

        // Validate image tag (alphanumeric, dashes, underscores, periods)
        let tag_regex = Regex::new(r"^[a-zA-Z0-9][a-zA-Z0-9._-]*$")
            .map_err(|e| DockerError::InvalidConfig(format!("Failed to compile regex: {}", e)))?;

        if !tag_regex.is_match(&self.image_tag) {
            return Err(DockerError::InvalidConfig(format!(
                "Invalid image tag: '{}'. Must match pattern: {}",
                self.image_tag,
                tag_regex.as_str()
            )));
        }

        Ok(())
    }
}
